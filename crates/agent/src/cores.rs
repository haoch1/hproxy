use crate::telemetry::{self, Shared};
use anyhow::{Context, Result};
use hproxy_common::*;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    path::PathBuf,
    process::Stdio,
};
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::{Child, Command},
    task::JoinHandle,
};

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Desired {
    pub revision: i64,
    pub resources: Vec<Resource>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cores: Option<Vec<String>>,
}
struct Process {
    child: Child,
    readers: Vec<JoinHandle<()>>,
    core: String,
    errors: std::sync::Arc<std::sync::Mutex<VecDeque<String>>>,
}
impl Process {
    fn failure(&self, fallback: &str) -> String {
        let detail = self.errors.lock().unwrap().iter().cloned().collect::<Vec<_>>().join("；");
        let occupied = detail.to_ascii_lowercase();
        if occupied.contains("address already in use") || occupied.contains("os error 98") || occupied.contains("os error 10048") {
            format!("{} 监听端口被占用，请修改节点或转发的监听端口后重试：{detail}", self.core)
        } else if detail.is_empty() { format!("{}：{fallback}", self.core) }
        else { format!("{}：{fallback}：{detail}", self.core) }
    }
    async fn stop(mut self) {
        #[cfg(unix)]
        if let Some(pid) = self.child.id() {
            unsafe {
                libc::kill(pid as i32, libc::SIGTERM);
            }
        }
        #[cfg(not(unix))]
        self.child.start_kill().ok();
        if tokio::time::timeout(std::time::Duration::from_secs(3), self.child.wait())
            .await
            .is_err()
        {
            self.child.start_kill().ok();
            self.child.wait().await.ok();
        }
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        for t in self.readers {
            t.abort();
        }
    }
}
fn check_listeners(resources: &[Resource], active: &[Resource]) -> Result<()> {
    for resource in resources {
        let address: std::net::SocketAddr = host_port(&resource.listen_ip, resource.listen_port).parse()?;
        let (tcp, udp) = resource.networks();
        for (protocol, needed) in [("TCP", tcp), ("UDP", udp)] {
            // These listeners belong to the running configuration and will be replaced.
            if !needed || active.iter().any(|old| old.listen_port == resource.listen_port
                && if protocol == "TCP" { old.networks().0 } else { old.networks().1 }) { continue; }
            let result = if protocol == "TCP" {
                std::net::TcpListener::bind(address).map(drop)
            } else { std::net::UdpSocket::bind(address).map(drop) };
            if let Err(error) = result {
                if error.kind() == std::io::ErrorKind::AddrInUse {
                    anyhow::bail!("{protocol} 监听端口 {} 已被占用，请修改端口后重试（{}）", resource.listen_port, resource.name);
                }
                return Err(error).with_context(|| format!("{} 无法监听 {protocol} {address}", resource.name));
            }
        }
    }
    Ok(())
}
pub struct Manager {
    pub dir: PathBuf,
    pub core_dir: PathBuf,
    pub desired: Desired,
    processes: Vec<Process>,
    pub shared: Shared,
    active: BTreeSet<String>,
    pub error: Option<String>,
    pub cleanup_pending: bool,
    pub api_port: u16,
    pub api_secret: String,
}
pub fn sing_config(resources: &[Resource], port: u16, token: &str) -> Value {
    let inbounds=resources.iter().filter(|r|!r.is_forward()).map(|r|match &r.spec{
        Spec::Vless{uuid,private_key,short_id,sni,..}=>json!({"type":"vless","tag":r.id,"listen":r.listen_ip,"listen_port":r.listen_port,"users":[{"name":r.id,"uuid":uuid,"flow":"xtls-rprx-vision"}],"tls":{"enabled":true,"server_name":sni,"reality":{"enabled":true,"handshake":{"server":sni,"server_port":443},"private_key":private_key,"short_id":[short_id]}}}),
        Spec::Shadowsocks{password}=>json!({"type":"shadowsocks","tag":r.id,"listen":r.listen_ip,"listen_port":r.listen_port,"udp_fragment":true,"method":"2022-blake3-aes-128-gcm","password":password}),
        _=>unreachable!()
    }).collect::<Vec<_>>();
    json!({"log":{"level":"warn","timestamp":true},"inbounds":inbounds,"outbounds":[{"type":"direct","tag":"direct"}],"route":{"rules":[{"action":"sniff","sniffer":["http","tls","quic"],"timeout":"300ms"}],"final":"direct"},"services":[{"type":"api","listen":"127.0.0.1","listen_port":port,"secret":token,"dashboard":false}]})
}
pub fn realm_config(resources: &[Resource]) -> Value {
    let endpoints = resources
        .iter()
        .flat_map(realm_endpoints)
        .collect::<Vec<_>>();
    json!({"log":{"level":"warn"},"dns":{"mode":"ipv4_and_ipv6","min_ttl":0,"max_ttl":30,"negative_max_ttl":5},"network":{"tcp_timeout":10,"udp_timeout":30},"endpoints":endpoints})
}
// Realm keeps ownership of the data path. Separate native listeners preserve
// the client's address family for a dual-stack destination, for TCP and UDP.
fn realm_endpoints(r: &Resource) -> Vec<Value> {
    let Spec::Forward {
        remote_host,
        remote_host_v6,
        remote_port,
        network,
    } = &r.spec
    else {
        return vec![];
    };
    let endpoint = |listen: &str, remote: &str, ipv6_only: bool| {
        json!({
            "listen": host_port(listen, r.listen_port), "remote": host_port(remote, *remote_port),
            "network": { "no_tcp": network == "udp", "use_udp": network != "tcp", "ipv6_only": ipv6_only }
        })
    };
    if r.listen_ip == "::" {
        if let Some(v6) = remote_host_v6 {
            return vec![
                endpoint("0.0.0.0", remote_host, false),
                endpoint("::", v6, true),
            ];
        }
    }
    vec![endpoint(&r.listen_ip, remote_host, false)]
}
fn adapt_listener(mut resource: Resource, ipv6_available: bool) -> Resource {
    // Keep the saved dual-stack preference; only adapt the running configuration
    // on hosts whose kernel cannot create an IPv6 listener.
    if resource.listen_ip == "::" && !ipv6_available {
        resource.listen_ip = "0.0.0.0".into();
    }
    resource
}
impl Manager {
    pub fn new(dir: PathBuf, core_dir: PathBuf, shared: Shared) -> Result<Self> {
        let desired: Desired = if dir.join("desired.json").exists() {
            serde_json::from_slice(&std::fs::read(dir.join("desired.json"))?)?
        } else {
            Desired::default()
        };
        shared.lock().unwrap().set_resources(&desired.resources);
        let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
        let port = listener.local_addr()?.port();
        drop(listener);
        Ok(Self {
            dir,
            core_dir,
            desired,
            processes: vec![],
            shared,
            active: Default::default(),
            error: None,
            cleanup_pending: false,
            api_port: port,
            api_secret: secret(32),
        })
    }
    pub fn executable(&self, core: &str) -> PathBuf {
        self.core_dir
            .join(format!("{core}{}", std::env::consts::EXE_SUFFIX))
    }
    pub async fn check(&self) -> Result<()> {
        for (name, arg) in [("sing-box", "version"), ("realm", "--version")] {
            if !self.executable(name).is_file() {
                continue;
            }
            let version = release::core_version(&self.core_dir, name)?;
            let result = tokio::time::timeout(
                std::time::Duration::from_secs(10),
                Command::new(self.executable(name)).arg(arg).output(),
            )
            .await??;
            anyhow::ensure!(
                result.status.success()
                    && String::from_utf8_lossy(&result.stdout).contains(&version),
                "{name} 核心版本不匹配"
            );
        }
        self.validate(&self.effective(&self.desired)).await
    }
    fn effective(&self, d: &Desired) -> Vec<Resource> {
        let ipv6_available = std::net::TcpListener::bind("[::]:0").is_ok();
        let mut c = self.shared.lock().unwrap();
        c.refresh_quotas(now());
        d.resources
            .iter()
            .filter(|r| r.active(c.quota_usage(r).total()))
            .cloned()
            .map(|r| adapt_listener(r, ipv6_available))
            .collect()
    }
    async fn validate(&self, resources: &[Resource]) -> Result<()> {
        validate_resources(resources)?;
        let sing = sing_config(resources, self.api_port, &self.api_secret);
        let realm = realm_config(resources);
        for (core, config, required) in [
            ("sing-box", sing, resources.iter().any(|r| !r.is_forward())),
            ("realm", realm, resources.iter().any(Resource::is_forward)),
        ] {
            if !required {
                continue;
            }
            let bin = self.executable(core);
            anyhow::ensure!(bin.is_file(), "未安装 {core} 核心，请安装匹配的核心包");
            let ver = tokio::time::timeout(
                std::time::Duration::from_secs(10),
                Command::new(&bin)
                    .arg(if core == "realm" {
                        "--version"
                    } else {
                        "version"
                    })
                    .output(),
            )
            .await??;
            let version = String::from_utf8_lossy(&ver.stdout);
            let expected = release::core_version(&self.core_dir, core)?;
            anyhow::ensure!(
                ver.status.success() && version.contains(&expected),
                "{core} 版本不匹配，要求 {}",
                expected
            );
            let file = self.dir.join(format!("{core}.candidate.json"));
            atomic_json(&file, &config)?;
            let args = if core == "sing-box" {
                vec!["check", "-c"]
            } else {
                vec!["--check"]
            };
            let checked = tokio::time::timeout(
                std::time::Duration::from_secs(10),
                Command::new(&bin).args(args).arg(&file).output(),
            )
            .await??;
            anyhow::ensure!(
                checked.status.success(),
                "{core} 配置检查失败：{}",
                String::from_utf8_lossy(&checked.stderr)
                    .chars()
                    .take(2000)
                    .collect::<String>()
            );
        }
        Ok(())
    }
    async fn stop_all(&mut self) {
        for p in self.processes.drain(..) {
            p.stop().await;
        }
        self.active.clear();
    }
    async fn stop_cores(&mut self, cores: &[&str]) {
        let processes = std::mem::take(&mut self.processes);
        for p in processes {
            if cores.contains(&p.core.as_str()) { p.stop().await; }
            else { self.processes.push(p); }
        }
        self.active.retain(|id| self.desired.resources.iter()
            .any(|r| &r.id == id && !cores.contains(&required_core(r))));
    }
    async fn start(&mut self, resources: &[Resource]) -> Result<()> {
        self.start_cores(resources, &CORES).await
    }
    async fn start_cores(&mut self, resources: &[Resource], cores: &[&str]) -> Result<()> {
        let mut statuses = self.shared.lock().unwrap().cores.iter()
            .filter(|s| !cores.contains(&s.core.as_str())).cloned().collect::<Vec<_>>();
        for &core in cores {
            let selected = resources
                .iter()
                .filter(|r| r.is_forward() == (core == "realm"))
                .cloned()
                .collect::<Vec<_>>();
            if selected.is_empty() {
                let installed = self.executable(core).is_file();
                statuses.push(CoreStatus {
                    core: core.into(),
                    state: if installed { "idle" } else { "missing" }.into(),
                    version: if !installed {
                        String::new()
                    } else {
                        release::core_version(&self.core_dir, core)?
                    },
                    error: None,
                });
                continue;
            }
            let config = if core == "realm" {
                realm_config(&selected)
            } else {
                sing_config(&selected, self.api_port, &self.api_secret)
            };
            let file = self.dir.join(format!("{core}.json"));
            atomic_json(&file, &config)?;
            let mut command = Command::new(self.executable(core));
            if core == "sing-box" {
                command.arg("run");
            } else {
                command.env("HPROXY_TELEMETRY", "1");
            }
            command
                .current_dir(&self.dir)
                .arg("-c")
                .arg(&file)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .kill_on_drop(true);
            let mut child = command
                .spawn()
                .with_context(|| format!("启动 {core} 失败"))?;
            let (ready_tx, mut ready_rx) = tokio::sync::mpsc::channel::<String>(1024);
            let epoch = id();
            let mut readers = vec![];
            let mapping = selected
                .iter()
                .flat_map(|r| {
                    realm_endpoints(r)
                        .into_iter()
                        .map(|e| (e["listen"].as_str().unwrap().to_owned(), r.id.clone()))
                })
                .collect::<BTreeMap<_, _>>();
            let stdout = child.stdout.take().unwrap();
            let stderr = child.stderr.take().unwrap();
            let shared = self.shared.clone();
            let stream_epoch = epoch.clone();
            let core_name = core.to_owned();
            readers.push(tokio::spawn(async move {
                let mut lines = BufReader::new(stdout).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    if let Some(ready) = line.strip_prefix("HPROXY_READY ") {
                        ready_tx.send(ready.to_owned()).await.ok();
                    } else if core_name == "realm" {
                        telemetry::realm_line(&line, &stream_epoch, &mapping, &shared);
                    }
                    if !line.starts_with("HPROXY") {
                        tracing::debug!(core=%core_name,message=%line,"核心输出");
                    }
                }
            }));
            let core_name = core.to_owned();
            let errors = std::sync::Arc::new(std::sync::Mutex::new(VecDeque::<String>::new()));
            let captured = errors.clone();
            readers.push(tokio::spawn(async move {
                let mut lines = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    tracing::warn!(core=%core_name,message=%line,"核心日志");
                    let mut errors = captured.lock().unwrap();
                    errors.push_back(line.chars().take(500).collect());
                    if errors.len() > 8 { errors.pop_front(); }
                }
            }));
            if core == "sing-box" {
                readers.push(tokio::spawn(telemetry::sing_stream(
                    self.api_port,
                    self.api_secret.clone(),
                    epoch.clone(),
                    self.shared.clone(),
                )));
            }
            self.processes.push(Process {
                child,
                readers,
                core: core.into(),
                errors,
            });
            if core == "realm" {
                let mut expected = BTreeSet::new();
                for r in &selected {
                    let (tcp, udp) = r.networks();
                    for endpoint in realm_endpoints(r) {
                        let listen = endpoint["listen"].as_str().unwrap();
                        if tcp {
                            expected.insert(format!("tcp {listen}"));
                        }
                        if udp {
                            expected.insert(format!("udp {listen}"));
                        }
                    }
                }
                let ready = tokio::time::timeout(std::time::Duration::from_secs(4), async {
                    while !expected.is_empty() {
                        let ready = ready_rx.recv().await.context("Realm 在端口就绪前退出")?;
                        expected.remove(&ready);
                    }
                    Ok::<_, anyhow::Error>(())
                })
                .await.context("监听端口就绪超时").and_then(|result| result);
                if let Err(error) = ready {
                    tokio::task::yield_now().await;
                    anyhow::bail!("{}", self.processes.last().unwrap().failure(&format!("{error:#}")));
                }
            }
            statuses.push(CoreStatus {
                core: core.into(),
                state: "running".into(),
                version: release::core_version(&self.core_dir, core)?,
                error: None,
            });
        }
        tokio::time::sleep(std::time::Duration::from_millis(800)).await;
        for p in &mut self.processes {
            if p.child.try_wait()?.is_some() { anyhow::bail!("{}", p.failure("核心启动后退出")); }
        }
        for r in resources.iter().filter(|r| cores.contains(&required_core(r))) {
            if r.networks().0 {
                let ip = match r.listen_ip.as_str() {
                    "0.0.0.0" => "127.0.0.1",
                    "::" => "::1",
                    s => s,
                };
                tokio::time::timeout(
                    std::time::Duration::from_secs(2),
                    tokio::net::TcpStream::connect(host_port(ip, r.listen_port)),
                )
                .await
                .context("监听端口就绪超时")??;
            }
        }
        self.active.extend(resources.iter().filter(|r| cores.contains(&required_core(r))).map(|r| r.id.clone()));
        self.shared.lock().unwrap().cores = statuses;
        Ok(())
    }
    fn changed_cores(&self, previous: &[Resource], selected: &[Resource], active: &[Resource]) -> Vec<&'static str> {
        CORES.into_iter().filter(|core| {
            let ids = selected.iter().filter(|r| required_core(r) == *core)
                .map(|r| r.id.clone()).collect::<BTreeSet<_>>();
            let running = active.iter().filter(|r| required_core(r) == *core)
                .map(|r| r.id.clone()).collect::<BTreeSet<_>>();
            self.error.is_some() || ids != running || if *core == "realm" {
                realm_config(&selected) != realm_config(&previous)
            } else {
                sing_config(&selected, self.api_port, &self.api_secret)
                    != sing_config(&previous, self.api_port, &self.api_secret)
            }
        }).collect::<Vec<_>>()
    }
    pub async fn apply(&mut self, next: Desired) -> Result<()> {
        validate_resources(&next.resources)?;
        if let Some(cores) = &next.cores {
            validate_core_policy(cores, &next.resources)?;
        }
        let selected = self.effective(&next);
        let old = self.desired.clone();
        let previous = self.effective(&old);
        let active = previous.iter().filter(|r| self.active.contains(&r.id)).cloned().collect::<Vec<_>>();
        // Metadata (including public IP / share address) does not alter either
        // core's data path. Actual changes restart only the affected core.
        let changed = self.changed_cores(&previous, &selected, &active);
        if changed.is_empty() {
            atomic_json(&self.dir.join("desired.json"), &next)?;
            self.desired = next;
            self.remove_unused_cores()?;
            self.refresh_idle();
            self.finish_cleanup()?;
            return Ok(());
        }
        check_listeners(&selected, &active)?;
        let affected = selected.iter().filter(|r| changed.contains(&required_core(r))).cloned().collect::<Vec<_>>();
        self.validate(&affected).await?;
        self.stop_cores(&changed).await;
        if let Err(e) = self.start_cores(&selected, &changed).await {
            self.stop_cores(&changed).await;
            let rollback = self.start_cores(&previous, &changed).await;
            self.error = Some(format!(
                "应用失败：{e:#}；回退：{}",
                if rollback.is_ok() {
                    "成功"
                } else {
                    "失败，请检查端口与核心"
                }
            ));
            anyhow::bail!("{}", self.error.as_deref().unwrap());
        }
        if let Err(e) = atomic_json(&self.dir.join("desired.json"), &next) {
            self.stop_cores(&changed).await;
            self.start_cores(&previous, &changed).await.ok();
            return Err(e.context("无法持久化配置，已尝试回退"));
        }
        self.desired = next;
        self.error = None;
        self.remove_unused_cores()?;
        self.refresh_idle();
        self.finish_cleanup()?;
        Ok(())
    }
    pub fn finish_cleanup(&mut self) -> Result<()> {
        self.cleanup_pending = true;
        self.clean_runtime_files(&self.effective(&self.desired))?;
        crate::resource_cleanup::perform(self)?;
        self.cleanup_pending = false;
        Ok(())
    }
    fn clean_runtime_files(&self, selected: &[Resource]) -> Result<()> {
        for core in CORES {
            let mut files = vec![format!("{core}.candidate.json"), format!("{core}.candidate.tmp"), format!("{core}.tmp")];
            if !selected.iter().any(|r| required_core(r) == core) { files.push(format!("{core}.json")); }
            for name in files {
                let path = self.dir.join(name);
                match std::fs::remove_file(&path) {
                    Ok(()) => {},
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {},
                    Err(error) => return Err(error).with_context(|| format!("清理旧配置失败：{}", path.display())),
                }
            }
        }
        Ok(())
    }
    pub async fn maintain(&mut self) -> Result<()> {
        let effective = self.effective(&self.desired);
        let ids = effective
            .iter()
            .map(|r| r.id.clone())
            .collect::<BTreeSet<_>>();
        let mut failed = false;
        for p in &mut self.processes {
            if p.child.try_wait()?.is_some() {
                failed = true;
                tracing::error!(core=%p.core,"核心异常退出，准备重启");
            }
        }
        if failed || ids != self.active {
            self.validate(&effective).await?;
            self.stop_all().await;
            self.start(&effective).await?;
        }
        self.refresh_idle();
        Ok(())
    }
    fn refresh_idle(&self) {
        let mut shared = self.shared.lock().unwrap();
        for core in CORES {
            if self.processes.iter().any(|p| p.core == core) {
                continue;
            }
            let installed = self.executable(core).is_file();
            let status = CoreStatus {
                core: core.into(),
                state: if installed { "idle" } else { "missing" }.into(),
                version: if installed {
                    release::core_version(&self.core_dir, core).unwrap_or_default()
                } else {
                    String::new()
                },
                error: None,
            };
            if let Some(old) = shared.cores.iter_mut().find(|c| c.core == core) {
                *old = status;
            } else {
                shared.cores.push(status);
            }
        }
    }
    pub async fn reconcile_cores(&self, config: &crate::Config, next: &Desired) -> Result<()> {
        let Some(cores) = &next.cores else {
            return Ok(());
        };
        validate_core_policy(cores, &next.resources)?;
        for core in cores {
            if !self.executable(core).is_file() {
                crate::update::install_core(config, &self.core_dir, core).await.with_context(|| format!("{core} 安装失败"))?;
            }
        }
        Ok(())
    }
    fn remove_unused_cores(&self) -> Result<()> {
        let Some(cores) = &self.desired.cores else {
            return Ok(());
        };
        for core in CORES {
            if !cores.iter().any(|c| c == core) {
                anyhow::ensure!(
                    !self.processes.iter().any(|p| p.core == core),
                    "{core} 仍在运行，请先删除对应配置"
                );
                let file = self.executable(core);
                if file.is_file() {
                    std::fs::remove_file(file).with_context(|| format!("卸载 {core} 失败"))?;
                    tracing::info!(core, "已卸载闲置核心");
                }
            }
        }
        Ok(())
    }
    pub async fn shutdown(&mut self) {
        self.stop_all().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn public_ip_updates_keep_running_cores_counters_and_quota_baselines() {
        let dir = tempfile::tempdir().unwrap();
        let shared = std::sync::Arc::new(std::sync::Mutex::new(telemetry::Collector::default()));
        let mut manager = Manager::new(dir.path().into(), dir.path().join("cores"), shared.clone()).unwrap();
        let mut node = listener_fixture(18443, "shadowsocks");
        node.quota_bytes = Some(1024);
        node.quota_cycle = Some(QuotaCycle { starts_at:now()-60_000, months:1, days:0 });
        manager.desired = Desired { revision:1, resources:vec![node.clone()], cores:None };
        manager.active.insert(node.id.clone());
        {
            let mut c = shared.lock().unwrap();
            c.counters.insert(node.id.clone(), Counter { upload:123, download:456 });
            c.set_resources(&[node.clone()]);
        }
        let baseline = shared.lock().unwrap().quota_baselines.clone();
        std::fs::write(dir.path().join("sing-box.json"), "unchanged runtime configuration").unwrap();
        let old = node.clone();
        node.public_host = "198.51.100.99".into();
        node.address_bindings.public_host = true;
        // This succeeds without core executables: a metadata-only update must
        // never try to validate/restart the running data path.
        manager.apply(Desired { revision:2, resources:vec![node.clone()], cores:None }).await.unwrap();
        assert_eq!(std::fs::read_to_string(dir.path().join("sing-box.json")).unwrap(), "unchanged runtime configuration");
        assert_eq!(shared.lock().unwrap().counters[&node.id].total(),579);
        assert_eq!(serde_json::to_value(&shared.lock().unwrap().quota_baselines).unwrap(),serde_json::to_value(&baseline).unwrap());
        assert_eq!(manager.desired.resources[0].public_host,"198.51.100.99");
        let mut relay = listener_fixture(19443,"forward");
        let previous = vec![old,relay.clone()];
        if let Spec::Forward { remote_host,.. } = &mut relay.spec { *remote_host = "198.51.100.100".into(); }
        let selected = vec![node,relay];
        assert_eq!(manager.changed_cores(&previous,&selected,&previous),vec!["realm"]);
        assert_eq!(realm_config(&selected)["dns"]["max_ttl"],30);
        assert_eq!(realm_config(&selected)["dns"]["negative_max_ttl"],5);
    }
    fn listener_fixture(port: u16, kind: &str) -> Resource {
        Resource { id:id(),agent_id:id(),name:"port fixture".into(),enabled:true,listen_ip:"127.0.0.1".into(),
            listen_port:port,address_bindings: Default::default(), public_host:"127.0.0.1".into(),quota_bytes:None, quota_cycle: None,expires_at:None,
            spec:Resource::generate(kind).unwrap() }
    }
    #[test]
    fn occupied_tcp_and_udp_ports_explain_how_to_fix_and_allow_owned_listeners() {
        let tcp = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let node = listener_fixture(tcp.local_addr().unwrap().port(),"vless");
        let error = check_listeners(&[node.clone()], &[]).unwrap_err().to_string();
        assert!(error.contains("TCP") && error.contains(&node.listen_port.to_string()) && error.contains("请修改端口"));
        check_listeners(&[node.clone()], &[node]).unwrap();
        let udp = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        let mut forward = listener_fixture(udp.local_addr().unwrap().port(),"forward");
        if let Spec::Forward { network, .. } = &mut forward.spec { *network="udp".into(); }
        let error = check_listeners(&[forward.clone()], &[]).unwrap_err().to_string();
        assert!(error.contains("UDP") && error.contains(&forward.listen_port.to_string()) && error.contains("请修改端口"));
        check_listeners(&[forward.clone()], &[forward]).unwrap();
    }
    #[tokio::test]
    async fn port_conflict_does_not_replace_the_previous_configuration() {
        let dir = tempfile::tempdir().unwrap();
        let mut manager = Manager::new(dir.path().join("data"),dir.path().join("cores"),Default::default()).unwrap();
        manager.apply(Desired { revision:1,resources:vec![],cores:None }).await.unwrap();
        let occupied = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let node = listener_fixture(occupied.local_addr().unwrap().port(),"vless");
        let error = manager.apply(Desired { revision:2,resources:vec![node],cores:None }).await.unwrap_err().to_string();
        assert!(error.contains("已被占用"));
        assert_eq!(manager.desired.revision,1);
        let saved:Desired=serde_json::from_slice(&std::fs::read(manager.dir.join("desired.json")).unwrap()).unwrap();
        assert_eq!(saved.revision,1); assert!(saved.resources.is_empty());
    }
    #[test]
    fn wildcard_listener_falls_back_only_when_ipv6_is_unavailable() {
        let mut r = Resource {
            id: id(),
            agent_id: id(),
            name: "fallback".into(),
            enabled: true,
            listen_ip: "::".into(),
            listen_port: 8443,
            address_bindings: Default::default(), public_host: "192.0.2.1".into(),
            quota_bytes: None, quota_cycle: None,
            expires_at: None,
            spec: Spec::Forward {
                remote_host: "192.0.2.2".into(),
                remote_host_v6: Some("2001:db8::2".into()),
                remote_port: 8443,
                network: "tcp_udp".into(),
            },
        };
        assert_eq!(realm_endpoints(&adapt_listener(r.clone(), false)).len(), 1);
        assert_eq!(adapt_listener(r.clone(), false).listen_ip, "0.0.0.0");
        assert_eq!(realm_endpoints(&adapt_listener(r.clone(), true)).len(), 2);
        assert_eq!(r.listen_ip, "::");
        r.spec = Resource::generate("shadowsocks").unwrap();
        assert_eq!(
            sing_config(&[adapt_listener(r.clone(), false)], 1234, "test")["inbounds"][0]["listen"],
            "0.0.0.0"
        );
        r.listen_ip = "::1".into();
        assert_eq!(adapt_listener(r, false).listen_ip, "::1");
    }
    #[tokio::test]
    async fn unused_core_removal_preserves_identity_counters_and_desired_state() {
        let dir = tempfile::tempdir().unwrap();
        let node = Resource { id:id(),agent_id:id(),name:"kept".into(),enabled:false,
            listen_ip:"127.0.0.1".into(),listen_port:18443,address_bindings: Default::default(), public_host:"192.0.2.1".into(),
            quota_bytes:None, quota_cycle: None,expires_at:None,spec:Resource::generate("shadowsocks").unwrap() };
        let shared = std::sync::Arc::new(std::sync::Mutex::new(telemetry::Collector::default()));
        shared.lock().unwrap().counters.insert(
            node.id.clone(),
            Counter {
                upload: 12,
                download: 34,
            },
        );
        let mut manager = Manager::new(
            dir.path().join("data"),
            dir.path().join("cores"),
            shared.clone(),
        )
        .unwrap();
        std::fs::create_dir_all(&manager.core_dir).unwrap();
        std::fs::write(manager.executable("realm"), b"unused core fixture").unwrap();
        manager
            .apply(Desired {
                revision: 1,
                resources: vec![node.clone()],
                cores: Some(vec!["realm".into(),"sing-box".into()]),
            })
            .await
            .unwrap();
        assert_eq!(
            shared
                .lock()
                .unwrap()
                .cores
                .iter()
                .find(|c| c.core == "realm")
                .unwrap()
                .state,
            "idle"
        );
        manager
            .apply(Desired {
                revision: 2,
                resources: vec![node.clone()],
                cores: Some(vec!["sing-box".into()]),
            })
            .await
            .unwrap();
        assert!(!manager.executable("realm").exists());
        assert_eq!(shared.lock().unwrap().counters[&node.id].total(), 46);
        let restored = Manager::new(manager.dir.clone(), manager.core_dir.clone(), shared).unwrap();
        assert_eq!(restored.desired.revision, 2);
        assert_eq!(restored.desired.cores, Some(vec!["sing-box".into()]));
    }
    #[test]
    fn dual_stack_forward_uses_native_family_matched_listeners() {
        let mut r = Resource {
            id: id(),
            agent_id: id(),
            name: "dual".into(),
            enabled: true,
            listen_ip: "::".into(),
            listen_port: 8000,
            address_bindings: Default::default(), public_host: "relay.example".into(),
            quota_bytes: None, quota_cycle: None,
            expires_at: None,
            spec: Spec::Forward {
                remote_host: "192.0.2.2".into(),
                remote_host_v6: Some("2001:db8::2".into()),
                remote_port: 8443,
                network: "tcp_udp".into(),
            },
        };
        let config = realm_config(&[r.clone()]);
        assert_eq!(config["endpoints"].as_array().unwrap().len(), 2);
        assert_eq!(config["endpoints"][0]["listen"], "0.0.0.0:8000");
        assert_eq!(config["endpoints"][0]["remote"], "192.0.2.2:8443");
        assert_eq!(config["endpoints"][1]["listen"], "[::]:8000");
        assert_eq!(config["endpoints"][1]["remote"], "[2001:db8::2]:8443");
        assert_eq!(config["endpoints"][1]["network"]["ipv6_only"], true);
        assert_eq!(config["endpoints"][1]["network"]["use_udp"], true);
        r.listen_ip = "0.0.0.0".into();
        assert_eq!(
            realm_config(&[r.clone()])["endpoints"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        r.listen_ip = "::".into();
        if let Spec::Forward {
            remote_host_v6,
            remote_host,
            ..
        } = &mut r.spec
        {
            *remote_host_v6 = None;
            *remote_host = "target.example".into();
        }
        let config = realm_config(&[r]);
        assert_eq!(config["endpoints"].as_array().unwrap().len(), 1);
        assert_eq!(config["endpoints"][0]["network"]["ipv6_only"], false);
        assert_eq!(config["dns"]["mode"], "ipv4_and_ipv6");
    }
    #[test]
    fn no_secrets_or_user_data_become_shell_commands() {
        let r = Resource {
            id: id(),
            agent_id: id(),
            name: "name".into(),
            enabled: true,
            listen_ip: "::".into(),
            listen_port: 8000,
            address_bindings: Default::default(), public_host: "test.example".into(),
            quota_bytes: None, quota_cycle: None,
            expires_at: None,
            spec: Resource::generate("vless").unwrap(),
        };
        let c = sing_config(&[r.clone()], 9, "secret");
        assert_eq!(c["inbounds"][0]["tag"], r.id);
        assert_eq!(c["services"][0]["listen"], "127.0.0.1");
        assert_eq!(c["inbounds"][0]["users"][0]["flow"], "xtls-rprx-vision");
    }
}
