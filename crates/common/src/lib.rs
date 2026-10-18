use anyhow::{bail, Context, Result};
use base64::{
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
    Engine,
};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, path::Path};
pub mod release;
pub mod maintenance;
pub mod rolling_log;
pub mod quota;
pub mod address;
pub use address::AddressBindings;
pub use quota::{QuotaBaseline, QuotaCycle};

pub const SING_VERSION: &str = "1.14.3";
pub const REALM_VERSION: &str = "2.9.6-hproxy.1";
pub const MAX_FRAME: usize = 512 * 1024;
pub const CORES: [&str; 2] = ["sing-box", "realm"];
pub fn required_core(r: &Resource) -> &'static str {
    if r.is_forward() {
        "realm"
    } else {
        "sing-box"
    }
}
pub fn validate_core_policy(cores: &[String], resources: &[Resource]) -> Result<()> {
    anyhow::ensure!(
        cores.len() <= 2
            && cores.iter().all(|c| CORES.contains(&c.as_str()))
            && cores
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len()
                == cores.len(),
        "核心选择无效"
    );
    for r in resources {
        anyhow::ensure!(
            cores.iter().any(|c| c == required_core(r)),
            "{} 仍有配置使用，请先删除对应配置",
            required_core(r)
        );
    }
    Ok(())
}
pub fn now() -> i64 {
    chrono::Utc::now().timestamp_millis()
}
pub fn id() -> String {
    uuid::Uuid::new_v4().to_string()
}
pub fn secret(n: usize) -> String {
    let mut b = vec![0; n];
    rand::thread_rng().fill_bytes(&mut b);
    URL_SAFE_NO_PAD.encode(b)
}
pub fn digest(s: &str) -> String {
    hex::encode(Sha256::digest(s.as_bytes()))
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Spec {
    Vless {
        uuid: String,
        private_key: String,
        public_key: String,
        short_id: String,
        sni: String,
    },
    Shadowsocks {
        password: String,
    },
    Forward {
        remote_host: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        remote_host_v6: Option<String>,
        remote_port: u16,
        network: String,
    },
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Resource {
    pub id: String,
    pub agent_id: String,
    pub name: String,
    pub enabled: bool,
    pub listen_ip: String,
    pub listen_port: u16,
    pub public_host: String,
    #[serde(default)]
    pub address_bindings: AddressBindings,
    pub quota_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quota_cycle: Option<QuotaCycle>,
    pub expires_at: Option<i64>,
    pub spec: Spec,
}
impl Resource {
    pub fn generate(kind: &str) -> Result<Spec> {
        Ok(match kind {
            "vless" => {
                let private = x25519_dalek::StaticSecret::from(rand::random::<[u8; 32]>());
                let public = x25519_dalek::PublicKey::from(&private);
                Spec::Vless {
                    uuid: id(),
                    private_key: URL_SAFE_NO_PAD.encode(private.to_bytes()),
                    public_key: URL_SAFE_NO_PAD.encode(public.as_bytes()),
                    short_id: hex::encode(rand::random::<[u8; 4]>()),
                    sni: "www.bing.com".into(),
                }
            }
            "shadowsocks" => Spec::Shadowsocks {
                password: STANDARD.encode(rand::random::<[u8; 16]>()),
            },
            "forward" => Spec::Forward {
                remote_host: "127.0.0.1".into(),
                remote_host_v6: None,
                remote_port: 8443,
                network: "tcp".into(),
            },
            _ => bail!("不支持的协议"),
        })
    }
    pub fn is_forward(&self) -> bool {
        matches!(self.spec, Spec::Forward { .. })
    }
    pub fn networks(&self) -> (bool, bool) {
        match &self.spec {
            Spec::Vless { .. } => (true, false),
            Spec::Shadowsocks { .. } => (true, true),
            Spec::Forward { network, .. } => (network != "udp", network != "tcp"),
        }
    }
    pub fn validate(&self) -> Result<()> {
        uuid::Uuid::parse_str(&self.id).context("节点 ID 无效")?;
        uuid::Uuid::parse_str(&self.agent_id).context("服务器 ID 无效")?;
        if let Some(id) = &self.address_bindings.target_agent_id {
            uuid::Uuid::parse_str(id).context("落地服务器 ID 无效")?;
            anyhow::ensure!(self.is_forward(), "只有转发规则可以关联落地服务器");
        }
        if self.name.trim().is_empty() || self.name.len() > 200 {
            bail!("名称必须为 1–200 字节")
        }
        self.listen_ip
            .parse::<std::net::IpAddr>()
            .context("监听地址必须为 IPv4 或 IPv6 地址")?;
        if self.listen_port == 0 {
            bail!("端口须为 1–65535")
        }
        validate_host(&self.public_host)?;
        if self.quota_bytes == Some(0) {
            bail!("流量额度必须大于零")
        }
        if self.quota_cycle.as_ref().is_some_and(|cycle| !cycle.valid()) {
            bail!("流量周期或开始日期无效")
        }
        match &self.spec {
            Spec::Vless {
                uuid,
                private_key,
                public_key,
                short_id,
                sni,
            } => {
                uuid::Uuid::parse_str(uuid).context("UUID 无效")?;
                let bytes: [u8; 32] = URL_SAFE_NO_PAD
                    .decode(private_key)?
                    .try_into()
                    .map_err(|_| anyhow::anyhow!("Reality 私钥长度无效"))?;
                let derived =
                    x25519_dalek::PublicKey::from(&x25519_dalek::StaticSecret::from(bytes));
                if URL_SAFE_NO_PAD.encode(derived.as_bytes()) != *public_key {
                    bail!("Reality 公私钥不匹配")
                }
                if short_id.is_empty()
                    || short_id.len() > 16
                    || short_id.len() % 2 != 0
                    || hex::decode(short_id).is_err()
                {
                    bail!("short_id 须为 2–16 位偶数长度十六进制")
                }
                validate_host(sni)?;
            }
            Spec::Shadowsocks { password } => {
                if STANDARD.decode(password)?.len() != 16 {
                    bail!("SS 2022 密钥须为 16 字节 Base64")
                }
            }
            Spec::Forward {
                remote_host,
                remote_host_v6,
                remote_port,
                network,
            } => {
                validate_host(remote_host)?;
                if let Some(v6) = remote_host_v6 {
                    remote_host
                        .parse::<std::net::Ipv4Addr>()
                        .context("双栈落地的主地址须为 IPv4")?;
                    v6.parse::<std::net::Ipv6Addr>()
                        .context("双栈落地的 IPv6 地址无效")?;
                }
                if *remote_port == 0 || !["tcp", "udp", "tcp_udp"].contains(&network.as_str()) {
                    bail!("转发端口或网络类型无效")
                }
            }
        }
        Ok(())
    }
    pub fn active(&self, used: u64) -> bool {
        self.enabled
            && self.expires_at.is_none_or(|t| t > now())
            && self.quota_bytes.is_none_or(|q| used < q)
    }
    pub fn link(&self) -> Option<String> {
        let host = host_port(&self.public_host, self.listen_port);
        let name: String = url::form_urlencoded::byte_serialize(self.name.as_bytes()).collect();
        match &self.spec {
            Spec::Vless {
                uuid,
                public_key,
                short_id,
                sni,
                ..
            } => {
                let query = url::form_urlencoded::Serializer::new(String::new())
                    .extend_pairs([
                        ("security", "reality"),
                        ("encryption", "none"),
                        ("pbk", public_key),
                        ("fp", "chrome"),
                        ("type", "tcp"),
                        ("flow", "xtls-rprx-vision"),
                        ("sni", sni),
                        ("sid", short_id),
                    ])
                    .finish();
                Some(format!("vless://{uuid}@{host}?{query}#{name}"))
            }
            Spec::Shadowsocks { password } => Some(format!(
                "ss://{}@{host}#{name}",
                URL_SAFE_NO_PAD.encode(format!("2022-blake3-aes-128-gcm:{password}"))
            )),
            _ => None,
        }
    }
}
pub fn validate_host(s: &str) -> Result<()> {
    if s.is_empty()
        || s.len() > 253
        || s.contains(['/', '\\', '@', '#', '?', ' ', '\n', '\r', '\t'])
    {
        bail!("地址格式无效")
    }
    if s.parse::<std::net::IpAddr>().is_ok() {
        return Ok(());
    }
    if !s.split('.').all(|p| {
        !p.is_empty()
            && p.len() <= 63
            && !p.starts_with('-')
            && !p.ends_with('-')
            && p.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-')
    }) {
        bail!("域名格式无效")
    }
    Ok(())
}
pub fn host_port(host: &str, port: u16) -> String {
    if host.contains(':') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}
pub fn validate_resources(resources: &[Resource]) -> Result<()> {
    if resources.len() > 512 {
        bail!("单服务器最多 512 条配置")
    }
    for (i, r) in resources.iter().enumerate() {
        r.validate()?;
        for other in &resources[..i] {
            let (a, b) = r.networks();
            let (c, d) = other.networks();
            // Conservative conflict check includes dual-stack wildcard listeners.
            if r.id == other.id { bail!("配置 ID 重复：{} / {}", r.name, other.name) }
            if r.enabled
                    && other.enabled
                    && r.listen_port == other.listen_port
                    && ((a && c) || (b && d))
            {
                bail!("监听端口 {} 冲突，请修改端口后重试（{} / {}）", r.listen_port, r.name, other.name)
            }
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Counter {
    pub upload: u64,
    pub download: u64,
}
impl Counter {
    pub fn total(&self) -> u64 {
        self.upload.saturating_add(self.download)
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectionLog {
    pub id: String,
    pub resource_id: String,
    pub network: String,
    pub source: String,
    pub destination: String,
    pub domain: String,
    pub started_at: i64,
    pub closed_at: Option<i64>,
    pub upload: u64,
    pub download: u64,
    pub result: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoreStatus {
    pub core: String,
    pub state: String,
    pub version: String,
    pub error: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Report {
    pub id: String,
    pub at: i64,
    pub counters: BTreeMap<String, Counter>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub quota_baselines: BTreeMap<String, QuotaBaseline>,
    pub logs: Vec<ConnectionLog>,
    pub cores: Vec<CoreStatus>,
    pub dropped_logs: u64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ControllerMessage {
    RemoveAgent { request_id: String },
    ReadConfig {
        request_id: String,
        core: String,
    },
    CheckUpdate {
        generation: i64,
    },
    Desired {
        revision: i64,
        resources: Vec<Resource>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cores: Option<Vec<String>>,
    },
    Receipt {
        id: String,
    },
    Ping,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentMessage {
    Hello {
        version: String,
        platform: String,
        revision: i64,
        #[serde(default)]
        ipv4: Option<String>,
        #[serde(default)]
        ipv6: Option<String>,
        #[serde(default)]
        country_code: Option<String>,
        #[serde(default)]
        update_enabled: bool,
        #[serde(default)]
        update_build: Option<String>,
        #[serde(default)]
        core_management: bool,
        #[serde(default)]
        quota_cycles: bool,
        #[serde(default)]
        component_updates: bool,
        #[serde(default)]
        config_view: bool,
        #[serde(default)]
        cleanup: bool,
        #[serde(default)]
        uninstall: bool,
    },
    Addresses {
        ipv4: Option<String>,
        ipv6: Option<String>,
        #[serde(default)]
        country_code: Option<String>,
    },
    Applied {
        revision: i64,
        ok: bool,
        error: Option<String>,
    },
    Report {
        report: Report,
    },
    Pong,
    Config {
        request_id: String,
        core: String,
        content: Option<serde_json::Value>,
        error: Option<String>,
    },
    UpdateState {
        generation: i64,
        state: String,
        error: Option<String>,
    },
}

/// Durable private JSON writes, including on Windows (rename-over-existing differs).
pub fn atomic_json(path: &Path, value: &impl Serialize) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("tmp");
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut f = options.open(&tmp)?;
    use std::io::Write;
    f.write_all(&serde_json::to_vec(value)?)?;
    f.sync_all()?;
    drop(f);
    #[cfg(windows)]
    if path.exists() {
        std::fs::remove_file(path)?;
    }
    std::fs::rename(tmp, path)?;
    #[cfg(unix)]
    if let Some(parent) = path.parent() {
        std::fs::File::open(parent)?.sync_all()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    pub fn sample(kind: &str) -> Resource {
        Resource {
            id: id(),
            agent_id: id(),
            name: "香港节点".into(),
            enabled: true,
            listen_ip: "::".into(),
            listen_port: 8443,
            address_bindings: Default::default(), public_host: "2001:db8::1".into(),
            quota_bytes: None, quota_cycle: None,
            expires_at: None,
            spec: Resource::generate(kind).unwrap(),
        }
    }
    #[test]
    fn generated_keys_and_ipv6_links_are_valid() {
        for k in ["vless", "shadowsocks"] {
            let r = sample(k);
            r.validate().unwrap();
            assert!(r.link().unwrap().contains("@[2001:db8::1]:8443"));
        }
    }
    #[test]
    fn core_policy_protects_disabled_resources_and_old_protocol_stays_compatible() {
        let mut node = sample("vless");
        node.enabled = false;
        assert!(validate_core_policy(&[], &[node.clone()]).is_err());
        assert!(validate_core_policy(&["sing-box".into()], &[node]).is_ok());
        assert!(validate_core_policy(&["../core".into()], &[]).is_err());
        assert!(validate_core_policy(&["realm".into(), "realm".into()], &[]).is_err());
        let message: ControllerMessage =
            serde_json::from_str(r#"{"type":"desired","revision":1,"resources":[]}"#).unwrap();
        assert!(matches!(
            message,
            ControllerMessage::Desired { cores: None, .. }
        ));
        let hello: AgentMessage = serde_json::from_str(
            r#"{"type":"hello","version":"0.1.0","platform":"linux","revision":0}"#,
        )
        .unwrap();
        assert!(matches!(
            hello,
            AgentMessage::Hello {
                core_management: false,
                ..
            }
        ));
    }
    #[test]
    fn catches_conflicting_protocols() {
        let a = sample("vless");
        let mut b = sample("forward");
        b.agent_id = a.agent_id.clone();
        assert!(validate_resources(&[a.clone(), b.clone()]).is_err());
        if let Spec::Forward { network, .. } = &mut b.spec {
            *network = "udp".into()
        };
        assert!(validate_resources(&[a, b]).is_ok());
    }
    #[test]
    fn quota_and_expiry() {
        let mut r = sample("vless");
        r.quota_bytes = Some(100);
        assert!(r.active(99));
        assert!(!r.active(100));
        r.expires_at = Some(now() - 1);
        assert!(!r.active(0));
    }
    #[test]
    fn no_shell_or_url_addresses() {
        for s in ["a;touch", "https://a", "a\nb", "-bad.com", "bad..com"] {
            assert!(validate_host(s).is_err(), "{s}");
        }
    }
}
