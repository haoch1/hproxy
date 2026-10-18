mod addresses;
mod cores;
mod config_view;
mod migration;
mod telemetry;
mod update;
mod resource_cleanup;
mod uninstall;
#[cfg(test)]
mod report_tests;
use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use cores::{Desired, Manager};
use futures_util::{SinkExt, StreamExt};
use hproxy_common::*;
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
use tokio::sync::{mpsc, watch};
use tokio_tungstenite::{
    connect_async_tls_with_config,
    tungstenite::{client::IntoClientRequest, Message},
    Connector,
};
const DEFAULT_DATA: &str = if cfg!(target_os = "linux") { hproxy_common::maintenance::AGENT_DATA } else { "./agent-data" };
const DEFAULT_CORES: &str = if cfg!(target_os = "linux") { "/opt/hproxy/agent/current" } else { "./cores" };

#[derive(Parser)]
#[command(version, about = "hproxy 轻量子 agent")]
struct Cli {
    #[arg(long, global = true)]
    log_dir: Option<PathBuf>,
    #[command(subcommand)]
    command: Commands,
}
#[derive(Subcommand)]
enum Commands {
    #[command(hide = true)]
    BackupData { #[arg(long)] data: PathBuf, #[arg(long)] identity: Option<PathBuf> },
    CleanupAgent { #[arg(long)] ticket: PathBuf },
    Enroll {
        #[arg(long)]
        controller: String,
        #[arg(long)]
        token_file: PathBuf,
        #[arg(long, default_value = DEFAULT_DATA)]
        data: PathBuf,
        #[arg(long)]
        ca: Option<PathBuf>,
        #[arg(long)]
        allow_local_http: bool,
    },
    Run {
        #[arg(long, default_value = DEFAULT_DATA)]
        data: PathBuf,
        #[arg(long, default_value = DEFAULT_CORES)]
        core_dir: PathBuf,
        #[arg(long)]
        update_root: Option<PathBuf>,
    },
    Check {
        #[arg(long, default_value = DEFAULT_DATA)]
        data: PathBuf,
        #[arg(long, default_value = DEFAULT_CORES)]
        core_dir: PathBuf,
    },
    Recover {
        #[arg(long)]
        data: PathBuf,
        #[arg(long)]
        update_root: PathBuf,
        #[arg(long)]
        code: String,
    },
    InstallCore {
        #[arg(long)]
        core: String,
        #[arg(long)]
        file: PathBuf,
        #[arg(long)]
        sha256: String,
        #[arg(long, default_value = DEFAULT_CORES)]
        core_dir: PathBuf,
    },
    Import {
        #[arg(long)]
        agent_id: String,
        #[arg(long)]
        sing_box: Option<PathBuf>,
        #[arg(long)]
        nodes: Option<PathBuf>,
        #[arg(long)]
        realm: Option<PathBuf>,
        #[arg(long)]
        output: PathBuf,
    },
}
#[derive(Clone, Serialize, Deserialize)]
struct Config {
    id: String,
    token: String,
    controller: String,
    ca_pem: Option<String>,
    allow_local_http: bool,
}
fn private_dir(path: &Path) -> Result<()> {
    std::fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}
fn validate_controller(controller: &str, allow_local: bool) -> Result<url::Url> {
    let url = url::Url::parse(controller)?;
    anyhow::ensure!(
        url.username().is_empty()
            && url.password().is_none()
            && url.query().is_none()
            && url.fragment().is_none()
            && url.path() == "/",
        "主控地址只能包含 HTTPS 源地址"
    );
    let local = matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "[::1]"));
    anyhow::ensure!(
        url.scheme() == "https" || (allow_local && local && url.scheme() == "http"),
        "主控需要 HTTPS；本机调试可显式使用 --allow-local-http"
    );
    Ok(url)
}
fn tls(ca: Option<&str>) -> Result<rustls::ClientConfig> {
    let mut roots = rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    if let Some(pem) = ca {
        let mut reader = std::io::BufReader::new(pem.as_bytes());
        for cert in rustls_pemfile::certs(&mut reader) {
            roots.add(cert?)?;
        }
    }
    Ok(rustls::ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth())
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    rustls::crypto::ring::default_provider()
        .install_default()
        .ok();
    let cli = Cli::parse();
    let writer = match &cli.log_dir {
        Some(directory) => {
            let log = hproxy_common::rolling_log::RollingLog::new(directory, "hproxy-agent.log")?;
            tracing_subscriber::fmt::writer::BoxMakeWriter::new(move || log.clone())
        },
        None => tracing_subscriber::fmt::writer::BoxMakeWriter::new(std::io::stdout),
    };
    tracing_subscriber::fmt().with_ansi(false).with_writer(writer)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "hproxy_agent=info".into()),
        )
        .init();
    let result = execute(cli.command).await;
    if let Err(error) = &result { tracing::error!(error=%error,"子 agent 命令执行失败"); }
    result
}
async fn execute(command: Commands) -> Result<()> {
    match command {
        Commands::BackupData { data, identity } => println!("{}", uninstall::backup_for_rebind(&data, identity.as_deref())?.display()),
        Commands::CleanupAgent { ticket } => uninstall::execute(ticket).await?,
        Commands::Enroll {
            controller,
            token_file,
            data,
            ca,
            allow_local_http,
        } => {
            validate_controller(&controller, allow_local_http)?;
            private_dir(&data)?;
            anyhow::ensure!(
                !data.join("agent.json").exists(),
                "已经接入主控；重新接入前请停止服务并保存、移走旧 agent.json"
            );
            let pem = ca.map(std::fs::read_to_string).transpose()?;
            let mut client = reqwest::Client::builder().timeout(std::time::Duration::from_secs(20));
            if let Some(pem) = &pem {
                client =
                    client.add_root_certificate(reqwest::Certificate::from_pem(pem.as_bytes())?);
            }
            let token = std::fs::read_to_string(&token_file)?.trim().to_owned();
            let response = client
                .build()?
                .post(format!(
                    "{}/agent/v1/enroll",
                    controller.trim_end_matches('/')
                ))
                .json(&serde_json::json!({"token":token}))
                .send()
                .await?;
            anyhow::ensure!(
                response.status().is_success(),
                "接入被主控拒绝，请检查令牌是否有效、是否已被使用"
            );
            #[derive(Deserialize)]
            struct Identity {
                id: String,
                token: String,
            }
            let identity: Identity = response.json().await?;
            atomic_json(
                &data.join("agent.json"),
                &Config {
                    id: identity.id,
                    token: identity.token,
                    controller: controller.trim_end_matches('/').into(),
                    ca_pem: pem,
                    allow_local_http,
                },
            )?;
            tracing::info!("接入成功；令牌文件可删除，现在启动 run 子命令");
        }
        Commands::Run {
            data,
            core_dir,
            update_root,
        } => {
            if let Some(switch) = run(data.clone(), core_dir, update_root).await? {
                switch.activate(&data)?;
                std::process::exit(75);
            }
        }
        Commands::Recover {
            data,
            update_root,
            code,
        } => update::recover(&data, &update_root, &code)?,
        Commands::Check { data, core_dir } => {
            let shared = Arc::new(Mutex::new(telemetry::load_collector(
                &data.join("counters.json"),
            )?));
            let manager = Manager::new(data, core_dir, shared)?;
            manager.check().await?;
        }
        Commands::InstallCore {
            core,
            file,
            sha256,
            core_dir,
        } => {
            anyhow::ensure!(
                ["sing-box", "realm"].contains(&core.as_str()),
                "核心只能是 sing-box 或 realm"
            );
            anyhow::ensure!(sha256.len() == 64, "SHA256 须为 64 位十六进制");
            use sha2::Digest;
            let content = std::fs::read(&file)?;
            anyhow::ensure!(
                hex::encode(sha2::Sha256::digest(&content)).eq_ignore_ascii_case(&sha256),
                "SHA256 不匹配，已拒绝安装"
            );
            private_dir(&core_dir)?;
            let target = core_dir.join(format!("{core}{}", std::env::consts::EXE_SUFFIX));
            let next =
                target.with_file_name(format!("{core}.next{}", std::env::consts::EXE_SUFFIX));
            std::fs::write(&next, &content)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&next, std::fs::Permissions::from_mode(0o700))?;
            }
            let version = tokio::time::timeout(
                std::time::Duration::from_secs(10),
                tokio::process::Command::new(&next)
                    .arg(if core == "realm" {
                        "--version"
                    } else {
                        "version"
                    })
                    .output(),
            )
            .await??;
            let expected = hproxy_common::release::core_version(&core_dir, &core)?;
            anyhow::ensure!(
                version.status.success()
                    && String::from_utf8_lossy(&version.stdout).contains(&expected),
                "核心版本不匹配"
            );
            if target.exists() {
                let backup = target.with_extension("previous");
                if backup.exists() {
                    std::fs::remove_file(&backup)?;
                }
                std::fs::rename(&target, &backup)?;
            }
            std::fs::rename(&next, &target)?;
            tracing::info!(%core,"核心安装完成；配置下次应用时生效");
        }
        Commands::Import {
            agent_id,
            sing_box,
            nodes,
            realm,
            output,
        } => migration::legacy_import(&agent_id, sing_box, nodes, realm, output)?,
    }
    Ok(())
}
fn spool_files(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut files = std::fs::read_dir(dir)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|s| s == "json"))
        .collect::<Vec<_>>();
    files.sort();
    Ok(files)
}
fn spool(dir: &Path, report: &Report, shared: &telemetry::Shared) -> Result<()> {
    let path = dir.join(format!("{:013}-{}.json", report.at, report.id));
    atomic_json(&path, report)?;
    let files = spool_files(dir)?;
    let mut total = files
        .iter()
        .filter_map(|p| p.metadata().ok())
        .map(|m| m.len())
        .sum::<u64>();
    for file in files {
        if total <= 16 * 1024 * 1024 {
            break;
        }
        let len = file.metadata()?.len();
        let lost: Report = serde_json::from_slice(&std::fs::read(&file)?)?;
        std::fs::remove_file(&file)?;
        total = total.saturating_sub(len);
        shared.lock().unwrap().dropped += lost.logs.len() as u64;
    }
    Ok(())
}

fn queue_report(data: &Path, shared: &telemetry::Shared) -> Result<()> {
    let mut collector = shared.lock().unwrap();
    collector.refresh_quotas(now());
    atomic_json(&data.join("counters.json"), &*collector)?;
    let report = collector.snapshot();
    drop(collector);
    if let Err(error) = spool(&data.join("spool"), &report, shared) {
        // Preserve unsent details on a temporary disk error. Newer in-memory
        // events win; totals are cumulative and replay remains idempotent.
        let mut collector = shared.lock().unwrap();
        for log in report.logs {
            if collector.pending.contains_key(&log.id) { continue; }
            if collector.pending.len() < 1000 {
                collector.pending.insert(log.id.clone(), log);
            } else {
                collector.dropped += 1;
            }
        }
        collector.log_changed.notify_one();
        return Err(error);
    }
    Ok(())
}

async fn report_loop(
    data: PathBuf,
    shared: telemetry::Shared,
    ready: Arc<tokio::sync::Notify>,
    mut stop: watch::Receiver<bool>,
) {
    let changed = shared.lock().unwrap().log_changed.clone();
    let mut tick = tokio::time::interval(std::time::Duration::from_secs(5));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            _ = tick.tick() => {},
            _ = changed.notified() => {
                // Microbatch bursts; traffic-only and core health reports keep
                // their slower idle cadence rather than polling every frame.
                tokio::select! {
                    _ = tokio::time::sleep(std::time::Duration::from_millis(100)) => {},
                    _ = stop.changed() => break,
                }
                if shared.lock().unwrap().pending.is_empty() { continue; }
            }
            _ = stop.changed() => break,
        }
        match queue_report(&data, &shared) {
            Ok(()) => ready.notify_one(),
            Err(error) => {
                tracing::error!(%error, "写入流量检查点/离线队列失败");
                tokio::select! {
                    _ = tokio::time::sleep(std::time::Duration::from_secs(1)) => {},
                    _ = stop.changed() => break,
                }
            }
        }
    }
}

fn queued_report(data: &Path) -> Result<Option<(String, Message)>> {
    let Some(file) = spool_files(&data.join("spool"))?.into_iter().next() else { return Ok(None); };
    let bytes = std::fs::read(file)?;
    anyhow::ensure!(bytes.len() < MAX_FRAME, "离线报告超出限制");
    let report: Report = serde_json::from_slice(&bytes)?;
    let id = report.id.clone();
    Ok(Some((id, Message::Text(serde_json::to_string(&AgentMessage::Report { report })?.into()))))
}
async fn run(
    data: PathBuf,
    core_dir: PathBuf,
    update_root: Option<PathBuf>,
) -> Result<Option<update::Switch>> {
    let data = std::path::absolute(data)?;
    let core_dir = std::path::absolute(core_dir)?;
    private_dir(&data)?;
    private_dir(&data.join("spool"))?;
    let lock = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(data.join("run.lock"))?;
    fs2::FileExt::try_lock_exclusive(&lock).context("已有 agent 使用此数据目录")?;
    let config: Config = serde_json::from_slice(
        &std::fs::read(data.join("agent.json")).context("尚未接入主控，请先 enroll")?,
    )?;
    validate_controller(&config.controller, config.allow_local_http)?;
    let shared = Arc::new(Mutex::new(telemetry::load_collector(
        &data.join("counters.json"),
    )?));
    let removal_plan = uninstall::prepare(&config, &data, &core_dir, update_root.as_deref());
    let can_uninstall = removal_plan.is_some();
    if data.join("removal.json").exists() {
        let request_id: String = serde_json::from_slice(&std::fs::read(data.join("removal.json"))?)?;
        fs2::FileExt::unlock(&lock)?;
        if let Err(error) = uninstall::launch(removal_plan.context("当前安装不支持远程卸载")?, config.clone(), request_id.clone()).await {
            uninstall::report_error(&config,&request_id,&error.to_string()).await;
            return Err(error);
        }
        return Ok(None);
    }
    let (removal_tx, mut removal_rx) = watch::channel::<Option<String>>(None);
    let mut manager = Manager::new(data.clone(), core_dir, shared.clone())?;
    let initial = manager.desired.clone();
    let restored = async {
        manager.reconcile_cores(&config, &initial).await?;
        manager.apply(initial.clone()).await
    }
    .await;
    if let Err(e) = restored {
        manager.error = Some(e.to_string());
        tracing::error!(error=%e,"恢复本地配置失败");
    }
    let (desired_tx, mut desired_rx) = watch::channel(initial.clone());
    let (applied_tx, applied_rx) = mpsc::channel::<AgentMessage>(16);
    let update_reports = applied_tx.clone();
    let update_notify = Arc::new(tokio::sync::Notify::new());
    let update_generation = Arc::new(std::sync::atomic::AtomicI64::new(0));
    let update_enabled = update_root.is_some() && update::platform().is_some();
    let update_build = update_root
        .as_ref()
        .and_then(|root| std::fs::canonicalize(root.join("current")).ok())
        .and_then(|path| {
            path.file_name()
                .map(|name| name.to_string_lossy().into_owned())
        });
    let (stop_tx, stop_rx) = watch::channel(false);
    let (healthy_tx, healthy_rx) = watch::channel(false);
    let mut manager_stop = stop_rx.clone();
    let revision = Arc::new(std::sync::atomic::AtomicI64::new(initial.revision));
    let live_revision = revision.clone();
    let manager_shared = shared.clone();
    let worker_config = config.clone();
    let mut worker = tokio::spawn(async move {
        let mut health = tokio::time::interval(std::time::Duration::from_secs(2));
        loop {
            tokio::select! {
                changed=desired_rx.changed()=>{
                    if changed.is_err(){break}
                    let next=desired_rx.borrow_and_update().clone();
                    let rev=next.revision;
                    let result=async { manager.reconcile_cores(&worker_config,&next).await?; manager.apply(next).await }.await;
                    let error=result.err().map(|e|format!("{e:#}"));
                    manager.error=error.clone();
                    if error.is_none(){live_revision.store(rev,std::sync::atomic::Ordering::Relaxed);}
                    tokio::select! { _=applied_tx.send(AgentMessage::Applied{revision:rev,ok:error.is_none(),error})=>{}, _=manager_stop.changed()=>break }
                },
                _=health.tick()=>{
                    if let Err(e)=manager.maintain().await{tracing::error!(error=%e,"核心检查失败");manager_shared.lock().unwrap().cores=vec![CoreStatus{core:"runtime".into(),state:"error".into(),version:String::new(),error:Some(format!("{e:#}"))}];}
                    if manager.cleanup_pending {
                        match manager.finish_cleanup() {
                            Ok(()) => {
                                manager.error=None;
                                live_revision.store(manager.desired.revision,std::sync::atomic::Ordering::Relaxed);
                                applied_tx.try_send(AgentMessage::Applied{revision:manager.desired.revision,ok:true,error:None}).ok();
                            },
                            Err(error) => { tracing::warn!(%error,"本地配置清理失败，稍后重试"); }
                        }
                    }
                },
                _=manager_stop.changed()=>break,
            }
        }
        manager.shutdown().await;
    });
    let report_ready = Arc::new(tokio::sync::Notify::new());
    let reporter = tokio::spawn(report_loop(data.clone(), shared.clone(), report_ready.clone(), stop_rx.clone()));
    let network = tokio::spawn(network(
        config.clone(),
        data.clone(),
        desired_tx,
        applied_rx,
        revision,
        stop_rx.clone(),
        healthy_tx,
        update_notify.clone(),
        update_generation.clone(),
        update_enabled,
        update_build,
        report_ready,
        removal_tx,
        can_uninstall,
    ));
    let switch = tokio::select! {
        result=shutdown_signal()=>{result?;None},
        switch=update::watch_release(config.clone(),data.clone(),update_root,healthy_rx,update_notify,update_generation,update_reports)=>Some(switch),
        _=removal_rx.changed()=>None,
    };
    let removing = removal_rx.borrow().clone();
    if let Some(request_id) = &removing {
        atomic_json(&data.join("removal.json"), request_id)?;
        shared.lock().unwrap().set_resources(&[]);
    }
    stop_tx.send(true).ok();
    if tokio::time::timeout(std::time::Duration::from_secs(12), &mut worker).await.is_err() {
        worker.abort();
        worker.await.ok();
    }
    reporter.await.ok();
    network.await.ok();
    if removing.is_none() {
        let mut collector = shared.lock().unwrap();
        atomic_json(&data.join("counters.json"), &*collector)?;
        let report = collector.snapshot();
        drop(collector);
        spool(&data.join("spool"), &report, &shared)?;
    }
    fs2::FileExt::unlock(&lock)?;
    if let Some(request_id) = removing {
        if let Err(error) = uninstall::launch(removal_plan.context("当前安装不支持远程卸载")?, config.clone(), request_id.clone()).await {
            uninstall::report_error(&config,&request_id,&error.to_string()).await;
            return Err(error);
        }
    }
    Ok(switch)
}
async fn shutdown_signal() -> Result<()> {
    #[cfg(unix)]
    {
        let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
        tokio::select! {_=tokio::signal::ctrl_c()=>{},_=term.recv()=>{}};
    }
    #[cfg(not(unix))]
    tokio::signal::ctrl_c().await?;
    Ok(())
}
async fn network(
    config: Config,
    data: PathBuf,
    desired: watch::Sender<Desired>,
    mut applied: mpsc::Receiver<AgentMessage>,
    revision: Arc<std::sync::atomic::AtomicI64>,
    mut stop: watch::Receiver<bool>,
    healthy: watch::Sender<bool>,
    update_notify: Arc<tokio::sync::Notify>,
    update_generation: Arc<std::sync::atomic::AtomicI64>,
    update_enabled: bool,
    update_build: Option<String>,
    report_ready: Arc<tokio::sync::Notify>,
    removal: watch::Sender<Option<String>>,
    can_uninstall: bool,
) {
    let (mut address_rx, refresh_addresses, address_task) = addresses::watch(data.clone(), stop.clone());
    let mut backoff = 1u64;
    loop {
        let session=async {
            let url=format!("{}/agent/v1/connect",config.controller.replacen("https://","wss://",1).replacen("http://","ws://",1));
            let mut request=url.into_client_request()?;request.headers_mut().insert("authorization",format!("Bearer {}",config.token).parse()?);
            let connector=Connector::Rustls(Arc::new(tls(config.ca_pem.as_deref())?));
            let (mut socket,_)=tokio::time::timeout(std::time::Duration::from_secs(20),connect_async_tls_with_config(request,None,false,Some(connector))).await??;
            tracing::info!("已连接主控");
            refresh_addresses.notify_one();
            let addresses = address_rx.borrow_and_update().clone();
            socket.send(Message::Text(serde_json::to_string(&AgentMessage::Hello{version:env!("CARGO_PKG_VERSION").into(),platform:format!("{}-{}",std::env::consts::OS,std::env::consts::ARCH),revision:revision.load(std::sync::atomic::Ordering::Relaxed),ipv4:addresses.ipv4.clone(),ipv6:addresses.ipv6.clone(),country_code:addresses.country_code.clone(),update_enabled,update_build:update_build.clone(),core_management:update::platform().is_some(),quota_cycles:true,component_updates:true,config_view:true,cleanup:true,uninstall:can_uninstall})?.into())).await?;
            backoff=1;let mut tick=tokio::time::interval(std::time::Duration::from_secs(1));let mut pending:Option<(String,i64)>=None;let mut heard=now();
            loop{
                // Send on enqueue and immediately after each receipt. The timer
                // only checks liveness/retries; it no longer gates fresh reports.
                if pending.is_none() {
                    if let Some((id, message)) = queued_report(&data)? {
                        socket.send(message).await?;
                        pending = Some((id, now()));
                    }
                }
                tokio::select!{
                _=report_ready.notified()=>{},
                changed=address_rx.changed()=>{
                    changed?;
                    let addresses=address_rx.borrow_and_update().clone();
                    socket.send(Message::Text(serde_json::to_string(&AgentMessage::Addresses{ipv4:addresses.ipv4,ipv6:addresses.ipv6,country_code:addresses.country_code})?.into())).await?;
                },
                _=tick.tick()=>{anyhow::ensure!(now()-heard<45_000,"主控心跳超时");if pending.as_ref().is_some_and(|(_,t)|now()-*t>15_000){pending=None;}},
                msg=applied.recv()=>{if let Some(msg)=msg{if let AgentMessage::Applied{ok,..}=&msg{healthy.send(*ok).ok();}socket.send(Message::Text(serde_json::to_string(&msg)?.into())).await?;}},
                incoming=socket.next()=>{let msg=incoming.context("连接关闭")??;heard=now();if let Message::Text(text)=msg{anyhow::ensure!(text.len()<MAX_FRAME,"主控消息过大");match serde_json::from_str::<ControllerMessage>(&text)?{
                    ControllerMessage::CheckUpdate{generation}=>{if update_enabled && generation>update_generation.load(std::sync::atomic::Ordering::Relaxed){update_generation.store(generation,std::sync::atomic::Ordering::Relaxed);update_notify.notify_one();}},
                    ControllerMessage::ReadConfig{request_id,core}=>{
                        let result=config_view::read(&data,&core);
                        let (content,error)=match result {Ok(value)=>(Some(value),None),Err(error)=>(None,Some(error.to_string()))};
                        socket.send(Message::Text(serde_json::to_string(&AgentMessage::Config{request_id,core,content,error})?.into())).await?;
                    },
                    ControllerMessage::Desired{revision,resources,cores}=>{desired.send(Desired{revision,resources,cores})?;},
                    ControllerMessage::RemoveAgent{request_id}=>{anyhow::ensure!(can_uninstall,"当前安装不支持远程卸载");removal.send(Some(request_id))?;},
                    ControllerMessage::Ping=>{socket.send(Message::Text(serde_json::to_string(&AgentMessage::Pong)?.into())).await?;},
                    ControllerMessage::Receipt{id}=>{if pending.as_ref().is_some_and(|(p,_)|p==&id){for file in spool_files(&data.join("spool"))?{if file.file_stem().and_then(|s|s.to_str()).is_some_and(|s|s.ends_with(&id)){std::fs::remove_file(file)?;break}}pending=None;}},
                }}else if matches!(msg,Message::Close(_)){break}},
                _=stop.changed()=>{socket.close(None).await.ok();return Ok::<_,anyhow::Error>(());},
            }}Ok::<_,anyhow::Error>(())
        }.await;
        if *stop.borrow() {
            break;
        }
        healthy.send(false).ok();
        if let Err(e) = session {
            tracing::warn!(error=%e,retry_seconds=backoff,"主控连接中断；核心继续运行");
        }
        tokio::select! {_=tokio::time::sleep(std::time::Duration::from_secs(backoff))=>{},_=stop.changed()=>break}
        backoff = (backoff * 2).min(60);
    }
    address_task.abort();
    address_task.await.ok();
}
