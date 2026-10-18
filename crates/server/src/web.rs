use crate::{db, App};
use anyhow::{anyhow, Result};
use argon2::{
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};
use axum::{
    body::Body,
    extract::{
        ws::{Message, WebSocket},
        ConnectInfo, DefaultBodyLimit, Path, Query, State, WebSocketUpgrade,
    },
    http::{header, HeaderMap, StatusCode},
    middleware::{self, Next},
    response::{sse::{Event, Sse}, IntoResponse, Response},
    routing::{get, post, put},
    Json, Router,
};
use hproxy_common::*;
use rusqlite::{params, OptionalExtension};
use serde::Deserialize;
use serde_json::{json, Value};
use std::net::SocketAddr;

#[cfg(test)]
#[path = "core_tests.rs"]
mod core_tests;
#[cfg(test)]
#[path = "address_tests.rs"]
mod address_tests;
#[cfg(test)]
#[path = "log_tests.rs"]
mod log_tests;
#[cfg(test)]
#[path = "update_tests.rs"]
mod update_tests;
#[cfg(test)]
#[path = "cleanup_tests.rs"]
mod cleanup_tests;

#[derive(Debug)]
pub struct ApiError(pub StatusCode, pub String);
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(json!({"error":self.1}))).into_response()
    }
}
impl From<anyhow::Error> for ApiError {
    fn from(e: anyhow::Error) -> Self {
        tracing::warn!(error=%e,"请求失败");
        Self(StatusCode::BAD_REQUEST, e.to_string())
    }
}
impl From<rusqlite::Error> for ApiError {
    fn from(e: rusqlite::Error) -> Self {
        tracing::error!(error=%e,"数据库操作失败");
        Self(StatusCode::INTERNAL_SERVER_ERROR, "数据库操作失败".into())
    }
}
type Api<T> = std::result::Result<Json<T>, ApiError>;
fn unauthorized() -> ApiError {
    ApiError(StatusCode::UNAUTHORIZED, "请重新登录".into())
}
fn not_found() -> ApiError {
    ApiError(StatusCode::NOT_FOUND, "记录不存在".into())
}
pub fn hash_password(password: &str) -> Result<String> {
    let salt = SaltString::generate(&mut rand::rngs::OsRng);
    Ok(Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map_err(|e| anyhow!("{e}"))?
        .to_string())
}
pub fn router(app: App) -> Router {
    let protected = Router::new()
        .route("/me", get(me))
        .route("/logout", post(logout))
        .route("/password", put(change_password))
        .route("/overview", get(overview))
        .route("/agents", get(agents).post(create_agent))
        .route("/agents/{id}", put(edit_agent).delete(remove_agent))
        .route("/agents/{id}/record", axum::routing::delete(remove_agent_record))
        .route("/agents/{id}/enrollment", post(renew_enrollment))
        .route("/agents/{id}/restart", post(restart_agent))
        .route("/agents/{id}/cores/{core}", put(manage_core))
        .route("/agents/{id}/configs/{core}", get(read_config))
        .route("/agents/{id}/state", put(server_state))
        .route("/agents/{id}/revoke", post(revoke_agent))
        .route("/resources", get(list_resources).post(save_resource))
        .route("/resources/import", post(import_resources))
        .route(
            "/resources/{id}",
            put(update_resource).delete(remove_resource),
        )
        .route("/resources/{id}/link", get(resource_link))
        .route("/generate/{kind}", post(generate))
        .route("/jobs", get(jobs))
        .route("/logs", get(logs))
        .route("/logs/events", get(log_events))
        .route("/traffic", get(traffic))
        .route("/settings", get(settings).put(save_settings))
        .route("/updates", get(update_status))
        .route("/updates/check", post(check_updates))
        .route("/updates/apply", post(apply_updates))
        .route_layer(middleware::from_fn_with_state(app.clone(), authenticate));
    Router::new()
        .nest(
            "/api/v1",
            protected.merge(Router::new().route("/login", post(login))),
        )
        .route("/agent/v1/enroll", post(enroll))
        .route("/agent/v1/connect", get(connect))
        .route("/agent/v1/removal", post(removal_result))
        .route(
            "/install.sh",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "text/x-shellscript; charset=utf-8")],
                    include_str!("../../../scripts/install-agent.sh"),
                )
            }),
        )
        .route(
            "/agentctl.sh",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "text/x-shellscript; charset=utf-8")],
                    include_str!("../../../scripts/agentctl.sh"),
                )
            }),
        )
        .route(
            "/uninstall-agent.sh",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "text/x-shellscript; charset=utf-8")],
                    include_str!("../../../scripts/uninstall-agent.sh"),
                )
            }),
        )
        .route("/downloads/{file}", get(download_file))
        .route(
            "/healthz",
            get(|| async { Json(json!({"ok":true,"version":env!("CARGO_PKG_VERSION")})) }),
        )
        .fallback(static_file)
        .layer(DefaultBodyLimit::max(MAX_FRAME))
        .layer(middleware::from_fn(security_headers))
        .with_state(app)
}
async fn security_headers(request: axum::extract::Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    let h = response.headers_mut();
    for (k,v) in [("x-content-type-options","nosniff"),("referrer-policy","no-referrer"),("x-frame-options","DENY"),("cache-control","no-store"),("content-security-policy","default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; connect-src 'self'; frame-ancestors 'none'; base-uri 'none'")] {h.insert(axum::http::HeaderName::from_static(k),v.parse().unwrap());}
    response
}
fn session(headers: &HeaderMap) -> Option<String> {
    headers
        .get(header::COOKIE)?
        .to_str()
        .ok()?
        .split(';')
        .find_map(|c| c.trim().strip_prefix("hproxy_session=").map(digest))
}
fn check_origin(app: &App, h: &HeaderMap) -> Result<()> {
    if let Some(origin) = h.get(header::ORIGIN) {
        anyhow::ensure!(origin.to_str()? == app.origin, "请求来源不匹配");
    }
    Ok(())
}
async fn authenticate(
    State(app): State<App>,
    headers: HeaderMap,
    request: axum::extract::Request,
    next: Next,
) -> Response {
    let result = (|| -> Result<()> {
        let hash = session(&headers).ok_or_else(|| anyhow!("未登录"))?;
        let csrf = app
            .connection()
            .query_row(
                "SELECT csrf FROM sessions WHERE hash=? AND expires>?",
                params![hash, now()],
                |r| r.get::<_, String>(0),
            )
            .optional()?
            .ok_or_else(|| anyhow!("会话已过期"))?;
        if !matches!(request.method().as_str(), "GET" | "HEAD" | "OPTIONS") {
            check_origin(&app, &headers)?;
            anyhow::ensure!(
                headers.get("x-csrf-token").and_then(|v| v.to_str().ok()) == Some(csrf.as_str()),
                "CSRF 校验失败"
            );
        }
        Ok(())
    })();
    match result {
        Ok(()) => next.run(request).await,
        Err(_) => unauthorized().into_response(),
    }
}
#[derive(Deserialize)]
struct Password {
    password: String,
}
async fn login(
    State(app): State<App>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(body): Json<Password>,
) -> std::result::Result<Response, ApiError> {
    check_origin(&app, &headers)?;
    let ip = peer.ip().to_string();
    {
        let mut attempts = app.attempts.lock().unwrap();
        if attempts.len() > 10_000 {
            attempts.retain(|_, (t, _)| now() - *t < 900_000)
        }
        let (time, count) = attempts.entry(ip.clone()).or_insert((now(), 0));
        if now() - *time > 900_000 {
            *time = now();
            *count = 0
        }
        if *count >= 10 {
            return Err(ApiError(
                StatusCode::TOO_MANY_REQUESTS,
                "尝试次数过多，请 15 分钟后重试".into(),
            ));
        }
        *count += 1;
    }
    let hash = db::meta(&app.connection(), "password")?.unwrap_or_default();
    let parsed = PasswordHash::new(&hash).map_err(|_| unauthorized())?;
    if body.password.len() > 1024
        || Argon2::default()
            .verify_password(body.password.as_bytes(), &parsed)
            .is_err()
    {
        return Err(unauthorized());
    }
    app.attempts.lock().unwrap().remove(&ip);
    let token = secret(32);
    let csrf = secret(24);
    app.connection().execute(
        "INSERT INTO sessions VALUES(?,?,?)",
        params![digest(&token), csrf, now() + 12 * 3_600_000],
    )?;
    let secure = if app.secure { "; Secure" } else { "" };
    let cookie =
        format!("hproxy_session={token}; Path=/; HttpOnly; SameSite=Strict; Max-Age=43200{secure}");
    Ok(([(header::SET_COOKIE, cookie)], Json(json!({"csrf":csrf}))).into_response())
}
async fn me(State(app): State<App>, headers: HeaderMap) -> Api<Value> {
    let csrf: String = app.connection().query_row(
        "SELECT csrf FROM sessions WHERE hash=?",
        [session(&headers).ok_or_else(unauthorized)?],
        |r| r.get(0),
    )?;
    Ok(Json(
        json!({"csrf":csrf,"version":env!("CARGO_PKG_VERSION")}),
    ))
}
async fn logout(
    State(app): State<App>,
    headers: HeaderMap,
) -> std::result::Result<Response, ApiError> {
    app.connection().execute(
        "DELETE FROM sessions WHERE hash=?",
        [session(&headers).ok_or_else(unauthorized)?],
    )?;
    Ok((
        [(
            header::SET_COOKIE,
            "hproxy_session=; Path=/; HttpOnly; SameSite=Strict; Max-Age=0",
        )],
        Json(json!({"ok":true})),
    )
        .into_response())
}
#[derive(Deserialize)]
struct ChangePassword {
    current: String,
    password: String,
}
async fn change_password(State(app): State<App>, Json(b): Json<ChangePassword>) -> Api<Value> {
    let mut c = app.connection();
    let old = db::meta(&c, "password")?.unwrap_or_default();
    let hash = PasswordHash::new(&old).map_err(|_| unauthorized())?;
    if Argon2::default()
        .verify_password(b.current.as_bytes(), &hash)
        .is_err()
    {
        return Err(ApiError(StatusCode::BAD_REQUEST, "当前密码不正确".into()));
    }
    if b.password.len() < 12 || b.password.len() > 1024 {
        return Err(ApiError(
            StatusCode::BAD_REQUEST,
            "新密码须为 12–1024 字节".into(),
        ));
    }
    let hash = hash_password(&b.password)?;
    let tx = c.transaction()?;
    tx.execute("UPDATE meta SET value=? WHERE key='password'", [hash])?;
    tx.execute("DELETE FROM sessions", [])?;
    tx.commit()?;
    Ok(Json(json!({"ok":true})))
}
async fn agents(State(app): State<App>) -> Api<Value> {
    Ok(Json(db::list_agents(&app.connection())?))
}
#[derive(Deserialize)]
struct AgentInput {
    name: String,
    #[serde(default)]
    address: String,
    #[serde(default)]
    cores: Option<Vec<String>>,
}
fn validate_agent(b: &AgentInput) -> Result<()> {
    anyhow::ensure!(
        !b.name.trim().is_empty() && b.name.len() <= 200,
        "名称长度无效"
    );
    if !b.address.is_empty() {
        validate_host(&b.address)?;
    }
    if let Some(cores) = &b.cores { validate_core_policy(cores, &[])?; }
    Ok(())
}
fn new_enrollment(c: &rusqlite::Connection, agent: &str) -> Result<Value> {
    let token = secret(32);
    c.execute("DELETE FROM enrollment WHERE agent_id=?", [agent])?;
    c.execute(
        "INSERT INTO enrollment VALUES(?,?,?)",
        params![digest(&token), agent, now() + 600_000],
    )?;
    Ok(json!({"agent_id":agent,"token":token,"expires_at":now()+600_000}))
}
async fn create_agent(State(app): State<App>, Json(b): Json<AgentInput>) -> Api<Value> {
    validate_agent(&b)?;
    let mut c = app.connection();
    let tx = c.transaction()?;
    let agent = id();
    let cores = b.cores.unwrap_or_else(|| CORES.iter().map(|core| core.to_string()).collect());
    tx.execute(
        "INSERT INTO agents(id,name,address,core_policy) VALUES(?,?,?,?)",
        params![agent, b.name.trim(), b.address, json!(cores).to_string()],
    )?;
    let token = new_enrollment(&tx, &agent)?;
    tx.commit()?;
    Ok(Json(installation(&app, token)))
}
async fn renew_enrollment(State(app): State<App>, Path(id): Path<String>) -> Api<Value> {
    let mut c = app.connection();
    let tx = c.transaction()?;
    ensure_mutable(&tx, &id)?;
    if tx.execute(
        "UPDATE agents SET token_hash=NULL,revoked=0,last_seen=0 WHERE id=?",
        [&id],
    )? == 0
    {
        return Err(not_found());
    }
    let token = new_enrollment(&tx, &id)?;
    tx.commit()?;
    Ok(Json(installation(&app, token)))
}
fn installation(app: &App, mut token: Value) -> Value {
    token["controller"] = json!(app.origin);
    token
}
async fn edit_agent(
    State(app): State<App>,
    Path(id): Path<String>,
    Json(b): Json<AgentInput>,
) -> Api<Value> {
    validate_agent(&b)?;
    ensure_mutable(&app.connection(), &id)?;
    if app.connection().execute(
        "UPDATE agents SET name=?,address=? WHERE id=?",
        params![b.name.trim(), b.address, id],
    )? == 0
    {
        return Err(not_found());
    }
    Ok(Json(json!({"ok":true})))
}
async fn remove_agent(State(app): State<App>, Path(id): Path<String>) -> Api<Value> {
    let mut c = app.connection();
    let tx = c.transaction()?;
    let (enrolled, supported, pending, first_seen): (bool, bool, String, i64) = tx.query_row(
        "SELECT token_hash IS NOT NULL,uninstall_supported,deletion_id,first_seen FROM agents WHERE id=?", [&id],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
    ).optional()?.ok_or_else(not_found)?;
    if !enrolled && first_seen > 0 { return Err(ApiError(StatusCode::CONFLICT, "服务器接入凭据已失效，请重新接入后执行远程清理".into())); }
    if enrolled && !supported {
        return Err(ApiError(StatusCode::CONFLICT, "请先更新并连接子 agent，确认支持远程清理后再删除服务器".into()));
    }
    // Remote cleanup owns child-machine files. Controller history expires normally.
    tx.execute("DELETE FROM resources WHERE agent_id=?", [&id])?;
    tx.execute("DELETE FROM enrollment WHERE agent_id=?", [&id])?;
    let request_id = if pending.is_empty() { hproxy_common::id() } else { pending };
    if enrolled {
        tx.execute("UPDATE agents SET deletion_id=?,enabled=0,error=NULL WHERE id=?", params![request_id,id])?;
    } else {
        tx.execute("DELETE FROM agents WHERE id=?", [&id])?;
    }
    tx.commit()?;
    app.logs_changed();
    app.commands_changed();
    Ok(Json(json!({"ok":true,"cleanup":if enrolled {"queued"} else {"complete"},"request_id":request_id})))
}
// Manual fallback for an agent whose installation has already been removed on
// its host, or whose remote cleanup could not complete.
// Never claim that this operation removed files from the remote machine.
async fn remove_agent_record(State(app): State<App>, Path(id): Path<String>) -> Api<Value> {
    if app.sockets.lock().unwrap().contains_key(&id) {
        return Err(ApiError(StatusCode::CONFLICT, "子 agent 仍连接主控，请先在子机停止并卸载".into()));
    }
    let mut c = app.connection();
    let result = forget_offline_agent(&mut c, &id)?;
    app.logs_changed();
    app.commands_changed();
    Ok(Json(result))
}
fn forget_offline_agent(c: &mut rusqlite::Connection, id: &str) -> Result<Value, ApiError> {
    let tx = c.transaction()?;
    let last_seen: i64 = tx
        .query_row("SELECT last_seen FROM agents WHERE id=?", [id], |row| row.get(0))
        .optional()?
        .ok_or_else(not_found)?;
    if last_seen > now() - 60_000 {
        return Err(ApiError(StatusCode::CONFLICT, "请先在子机停止并卸载 agent，等待至少 60 秒确认离线".into()));
    }
    tx.execute("DELETE FROM agents WHERE id=?", [id])?;
    tx.commit()?;
    Ok(json!({"ok":true,"cleanup":"record_only"}))
}
#[cfg(test)]
#[test]
fn record_only_removal_requires_stale_connection_and_keeps_traffic() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = db::open(&dir.path().join("hproxy.db")).unwrap();
    let id = "00000000-0000-4000-8000-000000000001";
    c.execute("INSERT INTO agents(id,name,token_hash,last_seen,first_seen) VALUES(?,'agent','token',?,1)", params![id,now()]).unwrap();
    c.execute("INSERT INTO resources(id,agent_id,body) VALUES('r',?,?)", params![id,r#"{"spec":{"type":"forward"}}"#]).unwrap();
    c.execute("INSERT INTO counters(agent_id,resource_id,upload,download) VALUES(?,'r',9,11)", [id]).unwrap();
    assert_eq!(forget_offline_agent(&mut c,id).err().unwrap().0,StatusCode::CONFLICT);
    c.execute("UPDATE agents SET last_seen=? WHERE id=?", params![now()-61_000,id]).unwrap();
    assert_eq!(forget_offline_agent(&mut c,id).unwrap()["cleanup"],"record_only");
    assert_eq!(c.query_row("SELECT count(*) FROM agents",[],|r|r.get::<_,i64>(0)).unwrap(),0);
    assert_eq!(c.query_row("SELECT count(*) FROM resources",[],|r|r.get::<_,i64>(0)).unwrap(),0);
    assert_eq!(c.query_row("SELECT upload+download FROM counters WHERE resource_id='r'",[],|r|r.get::<_,i64>(0)).unwrap(),20);
}
async fn revoke_agent(State(app): State<App>, Path(id): Path<String>) -> Api<Value> {
    ensure_mutable(&app.connection(), &id)?;
    app.connection()
        .execute("UPDATE agents SET revoked=1,last_seen=0 WHERE id=?", [id])?;
    Ok(Json(json!({"ok":true})))
}
async fn restart_agent(State(app): State<App>, Path(id): Path<String>) -> Api<Value> {
    let mut c = app.connection();
    let tx = c.transaction()?;
    ensure_mutable(&tx, &id)?;
    let rev = db::bump(&tx, &id)?;
    tx.commit()?;
    Ok(Json(json!({"revision":rev})))
}
#[derive(Deserialize)]
struct CoreInput {
    installed: bool,
}
#[derive(Deserialize)]
struct ServerState {
    enabled: bool,
}
async fn server_state(
    State(app): State<App>,
    Path(id): Path<String>,
    Json(input): Json<ServerState>,
) -> Api<Value> {
    let mut c = app.connection();
    let tx = c.transaction()?;
    ensure_mutable(&tx, &id)?;
    if tx.execute(
        "UPDATE agents SET enabled=?,revoked=0 WHERE id=?",
        params![input.enabled, id],
    )? == 0
    {
        return Err(not_found());
    }
    let revision = db::bump(&tx, &id)?;
    tx.commit()?;
    Ok(Json(json!({"revision":revision})))
}
fn desired_resources(c: &rusqlite::Connection, agent: &str) -> Result<Vec<Resource>> {
    let enabled: bool = c.query_row("SELECT enabled FROM agents WHERE id=?", [agent], |r| {
        r.get(0)
    })?;
    let mut resources = db::resources(c, Some(agent))?;
    if !enabled {
        for r in &mut resources {
            r.enabled = false;
        }
    }
    Ok(resources)
}
async fn manage_core(
    State(app): State<App>,
    Path((id, core)): Path<(String, String)>,
    Json(input): Json<CoreInput>,
) -> Api<Value> {
    Ok(Json(set_core(&app, &id, &core, input.installed)?))
}
fn set_core(app: &App, id: &str, core: &str, installed: bool) -> Result<Value> {
    anyhow::ensure!(CORES.contains(&core), "核心名称无效");
    let mut c = app.connection();
    let tx = c.transaction()?;
    ensure_mutable(&tx, id)?;
    let (supported, revoked, last_seen): (bool, bool, i64) = tx
        .query_row(
            "SELECT core_management,revoked,last_seen FROM agents WHERE id=?",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?
        .ok_or_else(|| anyhow!("服务器不存在"))?;
    anyhow::ensure!(supported, "请先更新并连接子 agent，再管理核心");
    anyhow::ensure!(!revoked, "服务器连接已撤销");
    anyhow::ensure!(last_seen > now() - 35_000, "服务器离线，连接后可管理核心");
    let mut cores =
        db::core_policy(&tx, &id)?.unwrap_or_else(|| CORES.iter().map(|c| c.to_string()).collect());
    if installed {
        if !cores.iter().any(|c| c == core) {
            cores.push(core.into());
        }
    } else {
        cores.retain(|c| c != core);
    }
    validate_core_policy(&cores, &db::resources(&tx, Some(&id))?)?;
    tx.execute(
        "UPDATE agents SET core_policy=? WHERE id=?",
        params![serde_json::to_string(&cores)?, id],
    )?;
    let revision = db::bump(&tx, &id)?;
    tx.commit()?;
    app.commands_changed();
    Ok(json!({"revision":revision}))
}
async fn list_resources(State(app): State<App>) -> Api<Value> {
    let c = app.connection();
    let resources = visible_resources(&c)?;
    let counters = db::resource_counters(&c, &resources)?;
    Ok(Json(json!({"resources":resources,"counters":counters})))
}
fn visible_resources(c: &rusqlite::Connection) -> Result<Vec<Resource>> {
    let agents = db::list_agents(c)?;
    let ids: std::collections::HashSet<_> = agents
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|agent| agent["id"].as_str())
        .collect();
    Ok(db::resources(c, None)?
        .into_iter()
        .filter(|resource| ids.contains(resource.agent_id.as_str()))
        .collect())
}
fn store_resource(app: &App, mut r: Resource, new: bool) -> Result<Value> {
    let mut c = app.connection();
    let tx = c.transaction()?;
    crate::addresses::normalize(&tx, &mut r)?;
    r.validate()?;
    ensure_mutable(&tx, &r.agent_id)?;
    if r.quota_cycle.is_some() {
        let supported: bool = tx.query_row("SELECT quota_cycles FROM agents WHERE id=?", [&r.agent_id], |row| row.get(0))?;
        anyhow::ensure!(supported, "请先更新并连接子 agent，以支持流量周期重置");
    }
    let exists: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM resources WHERE id=?)",
        [&r.id],
        |x| x.get(0),
    )?;
    anyhow::ensure!(
        exists != new,
        if new {
            "配置 ID 已存在"
        } else {
            "配置不存在"
        }
    );
    let old_agent: Option<String> = tx
        .query_row("SELECT agent_id FROM resources WHERE id=?", [&r.id], |x| {
            x.get(0)
        })
        .optional()?;
    if let Some(old) = old_agent {
        anyhow::ensure!(
            old == r.agent_id,
            "配置不能直接移动到另一服务器，请新建配置"
        );
    }
    let mut all = db::resources(&tx, Some(&r.agent_id))?;
    all.retain(|x| x.id != r.id);
    all.push(r.clone());
    validate_resources(&all)?;
    db::ensure_resource_cores(&tx, &r.agent_id, &all)?;
    tx.execute(
        "INSERT INTO resources VALUES(?,?,?) ON CONFLICT(id) DO UPDATE SET body=excluded.body",
        params![r.id, r.agent_id, serde_json::to_string(&r)?],
    )?;
    let revision = db::bump(&tx, &r.agent_id)?;
    tx.commit()?;
    app.commands_changed();
    Ok(json!({"id":r.id,"revision":revision}))
}
async fn save_resource(State(app): State<App>, Json(r): Json<Resource>) -> Api<Value> {
    Ok(Json(store_resource(&app, r, true)?))
}
#[derive(Deserialize)]
struct ImportResources {
    resources: Vec<Resource>,
}
async fn import_resources(State(app): State<App>, Json(mut b): Json<ImportResources>) -> Api<Value> {
    if b.resources.is_empty() || b.resources.len() > 512 {
        return Err(ApiError(
            StatusCode::BAD_REQUEST,
            "一次导入 1–512 条配置".into(),
        ));
    }
    let mut c = app.connection();
    let tx = c.transaction()?;
    let mut affected = std::collections::BTreeSet::new();
    let mut imported_ids = std::collections::BTreeSet::new();
    for r in &mut b.resources {
        crate::addresses::normalize(&tx, r)?;
        r.validate()?;
        ensure_mutable(&tx, &r.agent_id)?;
        let exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM resources WHERE id=?)",
            [&r.id],
            |row| row.get(0),
        )?;
        if exists || !imported_ids.insert(r.id.clone()) {
            return Err(ApiError(
                StatusCode::BAD_REQUEST,
                "导入包含重复的配置 ID".into(),
            ));
        }
        let agent_exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM agents WHERE id=?)",
            [&r.agent_id],
            |row| row.get(0),
        )?;
        if !agent_exists {
            return Err(ApiError(
                StatusCode::BAD_REQUEST,
                "导入的服务器不存在".into(),
            ));
        }
        tx.execute(
            "INSERT INTO resources VALUES(?,?,?)",
            params![
                r.id,
                r.agent_id,
                serde_json::to_string(r).map_err(anyhow::Error::from)?
            ],
        )?;
        affected.insert(r.agent_id.clone());
    }
    for agent in affected {
        let resources = db::resources(&tx, Some(&agent))?;
        validate_resources(&resources)?;
        db::ensure_resource_cores(&tx, &agent, &resources)?;
        db::bump(&tx, &agent)?;
    }
    tx.commit()?;
    Ok(Json(json!({"imported":b.resources.len()})))
}
async fn update_resource(
    State(app): State<App>,
    Path(id): Path<String>,
    Json(r): Json<Resource>,
) -> Api<Value> {
    if id != r.id {
        return Err(ApiError(StatusCode::BAD_REQUEST, "ID 不匹配".into()));
    }
    Ok(Json(store_resource(&app, r, false)?))
}
async fn remove_resource(State(app): State<App>, Path(id): Path<String>) -> Api<Value> {
    let mut c = app.connection();
    let tx = c.transaction()?;
    let agent: String = tx
        .query_row("SELECT agent_id FROM resources WHERE id=?", [&id], |r| {
            r.get(0)
        })
        .optional()?
        .ok_or_else(not_found)?;
    ensure_mutable(&tx, &agent)?;
    let (enrolled, supported): (bool, bool) = tx.query_row("SELECT token_hash IS NOT NULL,cleanup_supported FROM agents WHERE id=?", [&agent], |row| Ok((row.get(0)?,row.get(1)?)))?;
    if enrolled && !supported { return Err(ApiError(StatusCode::CONFLICT, "请先更新并连接子 agent，以支持本地配置清理".into())); }
    tx.execute("DELETE FROM resources WHERE id=?", [&id])?;
    let revision = db::bump(&tx, &agent)?;
    tx.commit()?;
    app.logs_changed();
    app.commands_changed();
    Ok(Json(json!({"ok":true,"revision":revision})))
}

fn ensure_mutable(c: &rusqlite::Connection, agent: &str) -> Result<()> {
    let removing: String = c.query_row("SELECT deletion_id FROM agents WHERE id=?", [agent], |row| row.get(0)).optional()?.ok_or_else(|| anyhow!("服务器不存在"))?;
    anyhow::ensure!(removing.is_empty(), "服务器正在删除，等待子机清理完成");
    Ok(())
}
async fn resource_link(State(app): State<App>, Path(id): Path<String>) -> Api<Value> {
    let r = db::resources(&app.connection(), None)?
        .into_iter()
        .find(|r| r.id == id)
        .ok_or_else(not_found)?;
    Ok(Json(json!({"link":r.link()})))
}
async fn generate(Path(kind): Path<String>) -> Api<Spec> {
    Ok(Json(Resource::generate(&kind)?))
}
async fn overview(State(app): State<App>) -> Api<Value> {
    let c = app.connection();
    let agents = db::list_agents(&c)?;
    let resources = visible_resources(&c)?;
    Ok(Json(
        json!({"agents":agents,"nodes":resources.iter().filter(|r|!r.is_forward()).count(),"forwards":resources.iter().filter(|r|r.is_forward()).count()}),
    ))
}
async fn jobs(State(app): State<App>) -> Api<Value> {
    let c = app.connection();
    let mut s=c.prepare("SELECT id,agent_id,revision,state,error,created_at,finished_at FROM jobs ORDER BY created_at DESC LIMIT 100")?;
    let rows=s.query_map([],|r|Ok(json!({"id":r.get::<_,String>(0)?,"agent_id":r.get::<_,String>(1)?,"revision":r.get::<_,i64>(2)?,"state":r.get::<_,String>(3)?,"error":r.get::<_,Option<String>>(4)?,"created_at":r.get::<_,i64>(5)?,"finished_at":r.get::<_,Option<i64>>(6)?})))?.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(Json(json!(rows)))
}
#[derive(Deserialize, Default)]
struct Filter {
    agent_id: Option<String>,
    resource_id: Option<String>,
    q: Option<String>,
    from: Option<i64>,
    to: Option<i64>,
    page: Option<u32>,
    page_size: Option<u32>,
    source: Option<String>,
    target: Option<String>,
    network: Option<String>,
    status: Option<String>,
}
const LOG_FILTER_SQL: &str = "FROM (
          SELECT *, CASE WHEN domain='' THEN destination ELSE domain ||
            CASE WHEN destination GLOB '*:[0-9]*' THEN substr(destination,length(rtrim(destination,'0123456789'))) ELSE '' END END AS target_address
          FROM connections
        )
        WHERE (?1 IS NULL OR agent_id=?1) AND (?2 IS NULL OR resource_id=?2)
          AND (?3='' OR instr(lower(source),lower(?3))>0 OR instr(lower(destination),lower(?3))>0 OR instr(lower(target_address),lower(?3))>0)
          AND started_at>=?4 AND started_at<=?5
          AND resource_id IN (SELECT id FROM resource_types WHERE kind IN ('vless','shadowsocks'))
          AND (?6='' OR instr(lower(source),lower(?6))>0)
          AND (?7='' OR instr(lower(destination),lower(?7))>0 OR instr(lower(target_address),lower(?7))>0)
          AND (?8='' OR network=?8)
          AND (?9='' OR (?9='active' AND closed_at IS NULL AND substr(result,1,5)!='error')
            OR (?9='closed' AND closed_at IS NOT NULL AND substr(result,1,5)!='error')
            OR (?9='failed' AND substr(result,1,5)='error'))";

fn log_filter_params(f: &Filter) -> Result<Vec<rusqlite::types::Value>> {
    let q = f.q.as_deref().unwrap_or("").trim();
    let source = f.source.as_deref().unwrap_or("").trim();
    let target = f.target.as_deref().unwrap_or("").trim();
    let network = f.network.as_deref().unwrap_or("");
    let status = f.status.as_deref().unwrap_or("");
    if [q, source, target].iter().any(|s| s.len() > 512) {
        anyhow::bail!("筛选内容不能超过 512 字节");
    }
    if !["", "tcp", "udp"].contains(&network)
        || !["", "active", "closed", "failed"].contains(&status)
    {
        anyhow::bail!("无效的协议或连接状态");
    }
    if f.from.unwrap_or(0) < 0 || f.from.unwrap_or(0) > f.to.unwrap_or(i64::MAX) {
        anyhow::bail!("结束时间不能早于开始时间");
    }
    Ok(vec![
        f.agent_id.clone().into(),
        f.resource_id.clone().into(),
        q.to_owned().into(),
        f.from.unwrap_or(0).into(),
        f.to.unwrap_or(i64::MAX).into(),
        source.to_owned().into(),
        target.to_owned().into(),
        network.to_owned().into(),
        status.to_owned().into(),
    ])
}
fn query_logs(c: &rusqlite::Connection, f: &Filter, limit: u32, offset: u64) -> Result<Value> {
    let mut values = log_filter_params(f)?;
    values.push(limit.into());
    values.push(i64::try_from(offset)?.into());
    let mut s = c.prepare(&format!(
        "SELECT agent_id,id,resource_id,network,source,destination,domain,started_at,closed_at,upload,download,result {LOG_FILTER_SQL} ORDER BY started_at DESC,agent_id,id LIMIT ?10 OFFSET ?11"
    ))?;
    let rows=s.query_map(rusqlite::params_from_iter(values),|r|Ok(json!({"agent_id":r.get::<_,String>(0)?,"id":r.get::<_,String>(1)?,"resource_id":r.get::<_,String>(2)?,"network":r.get::<_,String>(3)?,"source":r.get::<_,String>(4)?,"destination":r.get::<_,String>(5)?,"domain":r.get::<_,String>(6)?,"started_at":r.get::<_,i64>(7)?,"closed_at":r.get::<_,Option<i64>>(8)?,"upload":r.get::<_,u64>(9)?,"download":r.get::<_,u64>(10)?,"result":r.get::<_,String>(11)?})))?.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(json!(rows))
}
fn query_log_page(c: &rusqlite::Connection, f: &Filter) -> Result<Value> {
    let page_size = f.page_size.unwrap_or(20);
    anyhow::ensure!(
        [20, 50, 100].contains(&page_size),
        "每页条数须为 20、50 或 100"
    );
    let total: u64 = c.query_row(
        &format!("SELECT COUNT(*) {LOG_FILTER_SQL}"),
        rusqlite::params_from_iter(log_filter_params(f)?),
        |r| r.get(0),
    )?;
    let pages = total.div_ceil(u64::from(page_size)).max(1);
    let page = u64::from(f.page.unwrap_or(1)).clamp(1, pages);
    let rows = query_logs(c, f, page_size, (page - 1) * u64::from(page_size))?;
    Ok(
        json!({"rows":rows,"total":total,"pages":pages,"page_size":page_size,"page":page,"has_more":page<pages}),
    )
}
async fn logs(State(app): State<App>, Query(f): Query<Filter>) -> Api<Value> {
    Ok(Json(query_log_page(&app.connection(), &f)?))
}

async fn log_events(State(app): State<App>, headers: HeaderMap) -> std::result::Result<Response, ApiError> {
    check_origin(&app, &headers)?;
    let hash = session(&headers).ok_or_else(unauthorized)?;
    let changes = app.log_changes.subscribe();
    let stream = futures_util::stream::unfold((app, changes, hash, true), |(app, mut changes, hash, first)| async move {
        let event_name = if first {
            Some("ready")
        } else {
            tokio::select! {
                result = changes.changed() => {
                    if result.is_err() { return None; }
                    Some("logs")
                }
                _ = tokio::time::sleep(std::time::Duration::from_secs(15)) => None,
            }
        };
        // Opening authentication is insufficient for a long-lived stream: logout,
        // password changes and expiry must also stop an already connected client.
        let authenticated = app.connection().query_row(
            "SELECT EXISTS(SELECT 1 FROM sessions WHERE hash=? AND expires>?)",
            params![hash, now()], |row| row.get::<_, bool>(0),
        ).unwrap_or(false);
        if !authenticated { return None; }
        let event = match event_name {
            Some(name) => {
                let revision = *changes.borrow_and_update();
                Event::default().event(name).data(revision.to_string())
            }
            None => Event::default().comment("keep-alive"),
        };
        Some((Ok::<_, std::convert::Infallible>(event), (app, changes, hash, false)))
    });
    let mut response = Sse::new(stream).into_response();
    response.headers_mut().insert("x-accel-buffering", "no".parse().unwrap());
    Ok(response)
}
async fn traffic(State(app): State<App>, Query(f): Query<Filter>) -> Api<Value> {
    let c = app.connection();
    let mut s=c.prepare("SELECT hour,COALESCE(SUM(upload),0),COALESCE(SUM(download),0) FROM traffic WHERE hour>=?1 AND hour<=?2 AND (?3 IS NULL OR agent_id=?3) AND (?4 IS NULL OR resource_id=?4) AND resource_id IN (SELECT id FROM resource_types WHERE kind!='forward') GROUP BY hour ORDER BY hour LIMIT 9000")?;
    let rows=s.query_map(params![f.from.unwrap_or(now()-86_400_000),f.to.unwrap_or(now()),f.agent_id,f.resource_id],|r|Ok(json!({"at":r.get::<_,i64>(0)?,"upload":r.get::<_,u64>(1)?,"download":r.get::<_,u64>(2)?})))?.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(Json(json!({"rows":rows})))
}
async fn settings(State(app): State<App>) -> Api<Value> {
    let days = db::meta(&app.connection(), "log_days")?
        .unwrap_or("7".into())
        .parse::<u32>()
        .unwrap_or(7);
    Ok(Json(
        json!({"log_days":days,"timezone":"Asia/Shanghai","sing_box":SING_VERSION,"realm":REALM_VERSION,"version":env!("CARGO_PKG_VERSION")}),
    ))
}
async fn update_status(State(app): State<App>) -> Api<Value> {
    Ok(Json(app.updates.status(&app)?))
}
async fn check_updates(State(app): State<App>) -> Api<Value> {
    app.updates.check()?;
    Ok(Json(json!({"ok":true})))
}
#[derive(Deserialize)]
struct UpdateInput {
    build: String,
    #[serde(default)]
    component: crate::updates::Component,
}
async fn apply_updates(State(app): State<App>, Json(input): Json<UpdateInput>) -> Api<Value> {
    if !release::valid_hash(&input.build) {
        return Err(ApiError(StatusCode::BAD_REQUEST, "更新版本无效".into()));
    }
    app.updates
        .clone()
        .install(app, input.build, input.component)?;
    Ok(Json(json!({"ok":true})))
}
#[derive(Deserialize)]
struct Settings {
    log_days: u32,
}
async fn save_settings(State(app): State<App>, Json(b): Json<Settings>) -> Api<Value> {
    if !(1..=365).contains(&b.log_days) {
        return Err(ApiError(
            StatusCode::BAD_REQUEST,
            "日志保留期须为 1–365 天".into(),
        ));
    }
    let c = app.connection();
    c.execute(
        "UPDATE meta SET value=? WHERE key='log_days'",
        [b.log_days.to_string()],
    )?;
    db::cleanup(&c)?;
    app.logs_changed();
    Ok(Json(json!({"ok":true})))
}

#[derive(Deserialize)]
struct Enrollment {
    token: String,
}
async fn enroll(State(app): State<App>, Json(b): Json<Enrollment>) -> Api<Value> {
    if b.token.len() > 128 {
        return Err(unauthorized());
    }
    let mut c = app.connection();
    let tx = c.transaction()?;
    let agent: String = tx
        .query_row(
            "SELECT agent_id FROM enrollment WHERE hash=? AND expires>?",
            params![digest(&b.token), now()],
            |r| r.get(0),
        )
        .optional()?
        .ok_or_else(unauthorized)?;
    let token = secret(32);
    tx.execute("DELETE FROM enrollment WHERE agent_id=?", [&agent])?;
    tx.execute(
        "UPDATE agents SET token_hash=?,revoked=0 WHERE id=?",
        params![digest(&token), agent],
    )?;
    tx.commit()?;
    Ok(Json(json!({"id":agent,"token":token})))
}
fn agent_auth(app: &App, token_hash: &str) -> Result<Option<String>> {
    Ok(app
        .connection()
        .query_row(
            "SELECT id FROM agents WHERE token_hash=? AND (revoked=0 OR deletion_id!='')",
            [token_hash],
            |r| r.get(0),
        )
        .optional()?)
}
async fn connect(
    State(app): State<App>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> std::result::Result<Response, ApiError> {
    let bearer = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .ok_or_else(unauthorized)?;
    if bearer.len() > 128 {
        return Err(unauthorized());
    }
    let hash = digest(bearer);
    let id = agent_auth(&app, &hash)?.ok_or_else(unauthorized)?;
    Ok(ws
        .max_message_size(MAX_FRAME)
        .max_frame_size(MAX_FRAME)
        .on_upgrade(move |socket| agent_socket(app, socket, id, hash)))
}

#[derive(Deserialize)]
struct RemovalResult { request_id: String, error: Option<String> }
async fn removal_result(State(app): State<App>, headers: HeaderMap, Json(result): Json<RemovalResult>) -> Api<Value> {
    let token = headers.get(header::AUTHORIZATION).and_then(|v| v.to_str().ok()).and_then(|v| v.strip_prefix("Bearer ")).filter(|v| v.len() <= 128).ok_or_else(unauthorized)?;
    let hash = digest(token);
    let mut c = app.connection();
    let tx = c.transaction()?;
    let completed: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM removal_receipts WHERE hash=? AND request_id=? AND expires>?)", params![hash,result.request_id,now()], |row| row.get(0))?;
    if completed { return Ok(Json(json!({"ok":true}))); }
    let agent: String = tx.query_row("SELECT id FROM agents WHERE token_hash=? AND deletion_id=? AND deletion_id!=''", params![hash,result.request_id], |row| row.get(0)).optional()?.ok_or_else(unauthorized)?;
    if let Some(error) = result.error {
        tx.execute("UPDATE agents SET error=? WHERE id=?", params![format!("清理失败：{}", error.chars().take(2000).collect::<String>()),agent])?;
    } else {
        tx.execute("DELETE FROM agents WHERE id=?", [&agent])?;
        tx.execute("INSERT INTO removal_receipts VALUES(?,?,?)", params![hash,result.request_id,now()+7*86_400_000])?;
    }
    tx.commit()?;
    app.logs_changed();
    app.commands_changed();
    Ok(Json(json!({"ok":true})))
}
async fn send(socket: &mut WebSocket, v: ControllerMessage) -> Result<()> {
    socket
        .send(Message::Text(serde_json::to_string(&v)?.into()))
        .await?;
    Ok(())
}
pub struct ConfigRequest {
    pub core: String,
    pub reply: tokio::sync::oneshot::Sender<std::result::Result<Value, String>>,
}
async fn read_config(State(app): State<App>, Path((agent, core)): Path<(String, String)>) -> Api<Value> {
    ensure_mutable(&app.connection(), &agent)?;
    if !CORES.contains(&core.as_str()) {
        return Err(ApiError(StatusCode::BAD_REQUEST, "核心名称无效".into()));
    }
    {
        let c = app.connection();
        let seen = c.query_row("SELECT last_seen FROM agents WHERE id=? AND revoked=0 AND first_seen>0", [&agent], |r| r.get::<_, i64>(0)).optional()?.ok_or_else(not_found)?;
        if seen < now() - 35_000 {
            return Err(ApiError(StatusCode::CONFLICT, "服务器已离线，连接后可查看配置".into()));
        }
    }
    let sender = app.config_readers.lock().unwrap().get(&agent).map(|(_, sender)| sender.clone())
        .ok_or_else(|| ApiError(StatusCode::CONFLICT, "请先更新该服务器的 agent 后查看配置".into()))?;
    let (reply, received) = tokio::sync::oneshot::channel();
    sender.try_send(ConfigRequest { core: core.clone(), reply })
        .map_err(|_| ApiError(StatusCode::CONFLICT, "正在读取配置，请稍后重试".into()))?;
    let content = tokio::time::timeout(std::time::Duration::from_secs(10), received).await
        .map_err(|_| ApiError(StatusCode::GATEWAY_TIMEOUT, "读取配置超时，请检查服务器连接".into()))?
        .map_err(|_| ApiError(StatusCode::CONFLICT, "服务器连接已断开".into()))?
        .map_err(|message| ApiError(StatusCode::BAD_REQUEST, message))?;
    Ok(Json(json!({"filename":format!("{core}.json"),"content":content})))
}
async fn agent_socket(app: App, mut socket: WebSocket, agent: String, hash: String) {
    let fence = id();
    app.sockets
        .lock()
        .unwrap()
        .insert(agent.clone(), fence.clone());
    let mut ticker = tokio::time::interval(std::time::Duration::from_secs(2));
    let mut commands = app.command_changes.subscribe();
    let mut sent = -1;
    let mut greeted = false;
    let mut can_remove = false;
    let mut heard = now();
    let mut update_enabled = false;
    let mut update_sent = 0;
    let (config_tx, mut config_rx) = tokio::sync::mpsc::channel::<ConfigRequest>(4);
    let mut config_pending = std::collections::HashMap::<String, ConfigRequest>::new();
    let result=async {loop {tokio::select! {
        changed=commands.changed()=>{if changed.is_err(){break}ticker.reset_immediately();},
        _=ticker.tick()=>{
            if now()-heard>45_000 || agent_auth(&app,&hash)?.as_deref()!=Some(&agent) || app.sockets.lock().unwrap().get(&agent)!=Some(&fence){break}
            let deletion: String = app.connection().query_row("SELECT deletion_id FROM agents WHERE id=?", [&agent], |row| row.get(0))?;
            if !deletion.is_empty() {
                if greeted && can_remove { send(&mut socket,ControllerMessage::RemoveAgent{request_id:deletion}).await?; }
                send(&mut socket,ControllerMessage::Ping).await?;
                continue;
            }
            let (revision,resources,cores)={let c=app.connection();let rev=c.query_row("SELECT revision FROM agents WHERE id=?",[&agent],|r|r.get::<_,i64>(0))?;(rev,desired_resources(&c,&agent)?,db::core_policy(&c,&agent)?)};
            if revision!=sent {send(&mut socket,ControllerMessage::Desired{revision,resources,cores}).await?;sent=revision;}
            if update_enabled {
                let generation = db::meta(&app.connection(), "update_generation")?.and_then(|s|s.parse::<i64>().ok()).unwrap_or(0);
                if generation > update_sent { send(&mut socket,ControllerMessage::CheckUpdate{generation}).await?; update_sent = generation; }
            }
            send(&mut socket,ControllerMessage::Ping).await?;
            config_pending.retain(|_, request| !request.reply.is_closed());
        },
        Some(request)=config_rx.recv()=>{
            if request.reply.is_closed() { continue; }
            if config_pending.len()>=4 { request.reply.send(Err("正在读取配置，请稍后重试".into())).ok(); continue; }
            let request_id=id();
            send(&mut socket,ControllerMessage::ReadConfig{request_id:request_id.clone(),core:request.core.clone()}).await?;
            config_pending.insert(request_id,request);
        },
        message=socket.recv()=>{
            let Some(message)=message else{break};let message=message?;let Message::Text(body)=message else{if matches!(message,Message::Close(_)){break}else{continue}};
            heard=now();let msg:AgentMessage=serde_json::from_str(&body)?;
            match msg {
                AgentMessage::Hello{version,platform,ipv4,ipv6,country_code,update_enabled:enabled,update_build,core_management,quota_cycles,component_updates,config_view,cleanup,uninstall,..}=>{
                    greeted=true; can_remove=uninstall;
                    anyhow::ensure!(version.len()<=100 && platform.len()<=200,"agent metadata too large");
                    let changed = crate::addresses::report(&mut app.connection(), &agent, ipv4, ipv6)?;
                    if changed { app.commands_changed(); }
                    let country=country_code.filter(|s|s.len()==2 && s.bytes().all(|b|b.is_ascii_uppercase()) && s!="XX").unwrap_or_default();
                    app.connection().execute("UPDATE agents SET version=?,platform=?,country_code=CASE WHEN ?='' THEN country_code ELSE ? END,last_seen=?5,first_seen=CASE WHEN first_seen=0 THEN ?5 ELSE first_seen END WHERE id=?6",params![version,platform,country,country,now(),agent])?;
                    update_enabled = enabled;
                    let build = update_build.filter(|s|release::valid_hash(s)).unwrap_or_default();
                    app.connection().execute("UPDATE agents SET update_enabled=?,update_build=? WHERE id=?",params![enabled,build,agent])?;
                    app.connection().execute("UPDATE agents SET core_management=? WHERE id=?",params![core_management,agent])?;
                    app.connection().execute("UPDATE agents SET quota_cycles=? WHERE id=?",params![quota_cycles,agent])?;
                    app.connection().execute("UPDATE agents SET component_updates=? WHERE id=?",params![component_updates,agent])?;
                    app.connection().execute("UPDATE agents SET cleanup_supported=?,uninstall_supported=? WHERE id=?",params![cleanup,uninstall,agent])?;
                    if !uninstall { app.connection().execute("UPDATE agents SET error='当前 agent 的安装方式不支持远程卸载，请更新或按安装器重新接入' WHERE id=? AND deletion_id!=''",[&agent])?; }
                    ticker.reset_immediately();
                    if config_view { app.config_readers.lock().unwrap().insert(agent.clone(),(fence.clone(),config_tx.clone())); }
                },
                AgentMessage::Addresses{ipv4,ipv6,country_code}=>{
                    anyhow::ensure!(greeted,"agent must greet before reporting addresses");
                    if let Some(code)=country_code.filter(|s| s.len()==2 && s.bytes().all(|b|b.is_ascii_uppercase()) && s!="XX") {
                        app.connection().execute("UPDATE agents SET country_code=? WHERE id=?",params![code,agent])?;
                    }
                    if crate::addresses::report(&mut app.connection(), &agent, ipv4, ipv6)? {
                        app.commands_changed();
                        ticker.reset_immediately();
                    }
                },
                AgentMessage::Config{request_id,core,content,error}=>{
                    if let Some(request)=config_pending.remove(&request_id) {
                        let result=if request.core!=core { Err("配置响应不匹配".into()) }
                            else if let Some(error)=error { Err(error.chars().take(500).collect()) }
                            else { content.filter(Value::is_object).ok_or_else(|| "配置内容无效".into()) };
                        request.reply.send(result).ok();
                    }
                },
                AgentMessage::UpdateState{generation,state,error}=>{
                    anyhow::ensure!(["checking","restarting","up_to_date","failed"].contains(&state.as_str()),"invalid update state");
                    let wanted = db::meta(&app.connection(),"update_generation")?.and_then(|s|s.parse::<i64>().ok()).unwrap_or(0);
                    if generation == wanted && wanted > 0 {
                        let error=error.unwrap_or_default().chars().take(2000).collect::<String>();
                        app.connection().execute("UPDATE agents SET update_state=?,update_error=? WHERE id=?",params![state,error,agent])?;
                    }
                },
                AgentMessage::Applied{revision,ok,error}=>{let mut c=app.connection();let tx=c.transaction()?;let error=error.map(|e|e.chars().take(4000).collect::<String>());if ok{tx.execute("UPDATE agents SET applied=?,error=NULL WHERE id=? AND revision=?",params![revision,agent,revision])?;}else{tx.execute("UPDATE agents SET error=? WHERE id=? AND revision=?",params![error,agent,revision])?;}tx.execute("UPDATE jobs SET state=?,error=?,finished_at=? WHERE agent_id=? AND revision=? AND state='pending'",params![if ok{"applied"}else{"failed"},error,now(),agent,revision])?;tx.commit()?;},
                AgentMessage::Report{report}=>{app.ingest_report(&agent,&report)?;send(&mut socket,ControllerMessage::Receipt{id:report.id}).await?;},
                AgentMessage::Pong=>{app.connection().execute("UPDATE agents SET last_seen=? WHERE id=?",params![now(),agent])?;}
            }
        }
    }} Ok::<_,anyhow::Error>(())}.await;
    if let Err(e) = result {
        tracing::warn!(agent=%agent,error=%e,"子 agent 会话结束");
    }
    let mut readers = app.config_readers.lock().unwrap();
    if readers.get(&agent).is_some_and(|(current, _)| current == &fence) { readers.remove(&agent); }
    drop(readers);
    let mut fences = app.sockets.lock().unwrap();
    if fences.get(&agent) == Some(&fence) {
        fences.remove(&agent);
        app.connection()
            .execute("UPDATE agents SET last_seen=0 WHERE id=?", [&agent])
            .ok();
    }
}

#[derive(rust_embed::RustEmbed)]
#[folder = "../../web/dist/"]
struct Assets;
async fn download_file(State(app): State<App>, Path(file): Path<String>) -> Response {
    let valid = hproxy_common::release::PLATFORMS
        .iter()
        .any(|platform| {
            [
                format!("manifest-{platform}.json"),
                format!("SHA256SUMS-{platform}"),
                format!("hproxy-agent-{platform}"),
                format!("sing-box-{platform}"),
                format!("realm-{platform}"),
            ]
            .contains(&file)
        });
    if !valid {
        return StatusCode::NOT_FOUND.into_response();
    }
    let base = if app.downloads.join("current").is_dir() {
        app.downloads.join("current")
    } else {
        app.downloads.clone()
    };
    match tokio::fs::File::open(base.join(&file)).await {
        Ok(file) => {
            let size = file.metadata().await.map(|m| m.len()).unwrap_or(0);
            Response::builder()
                .header(header::CONTENT_TYPE, "application/octet-stream")
                .header(header::CONTENT_LENGTH, size)
                .body(Body::from_stream(tokio_util::io::ReaderStream::new(file)))
                .unwrap()
        }
        Err(_) => (
            StatusCode::NOT_FOUND,
            "主控尚未准备此架构的安装包，请部署完整发布包中的 downloads 目录。",
        )
            .into_response(),
    }
}
async fn static_file(uri: axum::http::Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    if path.starts_with("api/") || path.starts_with("agent/") || path.starts_with("downloads/") {
        return StatusCode::NOT_FOUND.into_response();
    }
    let path = if path.is_empty() { "index.html" } else { path };
    match Assets::get(path).or_else(|| {
        if !path.contains('.') {
            Assets::get("index.html")
        } else {
            None
        }
    }) {
        Some(file) => {
            let mime = mime_guess::from_path(path).first_or_octet_stream();
            Response::builder()
                .header(
                    header::CONTENT_TYPE,
                    if path.contains('.') {
                        mime.as_ref()
                    } else {
                        "text/html; charset=utf-8"
                    },
                )
                .body(Body::from(file.data.into_owned()))
                .unwrap()
        }
        None => (
            StatusCode::NOT_FOUND,
            "请先在 web 目录执行 npm ci && npm run build，再编译主控。",
        )
            .into_response(),
    }
}
