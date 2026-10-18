mod db;
mod addresses;
mod updates;
mod web;
use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use hproxy_common::now;
const DEFAULT_DATA: &str = if cfg!(target_os = "linux") { hproxy_common::maintenance::SERVER_DATA } else { "./data" };
const DEFAULT_DOWNLOADS: &str = if cfg!(target_os = "linux") { "/opt/hproxy/server/downloads" } else { "./downloads" };
use std::{
    collections::HashMap,
    net::SocketAddr,
    path::PathBuf,
    sync::{Arc, Mutex},
};

#[derive(Parser)]
#[command(version, about = "hproxy 轻量代理主控")]
struct Cli {
    #[arg(long, global = true)]
    log_dir: Option<PathBuf>,
    #[command(subcommand)]
    command: Commands,
}
#[derive(Subcommand)]
enum Commands {
    Run {
        #[arg(long, default_value = DEFAULT_DATA)]
        data: PathBuf,
        #[arg(long, default_value = "127.0.0.1:19080")]
        listen: SocketAddr,
        #[arg(long, default_value = "http://127.0.0.1:19080")]
        public_url: String,
        #[arg(long, requires = "tls_key")]
        tls_cert: Option<PathBuf>,
        #[arg(long, requires = "tls_cert")]
        tls_key: Option<PathBuf>,
        #[arg(long, default_value = DEFAULT_DOWNLOADS)]
        downloads: PathBuf,
        #[arg(long, env = "HPROXY_UPDATE_SOURCE")]
        update_source: Option<String>,
        #[arg(long, requires = "update_source")]
        update_ca: Option<PathBuf>,
        #[arg(long)]
        update_root: Option<PathBuf>,
    },
    SetPassword {
        #[arg(long, default_value = DEFAULT_DATA)]
        data: PathBuf,
        #[arg(long)]
        password_file: PathBuf,
    },
    Backup {
        #[arg(long, default_value = DEFAULT_DATA)]
        data: PathBuf,
        #[arg(long)]
        output: PathBuf,
    },
    Check {
        #[arg(long, default_value = DEFAULT_DATA)]
        data: PathBuf,
    },
    Recover {
        #[arg(long)]
        update_root: PathBuf,
    },
    ReleaseInfo,
}
#[derive(Clone)]
pub struct App {
    pub db: Arc<Mutex<rusqlite::Connection>>,
    pub origin: String,
    pub secure: bool,
    pub sockets: Arc<Mutex<HashMap<String, String>>>,
    pub config_readers: Arc<Mutex<HashMap<String, (String, tokio::sync::mpsc::Sender<web::ConfigRequest>)>>>,
    pub attempts: Arc<Mutex<HashMap<String, (i64, u32)>>>,
    pub downloads: PathBuf,
    pub updates: Arc<updates::Service>,
    pub log_changes: tokio::sync::watch::Sender<u64>,
    pub command_changes: tokio::sync::watch::Sender<u64>,
}
impl App {
    pub fn connection(&self) -> std::sync::MutexGuard<'_, rusqlite::Connection> {
        self.db.lock().unwrap_or_else(|p| p.into_inner())
    }
    pub fn logs_changed(&self) {
        // Retain only the latest revision, even when no browser is subscribed.
        self.log_changes.send_modify(|revision| *revision = revision.wrapping_add(1));
    }
    pub fn commands_changed(&self) {
        self.command_changes.send_modify(|revision| *revision = revision.wrapping_add(1));
    }
    pub fn ingest_report(&self, agent: &str, report: &hproxy_common::Report) -> Result<()> {
        if db::ingest(&mut self.connection(), agent, report)? {
            self.logs_changed();
        }
        Ok(())
    }
}
#[tokio::main]
async fn main() -> Result<()> {
    rustls::crypto::ring::default_provider()
        .install_default()
        .ok();
    let cli = Cli::parse();
    let writer = match &cli.log_dir {
        Some(directory) => {
            let log = hproxy_common::rolling_log::RollingLog::new(directory, "hproxy-server.log")?;
            tracing_subscriber::fmt::writer::BoxMakeWriter::new(move || log.clone())
        },
        None => tracing_subscriber::fmt::writer::BoxMakeWriter::new(std::io::stdout),
    };
    tracing_subscriber::fmt().with_ansi(false).with_writer(writer)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "hproxy_server=info,tower_http=info".into()),
        )
        .init();
    let result = execute(cli).await;
    if let Err(error) = &result { tracing::error!(error=%error,"主控命令执行失败"); }
    result
}
async fn execute(cli: Cli) -> Result<()> {
    if matches!(&cli.command, Commands::ReleaseInfo) {
        println!(
            "{}",
            serde_json::json!({"version":env!("CARGO_PKG_VERSION"),"sing_box":hproxy_common::SING_VERSION,"realm":hproxy_common::REALM_VERSION,"component_updates":true})
        );
        return Ok(());
    }
    if let Commands::Recover { update_root } = &cli.command {
        return updates::recover(update_root);
    }
    let data = match &cli.command {
        Commands::Run { data, .. }
        | Commands::SetPassword { data, .. }
        | Commands::Backup { data, .. }
        | Commands::Check { data } => data,
        Commands::Recover { .. } | Commands::ReleaseInfo => unreachable!(),
    };
    std::fs::create_dir_all(data)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(data, std::fs::Permissions::from_mode(0o700))?;
    }
    let conn = db::open(&data.join("hproxy.db"))?;
    match cli.command {
        Commands::Recover { .. } | Commands::ReleaseInfo => unreachable!(),
        Commands::Check { .. } => {
            let check: String = conn.query_row("PRAGMA quick_check", [], |r| r.get(0))?;
            anyhow::ensure!(
                check == "ok" && db::meta(&conn, "password")?.is_some(),
                "数据库检查失败"
            );
            println!("数据库检查通过");
        }
        Commands::SetPassword { password_file, .. } => {
            let password = std::fs::read_to_string(password_file)?
                .trim_end()
                .to_owned();
            anyhow::ensure!(
                password.len() >= 12 && password.len() <= 1024,
                "密码至少 12 字节、最多 1024 字节"
            );
            let hash = web::hash_password(&password)?;
            conn.execute("INSERT INTO meta VALUES('password',?) ON CONFLICT(key) DO UPDATE SET value=excluded.value",[hash])?;
            conn.execute("DELETE FROM sessions", [])?;
            println!("管理员密码已设置，已有会话已撤销。");
        }
        Commands::Backup { output, .. } => {
            anyhow::ensure!(!output.exists(), "备份文件已经存在");
            conn.backup("main", &output, None).context("备份失败")?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&output, std::fs::Permissions::from_mode(0o600))?;
            }
            println!("备份已保存到 {}", output.display());
        }
        Commands::Run {
            listen,
            public_url,
            tls_cert,
            tls_key,
            downloads,
            update_source,
            update_ca,
            update_root,
            data,
            ..
        } => {
            anyhow::ensure!(
                db::meta(&conn, "password")?.is_some(),
                "请先使用 set-password --password-file 初始化管理员密码"
            );
            let url = url::Url::parse(&public_url)?;
            let secure = url.scheme() == "https";
            anyhow::ensure!(
                secure
                    || (url.scheme() == "http"
                        && matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "[::1]"))),
                "公网地址必须使用 HTTPS；HTTP 仅用于本机预览"
            );
            anyhow::ensure!(
                url.path() == "/" && url.query().is_none() && url.fragment().is_none(),
                "public-url 只能包含站点源地址"
            );
            if !secure {
                anyhow::ensure!(listen.ip().is_loopback(), "HTTP 预览只允许监听回环地址");
            }
            let updates = if let Some(ca) = update_ca {
                updates::Service::with_ca(
                    update_source,
                    update_root,
                    data,
                    Some(std::fs::read(ca)?),
                )?
            } else {
                updates::Service::new(update_source, update_root, data)?
            };
            let app = App {
                db: Arc::new(Mutex::new(conn)),
                origin: url.origin().ascii_serialization(),
                secure,
                sockets: Default::default(),
                config_readers: Default::default(),
                attempts: Default::default(),
                downloads,
                updates: updates.clone(),
                log_changes: tokio::sync::watch::channel(0).0,
                command_changes: tokio::sync::watch::channel(0).0,
            };
            let housekeeper = app.clone();
            tokio::spawn(async move {
                loop {
                    tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
                    if let Err(e) = db::cleanup(&housekeeper.connection()) {
                        tracing::error!(error=%e,"数据库清理失败")
                    };
                    if let Err(e) = housekeeper.updates.maintain(&housekeeper.downloads) {
                        tracing::warn!(error=%e,"后台文件清理失败，稍后重试");
                    }
                    housekeeper
                        .attempts
                        .lock()
                        .unwrap()
                        .retain(|_, (t, _)| now() - *t < 900_000);
                }
            });
            db::cleanup(&app.connection())?;
            updates.auto_check();
            let router = web::router(app.clone());
            tracing::info!(%listen,%public_url,"hproxy 主控启动");
            let serve = async {
                if let (Some(cert), Some(key)) = (tls_cert, tls_key) {
                    let config =
                        axum_server::tls_rustls::RustlsConfig::from_pem_file(cert, key).await?;
                    let listener = std::net::TcpListener::bind(listen)?;
                    updates.mark_healthy(app.clone());
                    axum_server::from_tcp_rustls(listener, config)
                        .serve(router.into_make_service_with_connect_info::<SocketAddr>())
                        .await?;
                } else {
                    let listener = tokio::net::TcpListener::bind(listen).await?;
                    updates.mark_healthy(app.clone());
                    axum::serve(
                        listener,
                        router.into_make_service_with_connect_info::<SocketAddr>(),
                    )
                    .await?;
                }
                Ok::<(), anyhow::Error>(())
            };
            tokio::select! {
                result = serve => result?,
                _ = updates.restart.notified() => {
                    updates.activate()?;
                    std::process::exit(75);
                }
                _ = shutdown() => {}
            }
        }
    }
    Ok(())
}
async fn shutdown() {
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .expect("TERM handler");
        tokio::select! { _ = tokio::signal::ctrl_c() => {}, _ = terminate.recv() => {} }
    }
    #[cfg(not(unix))]
    {
        tokio::signal::ctrl_c().await.ok();
    }
}
