use super::*;
use axum::http::{Method, Request};
use http_body_util::BodyExt;
use std::sync::{Arc, Mutex};
use tower::ServiceExt;
use std::collections::BTreeMap;

pub(super) fn fixture() -> (tempfile::TempDir, App) {
    let dir = tempfile::tempdir().unwrap();
    let db = db::open(&dir.path().join("test.db")).unwrap();
    db.execute(
        "INSERT INTO agents(id,name,core_policy,core_management,cleanup_supported,uninstall_supported,token_hash,last_seen,first_seen) VALUES('00000000-0000-4000-8000-000000000001','test','[]',1,1,1,'installed',1,1)",
        [],
    )
    .unwrap();
    db.execute(
        "INSERT INTO sessions VALUES(?,?,?)",
        params![digest("fixture"), "csrf", now() + 60000],
    )
    .unwrap();
    let app = App {
        db: Arc::new(Mutex::new(db)),
        origin: "https://panel.example".into(),
        secure: true,
        sockets: Default::default(),
        config_readers: Default::default(),
        attempts: Default::default(),
        downloads: dir.path().join("downloads"),
        updates: crate::updates::Service::new(None, None, dir.path().into()).unwrap(),
        log_changes: tokio::sync::watch::channel(0).0,
        command_changes: tokio::sync::watch::channel(0).0,
    };
    (dir, app)
}

async fn event_frame(body: &mut Body) -> String {
    let frame = tokio::time::timeout(std::time::Duration::from_secs(1), body.frame())
        .await.expect("log event should not wait for a polling timer")
        .unwrap().unwrap().into_data().unwrap();
    String::from_utf8(frame.to_vec()).unwrap()
}

#[tokio::test]
async fn live_logs_require_auth_and_publish_only_committed_node_reports() {
    let (_dir, app) = fixture();
    let resource = node();
    store_resource(&app, resource.clone(), true).unwrap();
    let mut forward = node();
    forward.listen_port = 9443;
    forward.spec = Resource::generate("forward").unwrap();
    store_resource(&app, forward.clone(), true).unwrap();
    let get = |cookie: bool, origin: &str| {
        let mut request = Request::builder().uri("/api/v1/logs/events").header("origin", origin);
        if cookie { request = request.header("cookie", "hproxy_session=fixture"); }
        request.body(Body::empty()).unwrap()
    };
    let router = router(app.clone());
    assert_eq!(router.clone().oneshot(get(false, "https://panel.example")).await.unwrap().status(), StatusCode::UNAUTHORIZED);
    assert_eq!(router.clone().oneshot(get(true, "https://other.example")).await.unwrap().status(), StatusCode::BAD_REQUEST);
    let response = router.clone().oneshot(get(true, "https://panel.example")).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["content-type"], "text/event-stream");
    assert_eq!(response.headers()["cache-control"], "no-store");
    assert_eq!(response.headers()["x-accel-buffering"], "no");
    let mut body = response.into_body();
    assert!(event_frame(&mut body).await.contains("event: ready"));
    let report = Report {
        id: id(), at: now(), counters: Default::default(), cores: vec![], dropped_logs: 0, quota_baselines: Default::default(),
        logs: vec![ConnectionLog {
            id: "live-connection".into(), resource_id: resource.id.clone(), network: "tcp".into(),
            source: "client:1".into(), destination: "target:443".into(), domain: "target".into(),
            started_at: now(), closed_at: None, upload: 10, download: 20, result: "active".into(),
        }],
    };
    app.ingest_report(&resource.agent_id, &report).unwrap();
    let event = event_frame(&mut body).await;
    assert!(event.contains("event: logs") && event.contains("data: 1"));
    assert!(!event.contains("client") && !event.contains("target"));
    let page = query_log_page(&app.connection(), &Filter::default()).unwrap();
    assert_eq!(page["rows"][0]["id"], "live-connection");
    // Receipt replays and forbidden/unknown resources must not create live events.
    app.ingest_report(&resource.agent_id, &report).unwrap();
    let mut rejected = report.clone();
    rejected.id = id();
    rejected.logs[0].resource_id = forward.id;
    app.ingest_report(&resource.agent_id, &rejected).unwrap();
    assert_eq!(*app.log_changes.borrow(), 1);
    assert!(tokio::time::timeout(std::time::Duration::from_millis(30), body.frame()).await.is_err());
    let mut closed = report.clone();
    closed.id = id();
    closed.logs[0].closed_at = Some(now());
    closed.logs[0].result = "closed".into();
    closed.logs[0].download = 50;
    app.ingest_report(&resource.agent_id, &closed).unwrap();
    assert!(event_frame(&mut body).await.contains("event: logs"));
    let page = query_log_page(&app.connection(), &Filter::default()).unwrap();
    assert_eq!(page["rows"][0]["download"], 50);
    assert_eq!(page["rows"][0]["result"], "closed");
}

#[tokio::test]
async fn live_logs_coalesce_slow_clients_catch_up_on_reconnect_and_stop_after_logout() {
    let (_dir, app) = fixture();
    let get = || Request::builder().uri("/api/v1/logs/events")
        .header("cookie", "hproxy_session=fixture").body(Body::empty()).unwrap();
    let router = router(app.clone());
    let mut body = router.clone().oneshot(get()).await.unwrap().into_body();
    event_frame(&mut body).await;
    for _ in 0..1000 { app.logs_changed(); }
    let event = event_frame(&mut body).await;
    assert!(event.contains("event: logs") && event.contains("data: 1000"));
    assert!(tokio::time::timeout(std::time::Duration::from_millis(30), body.frame()).await.is_err());
    let mut reconnected = router.clone().oneshot(get()).await.unwrap().into_body();
    let event = event_frame(&mut reconnected).await;
    assert!(event.contains("event: ready") && event.contains("data: 1000"));
    app.connection().execute("DELETE FROM sessions", []).unwrap();
    app.logs_changed();
    assert!(tokio::time::timeout(std::time::Duration::from_secs(1), body.frame()).await.unwrap().is_none());
    assert_eq!(router.oneshot(get()).await.unwrap().status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn shortening_retention_immediately_prunes_by_start_time_and_preserves_totals() {
    let (_dir, app) = fixture();
    let day = 86_400_000;
    {
        let c = app.connection();
        c.execute("INSERT INTO counters VALUES('agent','node',100,200)", []).unwrap();
        c.execute("INSERT INTO traffic VALUES(?,'agent','node',100,200)", [now()]).unwrap();
        for (name, age, closed) in [("old",60,true),("middle",20,false),("recent",2,true),("long-active",40,false)] {
            c.execute("INSERT INTO connections VALUES('agent',?,'node','tcp','source','destination','',?,?,10,20,'active',?)", params![name, now()-age*day, if closed {Some(now())} else {None}, now()]).unwrap();
        }
    }
    for (days, count) in [(90,4),(30,2),(7,1)] {
        let response = router(app.clone()).oneshot(Request::builder().method(Method::PUT).uri("/api/v1/settings")
            .header("cookie","hproxy_session=fixture").header("x-csrf-token","csrf").header("content-type","application/json")
            .body(Body::from(json!({"log_days":days}).to_string())).unwrap()).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let c = app.connection();
        assert_eq!(c.query_row("SELECT COUNT(*) FROM connections", [], |r|r.get::<_,u32>(0)).unwrap(), count);
        assert_eq!(c.query_row("SELECT SUM(upload+download) FROM counters", [], |r|r.get::<_,u64>(0)).unwrap(), 300);
        assert_eq!(c.query_row("SELECT SUM(upload+download) FROM traffic", [], |r|r.get::<_,u64>(0)).unwrap(), 300);
    }
}

#[tokio::test]
async fn config_view_requires_session_online_agent_and_an_allowlisted_core() {
    let (_dir, app) = fixture();
    let agent = "00000000-0000-4000-8000-000000000001";
    let get = |core: &str, authorized: bool| {
        let mut request = Request::builder().uri(format!("/api/v1/agents/{agent}/configs/{core}"));
        if authorized {request=request.header("cookie","hproxy_session=fixture");}
        request.body(Body::empty()).unwrap()
    };
    let router = router(app.clone());
    assert_eq!(router.clone().oneshot(get("sing-box",false)).await.unwrap().status(),StatusCode::UNAUTHORIZED);
    assert_eq!(router.clone().oneshot(get("agent",true)).await.unwrap().status(),StatusCode::BAD_REQUEST);
    assert_eq!(router.clone().oneshot(get("sing-box",true)).await.unwrap().status(),StatusCode::CONFLICT);
    app.connection().execute("UPDATE agents SET last_seen=? WHERE id=?",params![now(),agent]).unwrap();
    assert_eq!(router.clone().oneshot(get("sing-box",true)).await.unwrap().status(),StatusCode::CONFLICT);
    let (sender, mut receiver) = tokio::sync::mpsc::channel::<ConfigRequest>(4);
    app.config_readers.lock().unwrap().insert(agent.into(),("fence".into(),sender));
    let responder=tokio::spawn(async move {
        for _ in 0..2 {
            let request=receiver.recv().await.unwrap();
            request.reply.send(Ok(json!({"core":request.core,"fixture":true}))).unwrap();
        }
    });
    for core in ["sing-box","realm"] {
        let response=router.clone().oneshot(get(core,true)).await.unwrap();
        assert_eq!(response.status(),StatusCode::OK);
        assert_eq!(response.headers().get("cache-control").unwrap(),"no-store");
        let body:Value=serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();
        assert_eq!(body["filename"],format!("{core}.json"));
        assert_eq!(body["content"]["core"],core);
    }
    responder.await.unwrap();
}

#[test]
fn editing_existing_resource_updates_type_index_without_conflict() {
    let (_dir, app) = fixture();
    let mut resource = node();
    store_resource(&app, resource.clone(), true).unwrap();
    resource.name = "edited".into();
    resource.enabled = false;
    store_resource(&app, resource.clone(), false).unwrap();
    resource.spec = Resource::generate("forward").unwrap();
    store_resource(&app, resource.clone(), false).unwrap();
    let stored = db::resources(&app.connection(), None).unwrap();
    assert_eq!(stored, vec![resource.clone()]);
    let kind: String = app
        .connection()
        .query_row(
            "SELECT kind FROM resource_types WHERE id=?",
            [&resource.id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(kind, "forward");
}

#[tokio::test]
async fn uninstalled_servers_stay_hidden_until_first_connection_and_offline_servers_remain() {
    let (_dir, app) = fixture();
    let router = router(app.clone());
    let get = |uri: &'static str| {
        Request::builder()
            .uri(uri)
            .header("cookie", "hproxy_session=fixture")
            .body(Body::empty())
            .unwrap()
    };
    app.connection()
        .execute(
            "INSERT INTO agents(id,name) VALUES('not-installed','new server')",
            [],
        )
        .unwrap();
    let mut pending_resource = node();
    pending_resource.agent_id = "not-installed".into();
    app.connection()
        .execute(
            "INSERT INTO resources VALUES(?,?,?)",
            params![
                pending_resource.id,
                pending_resource.agent_id,
                serde_json::to_string(&pending_resource).unwrap()
            ],
        )
        .unwrap();
    assert!(visible_resources(&app.connection()).unwrap().is_empty());
    for uri in ["/api/v1/agents", "/api/v1/overview"] {
        let response = router.clone().oneshot(get(uri)).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body: Value =
            serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes())
                .unwrap();
        let agents = if uri.ends_with("overview") {
            assert_eq!(body["nodes"], 0);
            &body["agents"]
        } else {
            &body
        };
        assert_eq!(agents.as_array().unwrap().len(), 1);
        assert!(agents
            .as_array()
            .unwrap()
            .iter()
            .all(|a| a["id"] != "not-installed"));
    }
    app.connection()
        .execute(
            "UPDATE agents SET token_hash='new-identity' WHERE id='not-installed'",
            [],
        )
        .unwrap();
    assert_eq!(
        db::list_agents(&app.connection())
            .unwrap()
            .as_array()
            .unwrap()
            .len(),
        1
    );
    for seen in [now(), 0] {
        app.connection()
            .execute(
                "UPDATE agents SET last_seen=?,first_seen=1 WHERE id='not-installed'",
                [seen],
            )
            .unwrap();
        let agents = db::list_agents(&app.connection()).unwrap();
        assert_eq!(agents.as_array().unwrap().len(), 2);
        assert_eq!(agents[1]["id"], "not-installed");
        assert_eq!(visible_resources(&app.connection()).unwrap().len(), 1);
    }
}

pub(super) fn node() -> Resource {
    Resource {
        id: id(),
        agent_id: "00000000-0000-4000-8000-000000000001".into(),
        name: "node".into(),
        enabled: true,
        listen_ip: "0.0.0.0".into(),
        listen_port: 8443,
        address_bindings: Default::default(), public_host: "192.0.2.1".into(),
        quota_bytes: None, quota_cycle: None,
        expires_at: None,
        spec: Resource::generate("vless").unwrap(),
    }
}

#[test]
fn quota_reports_keep_history_and_reject_replays_and_unsupported_agents() {
    let (_dir, app) = fixture();
    let mut resource = node();
    resource.quota_bytes = Some(1000);
    resource.quota_cycle = Some(QuotaCycle { starts_at:now()-60_000, months:1, days:0 });
    assert!(store_resource(&app,resource.clone(),true).unwrap_err().to_string().contains("更新并连接"));
    app.connection().execute("UPDATE agents SET quota_cycles=1",[]).unwrap();
    store_resource(&app,resource.clone(),true).unwrap();
    let total = Counter { upload:400, download:600 };
    let baseline = QuotaBaseline { cycle:resource.quota_cycle.clone().unwrap(), start:resource.quota_cycle.as_ref().unwrap().starts_at,
        counter:Counter { upload:100,download:200 } };
    let mut report = Report { id:id(), at:now(), counters:BTreeMap::from([(resource.id.clone(),total)]),
        quota_baselines:BTreeMap::from([(resource.id.clone(),baseline.clone())]), logs:vec![],cores:vec![],dropped_logs:0 };
    app.ingest_report(&resource.agent_id,&report).unwrap();
    app.ingest_report(&resource.agent_id,&report).unwrap();
    let counters = db::resource_counters(&app.connection(),&[resource.clone()]).unwrap();
    assert_eq!(counters[&resource.id].total(),700);
    assert_eq!(app.connection().query_row("SELECT SUM(upload+download) FROM traffic",[],|r|r.get::<_,u64>(0)).unwrap(),1000);
    report.id=id(); report.at-=1;
    report.quota_baselines.get_mut(&resource.id).unwrap().counter=Counter::default();
    app.ingest_report(&resource.agent_id,&report).unwrap();
    assert_eq!(db::resource_counters(&app.connection(),&[resource.clone()]).unwrap()[&resource.id].total(),700);
    // No cycle means lifetime totals; disabling a schedule doesn't erase history.
    resource.quota_cycle=None;
    assert_eq!(db::resource_counters(&app.connection(),&[resource.clone()]).unwrap()[&resource.id].total(),1000);
    app.connection().execute("DELETE FROM resources WHERE id=?",[&resource.id]).unwrap();
    assert_eq!(app.connection().query_row("SELECT COUNT(*) FROM quota_baselines",[],|r|r.get::<_,u64>(0)).unwrap(),0);
    assert_eq!(app.connection().query_row("SELECT SUM(upload+download) FROM traffic",[],|r|r.get::<_,u64>(0)).unwrap(),1000);
}

#[test]
fn offline_core_operations_preserve_policy_and_revision() {
    let (_dir, app) = fixture();
    let agent = "00000000-0000-4000-8000-000000000001";
    for seen in [0, now() - 36_000] {
        app.connection().execute("UPDATE agents SET last_seen=? WHERE id=?", params![seen,agent]).unwrap();
        for core in CORES {
            for installed in [true, false] {
                assert!(set_core(&app,agent,core,installed).unwrap_err().to_string().contains("服务器离线"));
            }
        }
        assert_eq!(db::core_policy(&app.connection(),agent).unwrap(), Some(vec![]));
        assert_eq!(app.connection().query_row("SELECT revision FROM agents WHERE id=?",[agent],|r|r.get::<_,i64>(0)).unwrap(),1);
    }
    app.connection().execute("UPDATE agents SET last_seen=? WHERE id=?",params![now(),agent]).unwrap();
    set_core(&app,agent,"realm",true).unwrap();
    assert_eq!(db::core_policy(&app.connection(),agent).unwrap(), Some(vec!["realm".into()]));
}

#[test]
fn resource_creation_installs_required_core_and_uninstall_is_guarded() {
    let (_dir, app) = fixture();
    app.connection().execute("UPDATE agents SET last_seen=?", [now()]).unwrap();
    let mut resource = node();
    resource.enabled = false;
    store_resource(&app, resource, true).unwrap();
    assert_eq!(
        db::core_policy(&app.connection(), "00000000-0000-4000-8000-000000000001").unwrap(),
        Some(vec!["sing-box".into()])
    );
    assert!(set_core(
        &app,
        "00000000-0000-4000-8000-000000000001",
        "sing-box",
        false
    )
    .is_err());
    set_core(&app, "00000000-0000-4000-8000-000000000001", "realm", true).unwrap();
    set_core(&app, "00000000-0000-4000-8000-000000000001", "realm", false).unwrap();
    assert_eq!(
        db::core_policy(&app.connection(), "00000000-0000-4000-8000-000000000001").unwrap(),
        Some(vec!["sing-box".into()])
    );
    assert!(set_core(
        &app,
        "00000000-0000-4000-8000-000000000001",
        "../realm",
        true
    )
    .is_err());
    app.connection()
        .execute(
            "UPDATE agents SET core_management=0 WHERE id='00000000-0000-4000-8000-000000000001'",
            [],
        )
        .unwrap();
    assert!(set_core(&app, "00000000-0000-4000-8000-000000000001", "realm", true).is_err());
}

#[tokio::test]
async fn new_servers_persist_core_choices_and_name_edits_preserve_them() {
    let (dir, app) = fixture();
    let router = router(app.clone());
    let request = |method, path: &str, body: Value| Request::builder().method(method).uri(path)
        .header("content-type", "application/json").header("cookie", "hproxy_session=fixture")
        .header("x-csrf-token", "csrf").body(Body::from(body.to_string())).unwrap();
    for choice in [None, Some(json!(["sing-box","realm"])), Some(json!(["sing-box"])), Some(json!(["realm"])), Some(json!([]))] {
        let mut input = json!({"name":"new server"});
        if let Some(cores) = &choice { input["cores"] = cores.clone(); }
        let response = router.clone().oneshot(request(Method::POST,"/api/v1/agents",input)).await.unwrap();
        assert_eq!(response.status(),StatusCode::OK);
        let created: Value = serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();
        let id = created["agent_id"].as_str().unwrap();
        let expected: Vec<String> = serde_json::from_value(choice.unwrap_or(json!(["sing-box","realm"]))).unwrap();
        let reopened = db::open(&dir.path().join("test.db")).unwrap();
        assert_eq!(db::core_policy(&reopened,id).unwrap(),Some(expected.clone()));
        let response = router.clone().oneshot(request(Method::POST,"/agent/v1/enroll",json!({"token":created["token"]}))).await.unwrap();
        assert_eq!(response.status(),StatusCode::OK);
        let response = router.clone().oneshot(request(Method::PUT,&format!("/api/v1/agents/{id}"),json!({"name":"renamed"}))).await.unwrap();
        assert_eq!(response.status(),StatusCode::OK);
        assert_eq!(db::core_policy(&app.connection(),id).unwrap(),Some(expected.clone()));
        // The same stored policy continues to serve the existing manual controls.
        app.connection().execute("UPDATE agents SET core_management=1,last_seen=? WHERE id=?",params![now(),id]).unwrap();
        for core in CORES {
            set_core(&app,id,core,true).unwrap();
            assert!(db::core_policy(&app.connection(),id).unwrap().unwrap().iter().any(|c|c==core));
            set_core(&app,id,core,false).unwrap();
            assert!(!db::core_policy(&app.connection(),id).unwrap().unwrap().iter().any(|c|c==core));
        }
    }
    let count = app.connection().query_row("SELECT COUNT(*) FROM agents",[],|r|r.get::<_,u32>(0)).unwrap();
    for cores in [json!(["singbox"]),json!(["realm","realm"]),json!(["sing-box","realm","realm"])] {
        let response = router.clone().oneshot(request(Method::POST,"/api/v1/agents",json!({"name":"invalid","cores":cores}))).await.unwrap();
        assert_eq!(response.status(),StatusCode::BAD_REQUEST);
    }
    assert_eq!(app.connection().query_row("SELECT COUNT(*) FROM agents",[],|r|r.get::<_,u32>(0)).unwrap(),count);
}

#[tokio::test]
async fn server_switch_preserves_configuration_and_requires_csrf() {
    let (_dir, app) = fixture();
    let resource = node();
    store_resource(&app, resource.clone(), true).unwrap();
    let router = router(app.clone());
    let request = |enabled: bool, csrf: bool| {
        let mut request = Request::builder()
            .method(Method::PUT)
            .uri("/api/v1/agents/00000000-0000-4000-8000-000000000001/state")
            .header("content-type", "application/json")
            .header("cookie", "hproxy_session=fixture")
            .header("origin", "https://panel.example");
        if csrf {
            request = request.header("x-csrf-token", "csrf");
        }
        request
            .body(Body::from(json!({"enabled":enabled}).to_string()))
            .unwrap()
    };
    assert_eq!(
        router
            .clone()
            .oneshot(request(false, false))
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    for enabled in [false, true] {
        let response = router
            .clone()
            .oneshot(request(enabled, true))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body: Value =
            serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes())
                .unwrap();
        assert!(body["revision"].as_i64().unwrap() > 0);
        assert_eq!(
            desired_resources(&app.connection(), "00000000-0000-4000-8000-000000000001").unwrap()
                [0]
            .enabled,
            enabled
        );
        assert_eq!(
            db::resources(
                &app.connection(),
                Some("00000000-0000-4000-8000-000000000001")
            )
            .unwrap()[0],
            resource
        );
        assert_eq!(
            db::list_agents(&app.connection()).unwrap()[0]["enabled"],
            enabled
        );
    }
}
