//! The controller publishes complete verified manifests, retaining unselected artifacts.
use crate::{db, App};
use anyhow::{Context, Result};
use futures_util::StreamExt;
use hproxy_common::{
    atomic_json, now,
    maintenance,
    release::{platform, Artifact, Release, PLATFORMS},
    REALM_VERSION, SING_VERSION,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{io::AsyncWriteExt, sync::Notify};

#[derive(Clone, Serialize, Deserialize)]
struct Catalog {
    source: String,
    checked_at: i64,
    releases: BTreeMap<String, Release>,
}
#[derive(Clone, Copy, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Component {
    #[default]
    All,
    Hproxy,
    SingBox,
    Realm,
}
fn select_release(current: &Release, latest: &Release, component: Component) -> Release {
    let mut selected = current.clone();
    if matches!(component, Component::All | Component::Hproxy)
        && newer(&latest.version, &current.version)
    {
        selected.version = latest.version.clone();
        selected.agent = latest.agent.clone();
        selected.server = latest.server.clone();
    }
    if matches!(component, Component::All | Component::SingBox)
        && newer(&latest.sing_box.version, &current.sing_box.version)
    {
        selected.sing_box = latest.sing_box.clone();
    }
    if matches!(component, Component::All | Component::Realm)
        && newer(&latest.realm.version, &current.realm.version)
    {
        selected.realm = latest.realm.clone();
    }
    selected
}
fn published_release(downloads: &Path, target: &str) -> Result<Release> {
    let base = if downloads.join("current").is_dir() {
        downloads.join("current")
    } else {
        downloads.into()
    };
    let release: Release = serde_json::from_slice(&std::fs::read(
        base.join(format!("manifest-{target}.json")),
    )?)?;
    release.validate(target)?;
    Ok(release)
}
#[derive(Default)]
struct State {
    catalog: Option<Catalog>,
    phase: String,
    error: Option<String>,
}
pub struct Service {
    source: Option<String>,
    pub root: Option<PathBuf>,
    data: PathBuf,
    state: Mutex<State>,
    pub restart: Notify,
    staged: Mutex<Option<PathBuf>>,
    client: reqwest::Client,
}
#[derive(Default, Serialize, Deserialize)]
struct SwitchState {
    previous: Option<PathBuf>,
    pending: bool,
    blocked: Option<String>,
    candidate: Option<String>,
    #[serde(default)]
    source_build: Option<String>,
    downloads: Option<PathBuf>,
    previous_downloads: Option<PathBuf>,
    next_downloads: Option<PathBuf>,
    #[serde(default)]
    rollback_previous: Option<PathBuf>,
    #[serde(default)]
    rollback_downloads: Option<PathBuf>,
}

pub fn newer(candidate: &str, current: &str) -> bool {
    match (
        semver::Version::parse(candidate.trim_start_matches('v')),
        semver::Version::parse(current.trim_start_matches('v')),
    ) {
        (Ok(next), Ok(old)) => next > old,
        _ => false,
    }
}
impl Service {
    pub fn new(source: Option<String>, root: Option<PathBuf>, data: PathBuf) -> Result<Arc<Self>> {
        Self::with_ca(source, root, data, None)
    }
    pub fn with_ca(
        source: Option<String>,
        root: Option<PathBuf>,
        data: PathBuf,
        ca: Option<Vec<u8>>,
    ) -> Result<Arc<Self>> {
        if let Some(source) = &source {
            let url = url::Url::parse(source)?;
            anyhow::ensure!(
                url.scheme() == "https"
                    && url.username().is_empty()
                    && url.password().is_none()
                    && url.query().is_none()
                    && url.fragment().is_none(),
                "更新源必须为不含认证信息的 HTTPS 地址"
            );
        }
        let source = source.map(|s| s.trim_end_matches('/').to_owned());
        let mut client = reqwest::Client::builder()
            .https_only(true)
            .timeout(Duration::from_secs(300));
        if let Some(pem) = ca {
            client = client.add_root_certificate(reqwest::Certificate::from_pem(&pem)?);
        }
        let catalog = std::fs::read(data.join("update-catalog.json"))
            .ok()
            .and_then(|b| serde_json::from_slice::<Catalog>(&b).ok())
            .filter(|c| Some(&c.source) == source.as_ref());
        Ok(Arc::new(Self {
            source,
            root,
            data,
            state: Mutex::new(State {
                catalog,
                phase: "idle".into(),
                error: None,
            }),
            restart: Notify::new(),
            staged: Mutex::new(None),
            client: client.build()?,
        }))
    }
    pub fn status(&self, app: &App) -> Result<Value> {
        let state = self.state.lock().unwrap();
        let release = state
            .catalog
            .as_ref()
            .and_then(|c| platform().and_then(|p| c.releases.get(p)))
            .or_else(|| {
                state
                    .catalog
                    .as_ref()
                    .and_then(|c| c.releases.values().next())
            });
        let installed = platform().and_then(|p| published_release(&app.downloads, p).ok());
        let versions = [
            (
                "hproxy",
                "hproxy",
                env!("CARGO_PKG_VERSION"),
                release.map(|r| r.version.as_str()),
            ),
            (
                "sing_box",
                "sing-box",
                installed
                    .as_ref()
                    .map_or(SING_VERSION, |r| r.sing_box.version.as_str()),
                release.map(|r| r.sing_box.version.as_str()),
            ),
            (
                "realm",
                "Realm",
                installed
                    .as_ref()
                    .map_or(REALM_VERSION, |r| r.realm.version.as_str()),
                release.map(|r| r.realm.version.as_str()),
            ),
        ];
        let core_ready = component_update_ready(&app.connection())?;
        let manifests_ready = PLATFORMS
            .iter()
            .all(|target| published_release(&app.downloads, target).is_ok());
        let components: Vec<Value> = versions.into_iter().map(|(key,name,current,latest)| {
            let blocked_reason = if !manifests_ready { "缺少已安装发布清单，请先更新全部组件" }
                else if key != "hproxy" && !core_ready { "请先更新 hproxy，并等待所有服务器完成同步后再单独更新核心" }
                else { "" };
            json!({"key":key,"name":name,"current":current,"latest":latest,"can_update":blocked_reason.is_empty(),"blocked_reason":blocked_reason,"state":match latest { Some(v) if newer(v,current)=>"available",Some(v) if v==current=>"current",Some(_)=>"newer",None=>"unchecked" }})
        }).collect();
        let agents = db::list_agents(&app.connection())?;
        let details: Vec<Value> = agents.as_array().unwrap().iter().filter(|a| a["enrolled"].as_bool().unwrap_or(false) && !a["revoked"].as_bool().unwrap_or(false)).map(|a| json!({"name":a["name"],"state":a["update_state"],"error":a["update_error"],"online":a["last_seen"].as_i64().unwrap_or(0)>now()-35000})).collect();
        let switch: SwitchState = self
            .root
            .as_ref()
            .and_then(|root| std::fs::read(root.join("update-state.json")).ok())
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        let rolled_back = release.is_some_and(|r| {
            switch.blocked.as_deref() == Some(r.build.as_str())
                || (switch.blocked.is_some()
                    && switch.source_build.as_deref() == Some(r.build.as_str()))
        });
        let phase = if switch.pending && state.phase == "idle" {
            "verifying"
        } else if rolled_back && state.phase == "idle" {
            "failed"
        } else {
            state.phase.as_str()
        };
        let error = state
            .error
            .clone()
            .or_else(|| rolled_back.then(|| "新版本未通过启动检查，已自动回退上一版".to_string()));
        Ok(
            json!({"phase":phase,"error":error,"source_configured":self.source.is_some(),"can_update":platform().is_some() && self.root.is_some(),"checked_at":state.catalog.as_ref().map(|c|c.checked_at),"build":release.map(|r|&r.build),"components":components,"agents":details}),
        )
    }
    fn begin(&self, phase: &str) -> Result<()> {
        anyhow::ensure!(
            self.source.is_some(),
            "尚未配置发布更新源，请按部署文档设置 --update-source"
        );
        let mut state = self.state.lock().unwrap();
        anyhow::ensure!(
            !["checking", "downloading", "restarting"].contains(&state.phase.as_str()),
            "已有更新任务正在执行"
        );
        state.phase = phase.into();
        state.error = None;
        Ok(())
    }
    fn failed(&self, error: anyhow::Error) {
        tracing::error!(%error, "版本更新失败，保留当前版本");
        let mut state = self.state.lock().unwrap();
        state.phase = "failed".into();
        state.error = Some(error.to_string().chars().take(2000).collect());
    }
    pub fn check(self: &Arc<Self>) -> Result<()> {
        self.begin("checking")?;
        let service = self.clone();
        tokio::spawn(async move {
            if let Err(e) = service.fetch_catalog().await {
                service.failed(e);
            }
        });
        Ok(())
    }
    pub fn auto_check(self: &Arc<Self>) {
        if self.source.is_none() {
            return;
        }
        let service = self.clone();
        tokio::spawn(async move {
            loop {
                service.check().ok();
                tokio::time::sleep(Duration::from_secs(6 * 3600)).await;
            }
        });
    }
    /// No collector runs during a download, activation, or rollback verification.
    pub fn maintain(&self, downloads: &Path) -> Result<()> {
        let state = self.state.lock().unwrap();
        if matches!(state.phase.as_str(), "downloading" | "restarting") { return Ok(()); }
        let Some(root) = &self.root else { return Ok(()); };
        let switch: SwitchState = match std::fs::read(root.join("update-state.json")) {
            Ok(bytes) => serde_json::from_slice(&bytes)?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => SwitchState::default(),
            Err(e) => return Err(e.into()),
        };
        if switch.pending { return Ok(()); }
        let mut keep = vec![maintenance::current_release(root)?];
        if let Some(previous) = switch.previous { keep.push(previous); }
        maintenance::prune_releases(root, &keep)?;
        for release in &keep {
            maintenance::remove_child(release, &release.join("check-data"))?;
            maintenance::prune_temporary_files(release)?;
        }
        let mut keep_downloads = vec![maintenance::current_release(downloads)?];
        if let Some(previous) = switch.previous_downloads { keep_downloads.push(previous); }
        maintenance::prune_releases(downloads, &keep_downloads)?;
        maintenance::prune_staging(downloads)?;
        maintenance::prune_backups(&self.data)?;
        maintenance::prune_temporary_files(&self.data.join("backups"))?;
        maintenance::prune_temporary_files(&self.data)?;
        maintenance::prune_temporary_files(root)?;
        Ok(())
    }
    async fn fetch_catalog(&self) -> Result<()> {
        let source = self.source.as_ref().context("未配置更新源")?;
        let client = &self.client;
        let mut releases = BTreeMap::new();
        for target in PLATFORMS {
            let response = client
                .get(format!("{source}/manifest-{target}.json"))
                .timeout(Duration::from_secs(15))
                .send()
                .await?
                .error_for_status()?;
            let bytes = bounded_bytes(response, 16_384).await?;
            let release: Release = serde_json::from_slice(&bytes)?;
            release.validate(target)?;
            anyhow::ensure!(release.server.is_some(), "发布包缺少主控文件");
            for version in [
                &release.version,
                &release.sing_box.version,
                &release.realm.version,
            ] {
                semver::Version::parse(version).context("发布版本号无效")?;
            }
            if let Some(first) = releases.values().next() {
                let first: &Release = first;
                anyhow::ensure!(
                    first.version == release.version
                        && first.sing_box.version == release.sing_box.version
                        && first.realm.version == release.realm.version,
                    "各平台发布版本不一致，请稍后重试"
                );
            }
            releases.insert(target.to_string(), release);
        }
        let catalog = Catalog {
            source: source.clone(),
            checked_at: now(),
            releases,
        };
        atomic_json(&self.data.join("update-catalog.json"), &catalog)?;
        let mut state = self.state.lock().unwrap();
        state.catalog = Some(catalog);
        state.phase = "idle".into();
        Ok(())
    }
    pub fn install(self: &Arc<Self>, app: App, build: String, component: Component) -> Result<()> {
        anyhow::ensure!(
            platform().is_some() && self.root.is_some(),
            "当前为手动运行方式，请使用主控安装脚本启用在线更新"
        );
        if matches!(component, Component::SingBox | Component::Realm) {
            anyhow::ensure!(
                component_update_ready(&app.connection())?,
                "请先更新 hproxy，并等待所有服务器完成同步后再单独更新核心"
            );
        }
        let mut catalog = self
            .state
            .lock()
            .unwrap()
            .catalog
            .clone()
            .context("请先检查更新")?;
        let release = catalog
            .releases
            .get(platform().unwrap())
            .context("缺少当前平台发布包")?;
        anyhow::ensure!(release.build == build, "更新清单已变化，请重新检查更新");
        let expected = release.clone();
        let current = published_release(&app.downloads, platform().unwrap()).ok();
        let has_update = match component {
            Component::All => {
                newer(&release.version, env!("CARGO_PKG_VERSION"))
                    || newer(
                        &release.sing_box.version,
                        current
                            .as_ref()
                            .map_or(SING_VERSION, |r| &r.sing_box.version),
                    )
                    || newer(
                        &release.realm.version,
                        current.as_ref().map_or(REALM_VERSION, |r| &r.realm.version),
                    )
            }
            Component::Hproxy => newer(&release.version, env!("CARGO_PKG_VERSION")),
            Component::SingBox => newer(
                &release.sing_box.version,
                current
                    .as_ref()
                    .map_or(SING_VERSION, |r| &r.sing_box.version),
            ),
            Component::Realm => newer(
                &release.realm.version,
                current.as_ref().map_or(REALM_VERSION, |r| &r.realm.version),
            ),
        };
        anyhow::ensure!(has_update, "所选组件没有可用更新");
        if component != Component::All || current.is_some() {
            for (target, latest) in &mut catalog.releases {
                let current = published_release(&app.downloads, target)
                    .context("缺少完整的已安装发布清单，请先部署完整发布包或更新全部组件")?;
                *latest = select_release(&current, latest, component);
            }
            let selected_build =
                hex::encode(Sha256::digest(serde_json::to_vec(&catalog.releases)?));
            for release in catalog.releases.values_mut() {
                release.build = selected_build.clone();
            }
        }
        let switch: SwitchState =
            std::fs::read(self.root.as_ref().unwrap().join("update-state.json"))
                .ok()
                .and_then(|b| serde_json::from_slice(&b).ok())
                .unwrap_or_default();
        anyhow::ensure!(!switch.pending, "正在确认上次更新结果，请稍后重试");
        anyhow::ensure!(
            switch.blocked.as_deref() != Some(&catalog.releases[platform().unwrap()].build),
            "此发布曾启动失败并已回退，请等待修复版本"
        );
        self.begin("downloading")?;
        let service = self.clone();
        tokio::spawn(async move {
            if let Err(e) = service.stage(app.clone(), catalog, expected).await {
                service.failed(e);
            }
            if let Err(e) = service.maintain(&app.downloads) {
                tracing::warn!(error=%e,"更新文件清理失败，稍后重试");
            }
        });
        Ok(())
    }
    async fn stage(&self, app: App, catalog: Catalog, expected: Release) -> Result<()> {
        let client = &self.client;
        let target = platform().context("不支持当前平台")?;
        let root = self.root.as_ref().context("未使用主控安装器")?;
        let release = catalog.releases.get(target).unwrap();
        let next = root.join("releases").join(&release.build);
        let bundle = app.downloads.join("staging").join(&release.build);
        std::fs::create_dir_all(&bundle)?;
        // No files become available to agents until every platform is complete.
        for (platform, release) in &catalog.releases {
            for artifact in [&release.agent, &release.sing_box, &release.realm] {
                let current_base = if app.downloads.join("current").is_dir() {
                    app.downloads.join("current")
                } else {
                    app.downloads.clone()
                };
                let existing = current_base.join(&artifact.file);
                let reusable = artifact.matches_file(&existing).unwrap_or(false);
                if reusable {
                    std::fs::copy(existing, bundle.join(&artifact.file))?;
                } else {
                    download(
                        &client,
                        &catalog.source,
                        artifact,
                        &bundle.join(&artifact.file),
                    )
                    .await?;
                }
            }
            let sums = [&release.agent, &release.sing_box, &release.realm]
                .iter()
                .map(|a| format!("{}  {}\n", a.sha256, a.file))
                .collect::<String>();
            std::fs::write(bundle.join(format!("SHA256SUMS-{platform}")), sums)?;
            atomic_json(&bundle.join(format!("manifest-{platform}.json")), release)?;
        }
        let published = app.downloads.join("releases").join(&release.build);
        std::fs::create_dir_all(published.parent().unwrap())?;
        if !published.exists() {
            std::fs::rename(&bundle, &published)?;
        }
        if !newer(&release.version, env!("CARGO_PKG_VERSION")) {
            let state_path = root.join("update-state.json");
            let mut switch: SwitchState = match std::fs::read(&state_path) {
                Ok(bytes) => serde_json::from_slice(&bytes)?,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => SwitchState::default(),
                Err(e) => return Err(e.into()),
            };
            switch.previous_downloads = Some(maintenance::current_release(&app.downloads)?);
            switch.downloads = Some(std::fs::canonicalize(&app.downloads)?);
            switch.next_downloads = Some(std::fs::canonicalize(&published)?);
            atomic_json(&state_path, &switch)?;
            switch_link(&app.downloads, &published)?;
            queue_agents(&app)?;
            self.state.lock().unwrap().phase = "idle".into();
            return Ok(());
        }
        maintenance::private_dir(&next)?;
        let artifact = release.server.as_ref().context("发布包缺少主控文件")?;
        let binary = next.join("hproxy-server");
        download(&client, &catalog.source, artifact, &binary).await?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700))?;
        }
        let version = tokio::time::timeout(
            Duration::from_secs(15),
            tokio::process::Command::new(&binary)
                .kill_on_drop(true)
                .arg("--version")
                .output(),
        )
        .await??;
        anyhow::ensure!(
            version.status.success()
                && String::from_utf8_lossy(&version.stdout).trim()
                    == format!("hproxy-server {}", release.version),
            "主控文件版本检查失败"
        );
        let info = tokio::time::timeout(
            Duration::from_secs(15),
            tokio::process::Command::new(&binary)
                .kill_on_drop(true)
                .arg("release-info")
                .output(),
        )
        .await??;
        let pins: Value = serde_json::from_slice(&info.stdout).context("无法读取新主控核心版本")?;
        if release.sing_box.version != expected.sing_box.version
            || release.realm.version != expected.realm.version
        {
            anyhow::ensure!(
                pins["component_updates"] == true,
                "目标 hproxy 发布不支持保留旧核心，请选择更新全部组件"
            );
        }
        anyhow::ensure!(
            info.status.success()
                && pins["version"] == expected.version
                && pins["sing_box"] == expected.sing_box.version
                && pins["realm"] == expected.realm.version,
            "发布核心与新主控的兼容版本不一致"
        );
        let backup = self
            .data
            .join("backups")
            .join(format!("before-update-{}.db", release.build));
        maintenance::private_dir(backup.parent().unwrap())?;
        if !backup.exists() {
            let part = backup.with_extension("part");
            for extension in ["part", "part-journal", "part-wal", "part-shm"] {
                maintenance::remove_child(backup.parent().unwrap(), &backup.with_extension(extension))?;
            }
            app.connection().backup("main", &part, None)?;
            std::fs::rename(part, &backup)?;
        }
        let check_dir = next.join("check-data");
        std::fs::create_dir_all(&check_dir)?;
        std::fs::copy(&backup, check_dir.join("hproxy.db"))?;
        let checked = tokio::time::timeout(
            Duration::from_secs(30),
            tokio::process::Command::new(&binary)
                .kill_on_drop(true)
                .arg("check")
                .arg("--data")
                .arg(&check_dir)
                .output(),
        )
        .await;
        // Drop the candidate process before removing its temporary database.
        let checked = checked??;
        maintenance::remove_child(&next, &check_dir)?;
        anyhow::ensure!(
            checked.status.success(),
            "新主控数据库检查失败：{}",
            String::from_utf8_lossy(&checked.stderr)
                .chars()
                .take(1000)
                .collect::<String>()
        );
        atomic_json(&next.join("release.json"), release)?;
        let previous = std::fs::canonicalize(root.join("current"))?;
        validate_target(root, &previous)?;
        let last: SwitchState = match std::fs::read(root.join("update-state.json")) {
            Ok(bytes) => serde_json::from_slice(&bytes)?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => SwitchState::default(),
            Err(e) => return Err(e.into()),
        };
        // Publish immutable version directories; root paths switch as one unit.
        let downloads = std::fs::canonicalize(&app.downloads)?;
        let previous_downloads = std::fs::canonicalize(app.downloads.join("current")).ok();
        atomic_json(
            &root.join("update-state.json"),
            &SwitchState {
                previous: Some(previous),
                pending: true,
                blocked: None,
                candidate: Some(release.build.clone()),
                source_build: Some(expected.build.clone()),
                downloads: Some(downloads),
                previous_downloads,
                next_downloads: Some(std::fs::canonicalize(published)?),
                rollback_previous: last.previous,
                rollback_downloads: last.previous_downloads,
            },
        )?;
        *self.staged.lock().unwrap() = Some(next);
        self.state.lock().unwrap().phase = "restarting".into();
        tracing::info!(version=%release.version, "更新包校验完成，准备重启主控并通知子 agent");
        self.restart.notify_one();
        Ok(())
    }
    pub fn activate(&self) -> Result<()> {
        let next = self
            .staged
            .lock()
            .unwrap()
            .take()
            .context("没有待切换版本")?;
        let root = self.root.as_ref().context("未启用托管更新")?;
        validate_target(root, &next)?;
        switch_link(root, &next)
    }
    pub fn mark_healthy(self: &Arc<Self>, app: App) {
        let Some(root) = self.root.clone() else {
            return;
        };
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(30)).await;
            let path = root.join("update-state.json");
            let result = (|| -> Result<()> {
                let mut state: SwitchState = std::fs::read(&path)
                    .ok()
                    .and_then(|b| serde_json::from_slice(&b).ok())
                    .unwrap_or_default();
                if state.pending {
                    let current = std::fs::canonicalize(root.join("current"))?;
                    if current.file_name().and_then(|s| s.to_str()) != state.candidate.as_deref() {
                        return Ok(());
                    }
                    let downloads = state.downloads.as_ref().context("缺少下载目录")?;
                    let next = state
                        .next_downloads
                        .as_ref()
                        .context("缺少子 agent 发布包")?;
                    let releases = std::fs::canonicalize(downloads.join("releases"))?;
                    anyhow::ensure!(
                        std::fs::canonicalize(next)?.parent() == Some(releases.as_path()),
                        "发布目录无效"
                    );
                    switch_link(downloads, next)?;
                    queue_agents(&app)?;
                    state.pending = false;
                    state.rollback_previous = None;
                    state.rollback_downloads = None;
                    atomic_json(&path, &state)?;
                    tracing::info!("主控更新后运行检查通过，通知子 agent 更新");
                }
                Ok(())
            })();
            if let Err(e) = result {
                tracing::error!(error=%e,"更新完成检查失败，将回退");
                std::process::exit(1);
            }
            if let Err(e) = app.updates.maintain(&app.downloads) {
                tracing::warn!(error=%e,"后台文件清理失败，稍后重试");
            }
        });
    }
}

fn component_update_ready(connection: &rusqlite::Connection) -> Result<bool> {
    // Deleting agents only receive removal commands and never consume releases.
    Ok(!connection.query_row("SELECT EXISTS(SELECT 1 FROM agents WHERE token_hash IS NOT NULL AND revoked=0 AND deletion_id='' AND (component_updates=0 OR update_enabled=0))", [], |r| r.get::<_, bool>(0))?)
}
fn queue_agents(app: &App) -> Result<()> {
    let mut c = app.connection();
    let tx = c.transaction()?;
    tx.execute("INSERT INTO meta VALUES('update_generation',?) ON CONFLICT(key) DO UPDATE SET value=excluded.value", [now().to_string()])?;
    tx.execute("UPDATE agents SET update_state='queued',update_error='' WHERE token_hash IS NOT NULL AND revoked=0", [])?;
    tx.commit()?;
    Ok(())
}

async fn bounded_bytes(response: reqwest::Response, max: usize) -> Result<Vec<u8>> {
    anyhow::ensure!(
        response.content_length().is_none_or(|n| n <= max as u64),
        "更新清单过大"
    );
    let mut stream = response.bytes_stream();
    let mut out = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        anyhow::ensure!(out.len() + chunk.len() <= max, "更新清单过大");
        out.extend_from_slice(&chunk);
    }
    Ok(out)
}
async fn download(
    client: &reqwest::Client,
    source: &str,
    artifact: &Artifact,
    target: &Path,
) -> Result<()> {
    if artifact.matches_file(target)? { return Ok(()); }
    let response = client
        .get(format!("{source}/{}", artifact.file))
        .send()
        .await?
        .error_for_status()?;
    let mut stream = response.bytes_stream();
    let mut hash = Sha256::new();
    let mut size = 0u64;
    let part = target.with_extension("part");
    let mut file = tokio::fs::File::create(&part).await?;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        size += chunk.len() as u64;
        anyhow::ensure!(size <= artifact.bytes, "更新文件超出声明大小");
        hash.update(&chunk);
        file.write_all(&chunk).await?;
    }
    file.sync_all().await?;
    drop(file);
    anyhow::ensure!(
        size == artifact.bytes
            && hex::encode(hash.finalize()).eq_ignore_ascii_case(&artifact.sha256),
        "更新文件校验失败"
    );
    tokio::fs::rename(part, target).await?;
    Ok(())
}
fn validate_target(root: &Path, next: &Path) -> Result<()> {
    let releases = std::fs::canonicalize(root.join("releases"))?;
    let next = std::fs::canonicalize(next)?;
    anyhow::ensure!(
        next.parent() == Some(releases.as_path()) && next.join("hproxy-server").is_file(),
        "主控更新目录无效"
    );
    Ok(())
}
fn switch_link(root: &Path, next: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        std::fs::create_dir_all(root)?;
        let next = std::fs::canonicalize(next)?;
        let tmp = root.join("current.next");
        if std::fs::symlink_metadata(&tmp).is_ok() {
            std::fs::remove_file(&tmp)?;
        }
        symlink(next, &tmp)?;
        std::fs::rename(tmp, root.join("current"))?;
        std::fs::File::open(root)?.sync_all()?;
        Ok(())
    }
    #[cfg(not(unix))]
    {
        let _ = (root, next);
        anyhow::bail!("在线切换仅支持 Linux 托管安装")
    }
}
pub fn recover(root: &Path) -> Result<()> {
    let path = root.join("update-state.json");
    let mut state: SwitchState = std::fs::read(&path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    if state.pending {
        let previous = state.previous.as_ref().context("缺少回退版本")?;
        validate_target(root, previous)?;
        state.blocked = state.candidate.clone();
        switch_link(root, previous)?;
        if let (Some(downloads), Some(previous)) = (&state.downloads, &state.previous_downloads) {
            let releases = std::fs::canonicalize(downloads.join("releases"))?;
            anyhow::ensure!(
                std::fs::canonicalize(previous)?.parent() == Some(releases.as_path()),
                "回退发布目录无效"
            );
            switch_link(downloads, previous)?;
        }
        state.pending = false;
        state.previous = state.rollback_previous.take();
        state.previous_downloads = state.rollback_downloads.take();
        atomic_json(&path, &state)?;
        tracing::error!("新主控启动失败，已回退上一版");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn point_current(root: &Path, target: &Path) {
        #[cfg(unix)] std::os::unix::fs::symlink(target, root.join("current")).unwrap();
        #[cfg(windows)] assert!(std::process::Command::new("cmd").args(["/c", "mklink", "/J"])
            .arg(root.join("current")).arg(target).output().unwrap().status.success());
    }
    #[cfg(unix)]
    #[test]
    fn supervisor_recovery_restores_both_links_and_retains_previous_healthy_releases() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("server");
        let downloads = root.join("downloads");
        let data = dir.path().join("data");
        std::fs::create_dir_all(&data).unwrap();
        for container in [&root, &downloads] {
            for hash in ["a", "b", "c"] {
                let path = container.join("releases").join(hash.repeat(64));
                std::fs::create_dir_all(&path).unwrap();
                std::fs::write(path.join("hproxy-server"), "fixture").unwrap();
            }
            point_current(container, &container.join("releases").join("b".repeat(64)));
        }
        let live = db::open(&data.join("hproxy.db")).unwrap();
        live.execute("INSERT INTO meta VALUES('fixture','live writes')", []).unwrap();
        atomic_json(&root.join("update-state.json"), &SwitchState {
            pending:true, candidate:Some("b".repeat(64)),
            previous:Some(root.join("releases").join("a".repeat(64))),
            previous_downloads:Some(downloads.join("releases").join("a".repeat(64))),
            downloads:Some(downloads.clone()),
            rollback_previous:Some(root.join("releases").join("c".repeat(64))),
            rollback_downloads:Some(downloads.join("releases").join("c".repeat(64))),
            ..SwitchState::default()
        }).unwrap();
        recover(&root).unwrap();
        Service::new(None, Some(root.clone()), data).unwrap().maintain(&downloads).unwrap();
        for container in [&root, &downloads] {
            assert_eq!(maintenance::current_release(container).unwrap(), std::fs::canonicalize(container.join("releases").join("a".repeat(64))).unwrap());
            assert!(!container.join("releases").join("b".repeat(64)).exists());
            assert!(container.join("releases").join("c".repeat(64)).exists());
        }
        assert_eq!(live.query_row("SELECT value FROM meta WHERE key='fixture'", [], |r| r.get::<_,String>(0)).unwrap(), "live writes");
        recover(&root).unwrap(); // Repeated supervisor recovery is harmless.
    }
    #[test]
    fn collection_waits_for_health_then_preserves_rollback_data_and_manual_backups() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("server");
        let downloads = root.join("downloads");
        let data = dir.path().join("data");
        std::fs::create_dir_all(&data).unwrap();
        for container in [&root, &downloads] {
            for hash in ["a", "b", "c"] {
                let release = container.join("releases").join(hash.repeat(64));
                std::fs::create_dir_all(&release).unwrap();
                std::fs::write(release.join("hproxy-server"), "binary").unwrap();
            }
            point_current(container, &container.join("releases").join("a".repeat(64)));
        }
        let live = db::open(&data.join("hproxy.db")).unwrap();
        live.execute("INSERT INTO meta VALUES('fixture','preserved')", []).unwrap();
        let service = Service::new(None, Some(root.clone()), data.clone()).unwrap();
        let path = root.join("update-state.json");
        let mut switch = SwitchState {
            pending: true, previous: Some(root.join("releases").join("b".repeat(64))),
            previous_downloads: Some(downloads.join("releases").join("b".repeat(64))),
            ..SwitchState::default()
        };
        atomic_json(&path, &switch).unwrap();
        service.maintain(&downloads).unwrap();
        assert!(root.join("releases").join("c".repeat(64)).exists());
        switch.pending = false;
        atomic_json(&path, &switch).unwrap();
        service.state.lock().unwrap().phase = "downloading".into();
        service.maintain(&downloads).unwrap();
        assert!(root.join("releases").join("c".repeat(64)).exists());
        service.state.lock().unwrap().phase = "idle".into();
        service.maintain(&downloads).unwrap();
        for container in [&root, &downloads] {
            assert!(container.join("releases").join("a".repeat(64)).exists());
            assert!(container.join("releases").join("b".repeat(64)).exists());
            assert!(!container.join("releases").join("c".repeat(64)).exists());
        }
        assert_eq!(live.query_row("SELECT value FROM meta WHERE key='fixture'", [], |r| r.get::<_,String>(0)).unwrap(), "preserved");
        // Corrupt state must fail closed instead of silently losing rollback artifacts.
        std::fs::write(&path, b"broken").unwrap();
        assert!(service.maintain(&downloads).is_err());
        assert!(switch.previous.unwrap().exists());
    }
    #[test]
    fn older_or_unmanaged_agents_must_upgrade_before_independent_core_updates() {
        let dir = tempfile::tempdir().unwrap();
        let c = db::open(&dir.path().join("test.db")).unwrap();
        assert!(component_update_ready(&c).unwrap());
        c.execute(
            "INSERT INTO agents(id,name,token_hash) VALUES('one','one','hash')",
            [],
        )
        .unwrap();
        assert!(!component_update_ready(&c).unwrap());
        c.execute("UPDATE agents SET component_updates=1,update_enabled=1", [])
            .unwrap();
        assert!(component_update_ready(&c).unwrap());
        c.execute("UPDATE agents SET update_enabled=0", []).unwrap();
        assert!(!component_update_ready(&c).unwrap());
        c.execute("UPDATE agents SET deletion_id='pending-cleanup'", []).unwrap();
        assert!(component_update_ready(&c).unwrap());
        c.execute("UPDATE agents SET deletion_id=''", []).unwrap();
        assert!(!component_update_ready(&c).unwrap());
    }
    fn fixture(version: &str, sing: &str, realm: &str, hash: &str) -> Release {
        let artifact = |name: &str, version: &str| Artifact {
            file: format!("{name}-linux-amd64"),
            sha256: hash.repeat(64),
            bytes: 8,
            version: version.into(),
        };
        Release {
            schema: 1,
            build: hash.repeat(64),
            version: version.into(),
            agent: artifact("hproxy-agent", version),
            server: Some(artifact("hproxy-server", version)),
            sing_box: artifact("sing-box", sing),
            realm: artifact("realm", realm),
        }
    }
    #[test]
    fn individual_updates_preserve_every_unselected_artifact() {
        let current = fixture("0.1.0", "1.14.3", "2.9.6-hproxy.1", "a");
        let latest = fixture("0.1.1", "1.14.4", "2.9.6-hproxy.2", "b");
        for (component, changed) in [
            (Component::Hproxy, [true, false, false]),
            (Component::SingBox, [false, true, false]),
            (Component::Realm, [false, false, true]),
            (Component::All, [true, true, true]),
        ] {
            let selected = select_release(&current, &latest, component);
            selected.validate("linux-amd64").unwrap();
            for (index, (old, next, actual)) in [
                (&current.agent, &latest.agent, &selected.agent),
                (&current.sing_box, &latest.sing_box, &selected.sing_box),
                (&current.realm, &latest.realm, &selected.realm),
            ]
            .into_iter()
            .enumerate()
            {
                assert_eq!(
                    serde_json::to_value(actual).unwrap(),
                    serde_json::to_value(if changed[index] { next } else { old }).unwrap()
                );
            }
            assert_eq!(
                selected.version,
                if changed[0] {
                    &latest.version
                } else {
                    &current.version
                }
                .as_str()
            );
            assert_eq!(
                serde_json::to_value(&selected.server).unwrap(),
                serde_json::to_value(if changed[0] {
                    &latest.server
                } else {
                    &current.server
                })
                .unwrap()
            );
        }
    }
    #[test]
    fn selection_never_downgrades_already_newer_components() {
        let current = fixture("0.2.0", "1.15.0", "2.10.0", "a");
        let older = fixture("0.1.1", "1.14.4", "2.9.6-hproxy.2", "b");
        assert_eq!(
            serde_json::to_value(select_release(&current, &older, Component::All)).unwrap(),
            serde_json::to_value(current).unwrap()
        );
        assert!(serde_json::from_str::<Component>("\"invalid\"").is_err());
        assert!(matches!(
            serde_json::from_str::<Component>("\"sing_box\"").unwrap(),
            Component::SingBox
        ));
    }
    #[test]
    fn managed_core_versions_follow_validated_manifest_and_unmanaged_keep_pins() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            hproxy_common::release::core_version(dir.path(), "sing-box").unwrap(),
            SING_VERSION
        );
        let selected = fixture("0.1.0", "1.14.4", "2.9.6-hproxy.1", "a");
        atomic_json(&dir.path().join("release.json"), &selected).unwrap();
        assert_eq!(
            hproxy_common::release::core_version(dir.path(), "sing-box").unwrap(),
            "1.14.4"
        );
        assert_eq!(
            hproxy_common::release::core_version(dir.path(), "realm").unwrap(),
            "2.9.6-hproxy.1"
        );
        let mut bad = selected;
        bad.sing_box.file = "../unsafe".into();
        atomic_json(&dir.path().join("release.json"), &bad).unwrap();
        assert!(hproxy_common::release::core_version(dir.path(), "sing-box").is_err());
    }
    #[test]
    fn version_order_is_semantic() {
        assert!(newer("0.1.10", "0.1.9"));
        assert!(newer("2.9.6-hproxy.2", "2.9.6-hproxy.1"));
        assert!(!newer("0.1.0", "0.2.0"));
        assert!(!newer("garbage", "0.1.0"));
    }
    #[test]
    fn rejects_insecure_update_sources() {
        for source in [
            "http://example.com",
            "https://u:p@example.com",
            "https://example.com?x=1",
        ] {
            assert!(Service::new(Some(source.into()), None, PathBuf::new()).is_err());
        }
    }
    #[test]
    fn switch_cannot_escape_release_directory() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("releases/one")).unwrap();
        std::fs::write(dir.path().join("releases/one/hproxy-server"), b"x").unwrap();
        std::fs::write(dir.path().join("hproxy-server"), b"x").unwrap();
        assert!(validate_target(dir.path(), &dir.path().join("releases/one")).is_ok());
        assert!(validate_target(dir.path(), dir.path()).is_err());
    }
    #[tokio::test]
    async fn corrupted_or_oversized_download_never_replaces_installed_file() {
        use axum::{routing::get, Router};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async {
            axum::serve(
                listener,
                Router::new().route("/fixture", get(|| async { "bad file" })),
            )
            .await
            .unwrap()
        });
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("hproxy-server");
        std::fs::write(&target, b"installed").unwrap();
        let mut artifact = Artifact {
            file: "fixture".into(),
            sha256: hex::encode(Sha256::digest(b"goodfile")),
            bytes: 8,
            version: "1.0.0".into(),
        };
        let client = reqwest::Client::new();
        assert!(download(&client, &base, &artifact, &target).await.is_err());
        assert_eq!(std::fs::read(&target).unwrap(), b"installed");
        artifact.bytes = 1;
        assert!(download(&client, &base, &artifact, &target).await.is_err());
        assert_eq!(std::fs::read(&target).unwrap(), b"installed");
        artifact.bytes = 8;
        artifact.sha256 = hex::encode(Sha256::digest(b"bad file"));
        // Windows rename-over-existing differs; the updater is Linux only.
        std::fs::remove_file(&target).unwrap();
        download(&client, &base, &artifact, &target).await.unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"bad file");
        task.abort();
    }
}
