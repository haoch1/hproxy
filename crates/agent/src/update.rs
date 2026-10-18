//! Updates use complete controller-published manifests, including partial component updates.
//! A release directory is complete and checked before its symlink is switched.
use crate::Config;
use anyhow::Result;
use futures_util::StreamExt;
use hproxy_common::{atomic_json, maintenance};
use hproxy_common::AgentMessage;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::{
    atomic::{AtomicI64, Ordering},
    Arc,
};
use std::{
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::sync::{mpsc, Notify};
use tokio::{io::AsyncWriteExt, sync::watch};

pub use hproxy_common::release::{platform, Release};
#[derive(Default, Deserialize, Serialize)]
struct UpdateState {
    pending: Option<Pending>,
    blocked_build: Option<String>,
    #[serde(default)]
    previous: Option<PathBuf>,
}
#[derive(Deserialize, Serialize)]
struct Pending {
    previous: PathBuf,
    build: String,
}
#[cfg_attr(not(unix), allow(dead_code))]
pub struct Switch {
    root: PathBuf,
    next: PathBuf,
    state: UpdateState,
}

fn maintain(data: &Path, root: &Path) -> Result<()> {
    let state: UpdateState = match std::fs::read(data.join("update-state.json")) {
        Ok(bytes) => serde_json::from_slice(&bytes)?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => UpdateState::default(),
        Err(e) => return Err(e.into()),
    };
    if state.pending.is_some() { return Ok(()); }
    let mut keep = vec![maintenance::current_release(root)?];
    if let Some(previous) = state.previous { keep.push(previous); }
    maintenance::prune_releases(root, &keep)?;
    for release in &keep { maintenance::prune_temporary_files(release)?; }
    maintenance::prune_backups(data)?;
    maintenance::prune_temporary_files(data)?;
    Ok(())
}
fn maintain_quietly(data: &Path, root: &Path) {
    if let Err(error) = maintain(data, root) {
        tracing::warn!(%error,"本地文件清理失败，稍后重试");
    }
}

fn client(config: &Config) -> Result<reqwest::Client> {
    let mut b = reqwest::Client::builder().timeout(Duration::from_secs(300));
    if let Some(pem) = &config.ca_pem {
        b = b.add_root_certificate(reqwest::Certificate::from_pem(pem.as_bytes())?);
    }
    Ok(b.build()?)
}
pub async fn install_core(config: &Config, directory: &Path, core: &str) -> Result<()> {
    use anyhow::Context;
    anyhow::ensure!(hproxy_common::CORES.contains(&core), "核心名称无效");
    let platform = platform().context("当前系统不支持自动安装核心")?;
    let client = client(config)?;
    let base = format!("{}/downloads", config.controller);
    let response = client
        .get(format!("{base}/manifest-{platform}.json"))
        .timeout(Duration::from_secs(15))
        .send()
        .await?
        .error_for_status()?;
    anyhow::ensure!(
        response.content_length().unwrap_or(0) < 16384,
        "核心清单过大"
    );
    let body = response.bytes().await?;
    anyhow::ensure!(body.len() < 16384, "核心清单过大");
    let release: Release = serde_json::from_slice(&body)?;
    release.validate(platform)?;
    let artifact = if core == "realm" {
        &release.realm
    } else {
        &release.sing_box
    };
    anyhow::ensure!(
        artifact.version == hproxy_common::release::core_version(directory, core)?,
        "主控核心包与当前 agent 不兼容，请先更新 agent"
    );
    crate::private_dir(directory)?;
    let part = directory.join(format!("{core}.install.part"));
    let result = async {
        let response = client
            .get(format!("{base}/{}", artifact.file))
            .send()
            .await?
            .error_for_status()?;
        let mut stream = response.bytes_stream();
        let mut file = tokio::fs::File::create(&part).await?;
        let mut digest = Sha256::new();
        let mut size = 0u64;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            size += chunk.len() as u64;
            anyhow::ensure!(size <= artifact.bytes, "核心文件大于清单声明");
            digest.update(&chunk);
            file.write_all(&chunk).await?;
        }
        file.sync_all().await?;
        drop(file);
        anyhow::ensure!(
            size == artifact.bytes
                && hex::encode(digest.finalize()).eq_ignore_ascii_case(&artifact.sha256),
            "核心文件校验失败"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&part, std::fs::Permissions::from_mode(0o700))?;
        }
        let checked = tokio::time::timeout(
            Duration::from_secs(10),
            tokio::process::Command::new(&part)
                .kill_on_drop(true)
                .arg(if core == "realm" {
                    "--version"
                } else {
                    "version"
                })
                .output(),
        )
        .await??;
        anyhow::ensure!(
            checked.status.success()
                && String::from_utf8_lossy(&checked.stdout).contains(&artifact.version),
            "核心版本检查失败"
        );
        std::fs::rename(
            &part,
            directory.join(format!("{core}{}", std::env::consts::EXE_SUFFIX)),
        )?;
        tracing::info!(core, version=%artifact.version, "核心安装完成");
        Ok::<_, anyhow::Error>(())
    }
    .await;
    if result.is_err() {
        std::fs::remove_file(part).ok();
    }
    result
}
async fn stage(
    config: &Config,
    data: &Path,
    root: &Path,
    platform: &str,
    state: &UpdateState,
) -> Result<Option<Switch>> {
    let client = client(config)?;
    let base = format!("{}/downloads", config.controller);
    let response = client
        .get(format!("{base}/manifest-{platform}.json"))
        .timeout(Duration::from_secs(15))
        .send()
        .await?
        .error_for_status()?;
    anyhow::ensure!(
        response.content_length().unwrap_or(0) < 16384,
        "更新清单过大"
    );
    let bytes = response.bytes().await?;
    anyhow::ensure!(bytes.len() < 16384, "更新清单过大");
    let release: Release = serde_json::from_slice(&bytes)?;
    release.validate(platform)?;
    let current = std::fs::canonicalize(root.join("current"))?;
    anyhow::ensure!(
        state.blocked_build.as_deref() != Some(&release.build),
        "该版本曾更新失败并已回退，等待新的兼容版本"
    );
    if current.file_name().and_then(|s| s.to_str()) == Some(&release.build) {
        return Ok(None);
    }
    let next = root.join("releases").join(&release.build);
    crate::private_dir(&next)?;
    let desired = std::fs::read(data.join("desired.json"))
        .ok()
        .map(|b| serde_json::from_slice::<crate::cores::Desired>(&b))
        .transpose()?
        .unwrap_or_default();
    for (name, artifact) in [
        ("hproxy-agent", &release.agent),
        ("sing-box", &release.sing_box),
        ("realm", &release.realm),
    ] {
        if name != "hproxy-agent"
            && desired
                .cores
                .as_ref()
                .is_some_and(|cores| !cores.iter().any(|c| c == name))
        {
            let unused = next.join(name);
            if unused.is_file() {
                std::fs::remove_file(unused)?;
            }
            continue;
        }
        let target = next.join(name);
        if artifact.matches_file(&target)? { continue; }
        let part = next.join(format!("{name}.part"));
        let response = client
            .get(format!("{base}/{}", artifact.file))
            .send()
            .await?
            .error_for_status()?;
        let mut stream = response.bytes_stream();
        let mut file = tokio::fs::File::create(&part).await?;
        let mut digest = Sha256::new();
        let mut size = 0u64;
        while let Some(chunk) = stream.next().await {
            let bytes = chunk?;
            size += bytes.len() as u64;
            anyhow::ensure!(size <= artifact.bytes, "更新文件大于清单声明");
            digest.update(&bytes);
            file.write_all(&bytes).await?;
        }
        file.sync_all().await?;
        drop(file);
        anyhow::ensure!(
            size == artifact.bytes
                && hex::encode(digest.finalize()).eq_ignore_ascii_case(&artifact.sha256),
            "更新文件校验失败，保留当前版本"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&part, std::fs::Permissions::from_mode(0o700))?;
        }
        std::fs::rename(part, target)?;
    }
    atomic_json(&next.join("release.json"), &release)?;
    // The new agent validates the live resource configuration against its own
    // protocol schema and its matching cores, without binding or stopping ports.
    let checked = tokio::time::timeout(
        Duration::from_secs(60),
        tokio::process::Command::new(next.join("hproxy-agent"))
            .kill_on_drop(true)
            .arg("check")
            .arg("--data")
            .arg(data)
            .arg("--core-dir")
            .arg(&next)
            .output(),
    )
    .await??;
    anyhow::ensure!(
        checked.status.success(),
        "新版本配置检查失败：{}",
        String::from_utf8_lossy(&checked.stderr)
            .chars()
            .take(2000)
            .collect::<String>()
    );
    atomic_json(&next.join("release.json"), &release)?;
    tracing::info!(build=%release.build,version=%release.version,"新版本已下载并通过配置检查，准备更新");
    Ok(Some(Switch {
        root: root.into(),
        next,
        state: UpdateState {
            pending: Some(Pending {
                previous: current,
                build: release.build,
            }),
            blocked_build: state.blocked_build.clone(),
            previous: state.previous.clone(),
        },
    }))
}

pub async fn watch_release(
    config: Config,
    data: PathBuf,
    root: Option<PathBuf>,
    mut healthy: watch::Receiver<bool>,
    notify: Arc<Notify>,
    generation: Arc<AtomicI64>,
    reports: mpsc::Sender<AgentMessage>,
) -> Switch {
    let Some(root) = root else {
        return std::future::pending().await;
    };
    let Some(platform) = platform() else {
        return std::future::pending().await;
    };
    let state_path = data.join("update-state.json");
    let mut state: UpdateState = std::fs::read(&state_path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    if let Some(pending) = state.pending.as_ref() {
        let previous = pending.previous.clone();
        let build = pending.build.clone();
        let until = tokio::time::Instant::now() + Duration::from_secs(120);
        let mut stable = None;
        loop {
            if *healthy.borrow() {
                let since = stable.get_or_insert_with(tokio::time::Instant::now);
                if since.elapsed() >= Duration::from_secs(20) {
                    let verified = UpdateState {
                        pending: None, blocked_build: state.blocked_build.clone(),
                        previous: Some(previous.clone()),
                    };
                    match atomic_json(&state_path, &verified) {
                        Ok(()) => {
                            state = verified;
                            tracing::info!(%build,"更新后核心与主控连接检查通过");
                            break;
                        },
                        Err(error) => tracing::warn!(%error,"更新检查状态暂时无法保存"),
                    }
                }
            } else {
                stable = None;
            }
            if tokio::time::Instant::now() >= until {
                tracing::error!(%build,"新版本未通过上线检查，回退上一版");
                return Switch {
                    root,
                    next: previous,
                    state: UpdateState {
                        pending: None,
                        blocked_build: Some(build),
                        previous: state.previous.clone(),
                    },
                };
            }
            tokio::select! {_=tokio::time::sleep(Duration::from_secs(2))=>{},_=healthy.changed()=>{}}
        }
    }
    maintain_quietly(&data, &root);
    tokio::select! { _=tokio::time::sleep(Duration::from_secs(60))=>{}, _=notify.notified()=>{} }
    loop {
        let current = generation.load(Ordering::Relaxed);
        if current > 0 {
            reports
                .send(AgentMessage::UpdateState {
                    generation: current,
                    state: "checking".into(),
                    error: None,
                })
                .await
                .ok();
        }
        match stage(&config, &data, &root, platform, &state).await {
            Ok(Some(switch)) => {
                if current > 0 {
                    reports
                        .send(AgentMessage::UpdateState {
                            generation: current,
                            state: "restarting".into(),
                            error: None,
                        })
                        .await
                        .ok();
                }
                return switch;
            }
            Ok(None) => {
                if current > 0 {
                    reports
                        .send(AgentMessage::UpdateState {
                            generation: current,
                            state: "up_to_date".into(),
                            error: None,
                        })
                        .await
                        .ok();
                }
            }
            Err(error) => {
                tracing::warn!(%error,"自动更新检查失败，当前版本继续运行");
                if current > 0 {
                    reports
                        .send(AgentMessage::UpdateState {
                            generation: current,
                            state: "failed".into(),
                            error: Some(error.to_string().chars().take(2000).collect()),
                        })
                        .await
                        .ok();
                }
            }
        }
        maintain_quietly(&data, &root);
        // Only a small manifest is requested unless its build has changed.
        let next_check = tokio::time::Instant::now() + Duration::from_secs(6 * 3600);
        loop {
            tokio::select! {
                _=tokio::time::sleep_until(next_check)=>break,
                _=notify.notified()=>break,
                _=tokio::time::sleep(Duration::from_secs(3600))=>maintain_quietly(&data, &root),
            }
        }
    }
}
impl Switch {
    /// Called only after the old cores have drained and counters are durable.
    pub fn activate(self, data: &Path) -> Result<()> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            let releases = std::fs::canonicalize(self.root.join("releases"))?;
            let next = std::fs::canonicalize(&self.next)?;
            anyhow::ensure!(
                next.parent() == Some(releases.as_path()) && next.join("hproxy-agent").is_file(),
                "更新目录无效"
            );
            if self.state.pending.is_some() {
                if let Ok(bytes) = std::fs::read(data.join("desired.json")) {
                    let value: serde_json::Value = serde_json::from_slice(&bytes)?;
                    atomic_json(&data.join("update-previous-desired.json"), &value)?;
                }
            } else if self.state.blocked_build.is_some() {
                if let Ok(bytes) = std::fs::read(data.join("update-previous-desired.json")) {
                    let value: serde_json::Value = serde_json::from_slice(&bytes)?;
                    atomic_json(&data.join("desired.json"), &value)?;
                }
            }
            atomic_json(&data.join("update-state.json"), &self.state)?;
            let tmp = self.root.join("current.next");
            if std::fs::symlink_metadata(&tmp).is_ok() {
                std::fs::remove_file(&tmp)?;
            }
            symlink(&next, &tmp)?;
            std::fs::rename(&tmp, self.root.join("current"))?;
            std::fs::File::open(&self.root)?.sync_all()?;
            tracing::info!(release=%next.display(),"已切换版本，服务管理器将重启 agent");
            Ok(())
        }
        #[cfg(not(unix))]
        {
            let _ = (self, data);
            anyhow::bail!("自动更新仅支持 Linux 安装")
        }
    }
}
/// Invoked by the small shell supervisor if the new process cannot start at all.
pub fn recover(data: &Path, root: &Path, code: &str) -> Result<()> {
    if code == "75" {
        return Ok(());
    }
    let state: UpdateState = std::fs::read(data.join("update-state.json"))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    if let Some(pending) = state.pending {
        tracing::error!(exit_code=code,build=%pending.build,"更新后的进程异常退出，回退上一版");
        Switch {
            root: root.into(),
            next: pending.previous,
            state: UpdateState {
                pending: None,
                blocked_build: Some(pending.build),
                previous: state.previous,
            },
        }
        .activate(data)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use hproxy_common::release::Artifact;
    #[cfg(unix)]
    #[test]
    fn failed_activation_rolls_back_before_collecting_without_resetting_persistent_data() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("agent");
        let data = dir.path().join("data");
        std::fs::create_dir_all(&data).unwrap();
        let old = root.join("releases").join("a".repeat(64));
        let next = root.join("releases").join("b".repeat(64));
        for release in [&old, &next] {
            std::fs::create_dir_all(release).unwrap();
            std::fs::write(release.join("hproxy-agent"), "fixture").unwrap();
        }
        std::os::unix::fs::symlink(&old, root.join("current")).unwrap();
        for file in ["agent.json", "desired.json", "counters.json"] {
            atomic_json(&data.join(file), &serde_json::json!({"fixture":file})).unwrap();
        }
        let identity = std::fs::read(data.join("agent.json")).unwrap();
        let counters = std::fs::read(data.join("counters.json")).unwrap();
        Switch { root:root.clone(), next:next.clone(), state:UpdateState {
            pending:Some(Pending { previous:old.clone(), build:"b".repeat(64) }),
            ..UpdateState::default()
        }}.activate(&data).unwrap();
        recover(&data, &root, "75").unwrap();
        assert_eq!(maintenance::current_release(&root).unwrap(), std::fs::canonicalize(&next).unwrap());
        // An unverified new process is never eligible for collection.
        maintain(&data, &root).unwrap();
        assert!(old.exists());
        recover(&data, &root, "1").unwrap();
        assert_eq!(maintenance::current_release(&root).unwrap(), std::fs::canonicalize(&old).unwrap());
        maintain(&data, &root).unwrap();
        assert!(!next.exists());
        assert_eq!(std::fs::read(data.join("agent.json")).unwrap(), identity);
        assert_eq!(std::fs::read(data.join("counters.json")).unwrap(), counters);
    }
    #[test]
    fn collection_protects_pending_versions_and_never_changes_identity_configuration_or_totals() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("agent");
        let data = dir.path().join("data");
        std::fs::create_dir_all(&data).unwrap();
        for hash in ["a", "b", "c"] {
            std::fs::create_dir_all(root.join("releases").join(hash.repeat(64))).unwrap();
        }
        let current = root.join("releases").join("a".repeat(64));
        #[cfg(unix)] std::os::unix::fs::symlink(&current, root.join("current")).unwrap();
        #[cfg(windows)] assert!(std::process::Command::new("cmd").args(["/c", "mklink", "/J"])
            .arg(root.join("current")).arg(&current).output().unwrap().status.success());
        for file in ["agent.json", "desired.json", "counters.json"] {
            std::fs::write(data.join(file), format!("preserve {file}")).unwrap();
        }
        let path = data.join("update-state.json");
        let mut state = UpdateState { pending: Some(Pending { previous: current, build: "next".into() }),
            previous: Some(root.join("releases").join("b".repeat(64))), blocked_build: None };
        atomic_json(&path, &state).unwrap();
        maintain(&data, &root).unwrap();
        assert!(root.join("releases").join("c".repeat(64)).exists());
        state.pending = None;
        atomic_json(&path, &state).unwrap();
        maintain(&data, &root).unwrap();
        assert!(!root.join("releases").join("c".repeat(64)).exists());
        assert!(state.previous.as_ref().unwrap().exists());
        for file in ["agent.json", "desired.json", "counters.json"] {
            assert_eq!(std::fs::read_to_string(data.join(file)).unwrap(), format!("preserve {file}"));
        }
        std::fs::write(&path, b"broken").unwrap();
        assert!(maintain(&data, &root).is_err());
        assert!(state.previous.unwrap().exists());
    }
    #[test]
    fn rejects_untrusted_paths_and_oversized_artifacts() {
        fn artifact(name: &str) -> Artifact {
            Artifact {
                file: format!("{name}-linux-amd64"),
                sha256: "a".repeat(64),
                bytes: 123,
                version: "test".into(),
            }
        }
        let mut r = Release {
            schema: 1,
            server: None,
            build: "b".repeat(64),
            version: "test".into(),
            agent: artifact("hproxy-agent"),
            sing_box: artifact("sing-box"),
            realm: artifact("realm"),
        };
        assert!(r.validate("linux-amd64").is_ok());
        r.agent.file = "../../agent.json".into();
        assert!(r.validate("linux-amd64").is_err());
        r.agent = artifact("hproxy-agent");
        r.realm.bytes = 151 * 1024 * 1024;
        assert!(r.validate("linux-amd64").is_err());
    }
}
