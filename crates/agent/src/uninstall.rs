//! Remote removal is restricted to a verified installer-owned Linux instance.
//! A separate worker survives stopping the agent service and acknowledges only
//! after its application directories are gone. No shell-built delete commands.
use crate::Config;
use anyhow::{Context, Result};
use hproxy_common::*;
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

#[derive(Clone, Serialize, Deserialize)]
pub struct Plan {
    root: PathBuf,
    data: PathBuf,
    agent_id: String,
    home: Option<PathBuf>,
}
#[derive(Serialize, Deserialize)]
struct Ticket {
    plan: Plan,
    config: Config,
    request_id: String,
}

/// Installer-only rebind snapshot, after the old service has stopped. Taking
/// the runtime lock also rejects orphaned agents whose supervisor disappeared.
pub fn backup_for_rebind(data: &Path, identity: Option<&Path>) -> Result<PathBuf> {
    let data = std::fs::canonicalize(data)?;
    anyhow::ensure!(data.join("agent.json").is_file() && !data.join("hproxy.db").exists(), "此目录不是子 agent 数据目录");
    let lock = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(data.join("run.lock"))?;
    fs2::FileExt::try_lock_exclusive(&lock).context("旧 agent 仍在运行，未移动任何配置")?;
    let same_binding = if let Some(identity) = identity {
        let old: Config = serde_json::from_slice(&std::fs::read(data.join("agent.json"))?)?;
        let new: Config = serde_json::from_slice(&std::fs::read(identity)?)?;
        old.id == new.id && old.controller.trim_end_matches('/') == new.controller.trim_end_matches('/')
    } else { false };
    let backups = data.join("backups");
    maintenance::private_dir(&backups)?;
    let backup = backups.join(format!("rebind-{}-{}", now(), std::process::id()));
    std::fs::create_dir(&backup)?;
    let mut moved: Vec<PathBuf> = vec![];
    let mut restored: Vec<PathBuf> = vec![];
    let result = (|| -> Result<()> {
        let entries = std::fs::read_dir(&data)?.collect::<std::io::Result<Vec<_>>>()?;
        for entry in entries {
            if matches!(entry.file_name().to_str(), Some("backups" | "logs" | "run.lock")) { continue; }
            std::fs::rename(entry.path(), backup.join(entry.file_name()))?;
            moved.push(entry.file_name().into());
        }
        // A token renewal keeps the same cumulative-counter namespace. Starting
        // its counters at zero would hide new traffic below the server's high
        // water mark. A different server/controller must start independently.
        if same_binding {
            for name in ["desired.json", "counters.json", "addresses.json"] {
                let source = backup.join(name);
                if !source.exists() { continue; }
                anyhow::ensure!(std::fs::symlink_metadata(&source)?.file_type().is_file(), "接入数据不能是符号链接");
                restored.push(name.into());
                std::fs::copy(&source, data.join(name))?;
            }
            let spool = backup.join("spool");
            if spool.exists() {
                anyhow::ensure!(std::fs::symlink_metadata(&spool)?.file_type().is_dir(), "上报队列不能是符号链接");
                maintenance::private_dir(&data.join("spool"))?;
                restored.push("spool".into());
                for entry in std::fs::read_dir(&spool)? {
                    let entry = entry?;
                    anyhow::ensure!(entry.file_type()?.is_file(), "上报队列包含异常文件");
                    std::fs::copy(entry.path(), data.join("spool").join(entry.file_name()))?;
                }
            }
        }
        Ok(())
    })();
    if let Err(error) = result {
        for name in restored.iter().rev() { maintenance::remove_child(&data, &data.join(name))?; }
        for name in moved.iter().rev() {
            std::fs::rename(backup.join(name), data.join(name))
                .with_context(|| format!("恢复重新绑定前的配置失败，备份仍在 {}", backup.display()))?;
        }
        std::fs::remove_dir(&backup).ok();
        return Err(error);
    }
    Ok(backup)
}

fn valid_paths(plan: &Plan) -> bool {
    let system = plan.root == Path::new(maintenance::AGENT_ROOT) && plan.data == Path::new(maintenance::AGENT_DATA);
    let user = plan.home.as_ref().is_some_and(|home| {
        home.is_absolute()
            && home.parent().is_some()
            && home != Path::new("/")
            && plan.root == home.join(".local/share/hproxy/agent")
            && plan.data == home.join(".local/state/hproxy/agent")
    });
    system || user
}

fn check_owned(plan: &Plan, config: &Config) -> Result<()> {
    anyhow::ensure!(
        valid_paths(plan) && plan.agent_id == config.id,
        "卸载目录不属于当前 hproxy 实例"
    );
    for path in [&plan.root, &plan.data] {
        if let Ok(meta) = std::fs::symlink_metadata(path) {
            anyhow::ensure!(
                meta.is_dir() && !meta.file_type().is_symlink(),
                "卸载目录不能是符号链接"
            );
            anyhow::ensure!(std::fs::canonicalize(path)? == *path, "卸载目录存在重定向");
        }
    }
    if plan.root.exists() {
        if let Ok(script) = std::fs::read_to_string(plan.root.join("supervise.sh")) {
            anyhow::ensure!(
                script.contains("/hproxy-agent' run --data") && !script.contains("/hproxy-server"),
                "此目录不是独立的子 agent 安装"
            );
        }
        anyhow::ensure!(
            !plan.root.join("current/hproxy-server").exists(),
            "不能卸载主控目录"
        );
    }
    if plan.data.exists() {
        anyhow::ensure!(
            !plan.data.join("hproxy.db").exists(),
            "不能清理主控数据目录"
        );
        if plan.data.join("agent.json").exists() {
            let saved: Config =
                serde_json::from_slice(&std::fs::read(plan.data.join("agent.json"))?)?;
            anyhow::ensure!(
                saved.id == config.id
                    && saved.token == config.token
                    && saved.controller == config.controller,
                "子 agent 身份与清理任务不匹配"
            );
        }
    }
    Ok(())
}

pub fn prepare(config: &Config, data: &Path, core_dir: &Path, root: Option<&Path>) -> Option<Plan> {
    if !cfg!(target_os = "linux") {
        return None;
    }
    let root = std::fs::canonicalize(root?).ok()?;
    if !root.join("supervise.sh").is_file() {
        return None;
    }
    let data = std::fs::canonicalize(data).ok()?;
    let executable = std::fs::canonicalize(std::env::current_exe().ok()?).ok()?;
    let cores = std::fs::canonicalize(core_dir).ok()?;
    if !executable.starts_with(root.join("releases"))
        || executable.parent() != Some(cores.as_path())
    {
        return None;
    }
    let plan = Plan {
        root,
        data,
        agent_id: config.id.clone(),
        home: std::env::var_os("HOME").map(PathBuf::from),
    };
    check_owned(&plan, config).ok()?;
    Some(plan)
}

fn owned_service(path: &Path, plan: &Plan) -> bool {
    std::fs::read_to_string(path).ok().is_some_and(|text| {
        text.contains(&plan.root.join("supervise.sh").to_string_lossy().to_string())
    })
}

pub async fn launch(plan: Plan, config: Config, request_id: String) -> Result<()> {
    anyhow::ensure!(
        cfg!(target_os = "linux"),
        "远程卸载只支持安装器部署的 Linux 子机"
    );
    check_owned(&plan, &config)?;
    anyhow::ensure!(
        request_id.len() == 36
            && request_id
                .bytes()
                .all(|b| b.is_ascii_hexdigit() || b == b'-'),
        "清理任务 ID 无效"
    );
    let systemd = Path::new("/run/systemd/system").is_dir()
        && owned_service(Path::new("/etc/systemd/system/hproxy-agent.service"), &plan);
    // A separate systemd unit survives stopping the agent and escapes its
    // read-only service sandbox. Linux allows unlinking its running executable.
    let name = format!("hproxy-cleanup-{request_id}");
    if systemd
        && tokio::process::Command::new("systemctl")
            .args(["is-active", "--quiet", &format!("{name}.service")])
            .status()
            .await?
            .success()
    {
        return Ok(());
    }
    if plan.data.join("cleanup.lock").exists() {
        let lock = std::fs::OpenOptions::new()
            .write(true)
            .open(plan.data.join("cleanup.lock"))?;
        if fs2::FileExt::try_lock_exclusive(&lock).is_err() {
            return Ok(());
        }
    }
    let executable = std::env::current_exe()?;
    let ticket = plan.data.join("uninstall.json");
    atomic_json(
        &ticket,
        &Ticket {
            plan: plan.clone(),
            config,
            request_id,
        },
    )?;
    if systemd {
        let status = tokio::process::Command::new("systemd-run")
            .args(["--quiet", "--collect", "--property=Type=exec"])
            .arg(format!("--unit={name}"))
            .arg(&executable)
            .args(["cleanup-agent", "--ticket"])
            .arg(&ticket)
            .status()
            .await?;
        anyhow::ensure!(status.success(), "无法启动独立清理服务");
    } else {
        let mut command = std::process::Command::new(&executable);
        command
            .arg("cleanup-agent")
            .arg("--ticket")
            .arg(&ticket)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            unsafe {
                command.pre_exec(|| {
                    if libc::setsid() < 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                    Ok(())
                });
            }
        }
        command.spawn().context("无法启动清理进程")?;
    }
    Ok(())
}

pub async fn report_error(config: &Config, request_id: &str, error: &str) {
    let result = async {
        let mut builder = reqwest::Client::builder().timeout(Duration::from_secs(10));
        if let Some(pem) = &config.ca_pem {
            builder = builder.add_root_certificate(reqwest::Certificate::from_pem(pem.as_bytes())?);
        }
        builder
            .build()?
            .post(format!("{}/agent/v1/removal", config.controller))
            .bearer_auth(&config.token)
            .json(&serde_json::json!({"request_id":request_id,"error":error}))
            .send()
            .await?;
        Ok::<_, anyhow::Error>(())
    }
    .await;
    if let Err(error) = result {
        tracing::warn!(%error,"无法回传清理失败状态");
    }
}

async fn run_service(program: &str, args: &[&str]) -> Result<()> {
    let output = tokio::time::timeout(
        Duration::from_secs(20),
        tokio::process::Command::new(program).args(args).output(),
    )
    .await??;
    anyhow::ensure!(
        output.status.success(),
        "停止 hproxy 服务失败：{}",
        String::from_utf8_lossy(&output.stderr)
            .chars()
            .take(500)
            .collect::<String>()
    );
    Ok(())
}

async fn stop_service(plan: &Plan) -> Result<()> {
    let systemd = Path::new("/etc/systemd/system/hproxy-agent.service");
    let openrc = Path::new("/etc/init.d/hproxy-agent");
    if owned_service(systemd, plan) {
        run_service("systemctl", &["disable", "--now", "hproxy-agent.service"]).await?;
        std::fs::remove_file(systemd)?;
        run_service("systemctl", &["daemon-reload"]).await?;
        return Ok(());
    }
    if owned_service(openrc, plan) {
        run_service("rc-service", &["--ifstarted", "hproxy-agent", "stop"]).await?;
        if std::fs::symlink_metadata("/etc/runlevels/default/hproxy-agent").is_ok() {
            run_service("rc-update", &["del", "hproxy-agent", "default"]).await?;
        }
        std::fs::remove_file(openrc)?;
        return Ok(());
    }
    #[cfg(target_os = "linux")]
    if let Ok(pid) = std::fs::read_to_string(plan.data.join("supervisor.pid")) {
        let pid: i32 = pid.trim().parse()?;
        anyhow::ensure!(pid > 1, "监督进程 PID 无效");
        let command = std::fs::read(format!("/proc/{pid}/cmdline")).unwrap_or_default();
        let expected = plan.root.join("supervise.sh");
        if !command.is_empty() {
            anyhow::ensure!(
                command
                    .split(|b| *b == 0)
                    .any(|arg| arg == expected.to_string_lossy().as_bytes()),
                "监督进程不属于当前安装"
            );
            unsafe {
                libc::kill(pid, libc::SIGTERM);
            }
            for _ in 0..100 {
                if !Path::new(&format!("/proc/{pid}")).exists() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            anyhow::ensure!(
                !Path::new(&format!("/proc/{pid}")).exists(),
                "监督进程尚未停止"
            );
        }
    }
    Ok(())
}

fn remove_owned_dirs(plan: &Plan, config: &Config) -> Result<()> {
    check_owned(plan, config)?;
    // Rebinding backups live inside this agent's data directory. Siblings
    // (especially the controller) are never scanned or removed.
    for path in [&plan.data, &plan.root] {
        if path.exists() {
            std::fs::remove_dir_all(path)
                .with_context(|| format!("清理目录失败：{}", path.display()))?;
        }
    }
    anyhow::ensure!(
        !plan.root.exists() && !plan.data.exists(),
        "子机目录仍有残留"
    );
    Ok(())
}

pub async fn execute(ticket: PathBuf) -> Result<()> {
    anyhow::ensure!(cfg!(target_os = "linux"), "清理进程只支持 Linux");
    let path = ticket;
    let ticket: Ticket = serde_json::from_slice(&std::fs::read(&path)?)?;
    anyhow::ensure!(
        path == ticket.plan.data.join("uninstall.json") && valid_paths(&ticket.plan),
        "清理进程目录不匹配"
    );
    check_owned(&ticket.plan, &ticket.config)?;
    let worker_lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(ticket.plan.data.join("cleanup.lock"))?;
    if fs2::FileExt::try_lock_exclusive(&worker_lock).is_err() {
        return Ok(());
    }
    let mut builder = reqwest::Client::builder().timeout(Duration::from_secs(20));
    if let Some(pem) = &ticket.config.ca_pem {
        builder = builder.add_root_certificate(reqwest::Certificate::from_pem(pem.as_bytes())?);
    }
    let client = builder.build()?;
    let endpoint = format!("{}/agent/v1/removal", ticket.config.controller);
    let mut cleaned = false;
    let mut backoff = 1;
    loop {
        let result = if cleaned {
            Ok(())
        } else {
            async {
                check_owned(&ticket.plan, &ticket.config)?;
                stop_service(&ticket.plan).await?;
                let running = ticket.plan.data.join("run.lock");
                let agent_lock = if running.exists() {
                    Some(std::fs::OpenOptions::new().write(true).open(running)?)
                } else {
                    None
                };
                if let Some(lock) = &agent_lock {
                    fs2::FileExt::try_lock_exclusive(lock)
                        .context("子 agent 仍在运行，稍后重试清理")?;
                }
                remove_owned_dirs(&ticket.plan, &ticket.config)?;
                Ok::<_, anyhow::Error>(())
            }
            .await
        };
        let error = result.err().map(|error| error.to_string());
        if error.is_none() {
            cleaned = true;
        }
        let reported = client
            .post(&endpoint)
            .bearer_auth(&ticket.config.token)
            .json(&serde_json::json!({"request_id":ticket.request_id,"error":error}))
            .send()
            .await;
        if cleaned && reported.is_ok_and(|response| response.status().is_success()) {
            break;
        }
        tokio::time::sleep(Duration::from_secs(backoff)).await;
        backoff = (backoff * 2).min(60);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn renewing_identity_keeps_metering_and_reports_but_a_new_binding_starts_independently() {
        for same_binding in [true, false] {
            let directory = tempfile::tempdir().unwrap();
            let data = directory.path().join("data");
            maintenance::private_dir(&data).unwrap();
            let old = Config { id: hproxy_common::id(), token: "old-token".into(), controller: "https://panel.example.com".into(), ca_pem: None, allow_local_http: false };
            let new = Config { id: if same_binding { old.id.clone() } else { hproxy_common::id() }, token: "new-token".into(), ..old.clone() };
            hproxy_common::atomic_json(&data.join("agent.json"), &old).unwrap();
            let identity = directory.path().join("new-identity.json");
            hproxy_common::atomic_json(&identity, &new).unwrap();
            let rid = hproxy_common::id();
            let counters = serde_json::json!({"counters":{&rid:{"upload":12345,"download":67890}},"quota_baselines":{&rid:{"counter":{"upload":100,"download":200}}}});
            hproxy_common::atomic_json(&data.join("counters.json"), &counters).unwrap();
            hproxy_common::atomic_json(&data.join("desired.json"), &serde_json::json!({"revision":9,"resources":[]})).unwrap();
            maintenance::private_dir(&data.join("spool")).unwrap();
            std::fs::write(data.join("spool/pending.json"), "unacknowledged report").unwrap();
            std::fs::write(data.join("other-binding-state"), "archived only").unwrap();
            let backup = backup_for_rebind(&data, Some(&identity)).unwrap();
            assert_eq!(serde_json::from_slice::<serde_json::Value>(&std::fs::read(backup.join("counters.json")).unwrap()).unwrap(), counters);
            assert!(!data.join("agent.json").exists()); // Installer writes the new token separately.
            assert!(!data.join("other-binding-state").exists());
            if same_binding {
                assert_eq!(serde_json::from_slice::<serde_json::Value>(&std::fs::read(data.join("counters.json")).unwrap()).unwrap(), counters);
                assert_eq!(std::fs::read_to_string(data.join("spool/pending.json")).unwrap(), "unacknowledged report");
                assert!(data.join("desired.json").is_file());
            } else {
                assert!(!data.join("counters.json").exists());
                assert!(!data.join("spool").exists());
            }
        }
    }
    #[cfg(unix)]
    #[test]
    fn unsafe_rebind_report_restores_original_data_without_following_links() {
        let directory = tempfile::tempdir().unwrap();
        let data = directory.path().join("data");
        maintenance::private_dir(&data.join("spool")).unwrap();
        let config = Config { id: hproxy_common::id(), token: "old-token".into(), controller: "https://panel.example.com".into(), ca_pem: None, allow_local_http: false };
        hproxy_common::atomic_json(&data.join("agent.json"), &config).unwrap();
        let identity = directory.path().join("new-identity.json");
        hproxy_common::atomic_json(&identity, &Config { token:"new-token".into(), ..config.clone() }).unwrap();
        std::fs::write(data.join("counters.json"), "preserve metering").unwrap();
        let outside = directory.path().join("outside");
        std::fs::write(&outside, "never touch").unwrap();
        std::os::unix::fs::symlink(&outside, data.join("spool/report.json")).unwrap();
        assert!(backup_for_rebind(&data, Some(&identity)).is_err());
        assert_eq!(std::fs::read_to_string(data.join("counters.json")).unwrap(), "preserve metering");
        assert_eq!(serde_json::from_slice::<Config>(&std::fs::read(data.join("agent.json")).unwrap()).unwrap().token, "old-token");
        assert_eq!(std::fs::read_to_string(outside).unwrap(), "never touch");
    }
    #[test]
    fn rebind_snapshots_require_a_stopped_agent_and_keep_existing_backups_and_logs() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path();
        for name in ["agent.json", "desired.json", "counters.json"] {
            std::fs::write(data.join(name), name).unwrap();
        }
        maintenance::private_dir(&data.join("spool")).unwrap();
        std::fs::write(data.join("spool/report.json"), "queued").unwrap();
        maintenance::private_dir(&data.join("logs")).unwrap();
        std::fs::write(data.join("logs/agent.log"), "log").unwrap();
        maintenance::private_dir(&data.join("backups/rebind-1-1")).unwrap();
        let lock = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(data.join("run.lock")).unwrap();
        fs2::FileExt::try_lock_exclusive(&lock).unwrap();
        assert!(backup_for_rebind(data, None).is_err());
        assert!(data.join("agent.json").exists());
        drop(lock);
        let backup = backup_for_rebind(data, None).unwrap();
        for name in ["agent.json", "desired.json", "counters.json", "spool/report.json"] {
            assert!(!data.join(name).exists() && backup.join(name).exists());
        }
        assert!(data.join("logs/agent.log").exists());
        assert!(data.join("backups/rebind-1-1").exists());
        assert!(data.join("run.lock").exists() && !backup.join("backups").exists());
    }
    #[test]
    fn only_installer_owned_non_controller_paths_are_allowed() {
        let mut plan = Plan {
            root: maintenance::AGENT_ROOT.into(),
            data: maintenance::AGENT_DATA.into(),
            agent_id: "agent".into(),
            home: None,
        };
        assert!(valid_paths(&plan));
        for path in [
            "/",
            "/opt",
            "/opt/hproxy",
            "/opt/hproxy/server",
            "/opt/hproxy/agent/../server",
        ] {
            plan.root = path.into();
            assert!(!valid_paths(&plan));
        }
        let home = PathBuf::from(if cfg!(windows) {
            "C:/Users/cleanup-fixture"
        } else {
            "/home/user"
        });
        plan.home = Some(home.clone());
        plan.root = home.join(".local/share/hproxy/agent");
        plan.data = home.join(".local/state/hproxy/agent");
        assert!(valid_paths(&plan));
        plan.data = home;
        assert!(!valid_paths(&plan));
    }
    #[test]
    fn directory_cleanup_removes_only_this_installation_and_its_replacement_backups() {
        let sandbox = tempfile::tempdir().unwrap();
        let home = std::fs::canonicalize(sandbox.path()).unwrap();
        let plan = Plan {
            root: home.join(".local/share/hproxy/agent"),
            data: home.join(".local/state/hproxy/agent"),
            agent_id: "fixture".into(),
            home: Some(home.clone()),
        };
        let config = Config {
            id: "fixture".into(),
            token: "test".into(),
            controller: "https://fixture.example".into(),
            ca_pem: None,
            allow_local_http: false,
        };
        crate::private_dir(&plan.root).unwrap();
        crate::private_dir(&plan.data).unwrap();
        atomic_json(&plan.data.join("agent.json"), &config).unwrap();
        std::fs::write(plan.root.join("core-fixture"), b"installed core").unwrap();
        let backup = plan.data.join("backups/rebind-1-123");
        crate::private_dir(&backup).unwrap();
        atomic_json(&backup.join("agent.json"), &config).unwrap();
        let outside = home.join("other-application");
        crate::private_dir(&outside).unwrap();
        std::fs::write(outside.join("keep"), b"unrelated").unwrap();
        let server = plan.data.with_file_name("server");
        crate::private_dir(&server).unwrap();
        std::fs::write(server.join("hproxy.db"), b"controller data").unwrap();
        let mut wrong = config.clone();
        wrong.id = "different-agent".into();
        assert!(remove_owned_dirs(&plan, &wrong).is_err());
        assert!(plan.root.exists() && plan.data.exists());
        remove_owned_dirs(&plan, &config).unwrap();
        assert!(!plan.root.exists() && !plan.data.exists() && !backup.exists());
        assert!(outside.join("keep").is_file());
        assert_eq!(std::fs::read(server.join("hproxy.db")).unwrap(), b"controller data");
        remove_owned_dirs(&plan, &config).unwrap(); // Receipt retries are idempotent.
    }
}
