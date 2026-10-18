use anyhow::Result;
use serde::{Deserialize, Serialize};

pub const PLATFORMS: &[&str] = &["linux-amd64", "linux-arm64", "linux-armv7"];
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Artifact {
    pub file: String,
    pub sha256: String,
    pub bytes: u64,
    pub version: String,
}
impl Artifact {
    /// Reusing a cached core must not allocate its entire (up to 150 MiB) file.
    pub fn matches_file(&self, path: &std::path::Path) -> Result<bool> {
        use sha2::{Digest, Sha256};
        use std::io::Read;
        let mut file = match std::fs::File::open(path) {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(e) => return Err(e.into()),
        };
        if file.metadata()?.len() != self.bytes { return Ok(false); }
        let mut hash = Sha256::new();
        let mut buffer = [0u8; 64 * 1024];
        let mut size = 0u64;
        loop {
            let n = file.read(&mut buffer)?;
            if n == 0 { break; }
            size += n as u64;
            if size > self.bytes { return Ok(false); }
            hash.update(&buffer[..n]);
        }
        Ok(size == self.bytes && hex::encode(hash.finalize()).eq_ignore_ascii_case(&self.sha256))
    }
}
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Release {
    pub schema: u32,
    pub build: String,
    pub version: String,
    pub agent: Artifact,
    pub sing_box: Artifact,
    pub realm: Artifact,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server: Option<Artifact>,
}
pub fn platform() -> Option<&'static str> {
    if std::env::consts::OS != "linux" {
        return None;
    }
    match std::env::consts::ARCH {
        "x86_64" => Some("linux-amd64"),
        "aarch64" => Some("linux-arm64"),
        "arm" => Some("linux-armv7"),
        _ => None,
    }
}
pub fn valid_hash(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())
}
/// Managed releases may advance one core at a time. Their validated manifest
/// is the version contract; unmanaged installations retain the compiled pins.
pub fn core_version(directory: &std::path::Path, core: &str) -> Result<String> {
    let path = directory.join("release.json");
    if !path.exists() {
        return Ok(match core {
            "sing-box" => crate::SING_VERSION,
            "realm" => crate::REALM_VERSION,
            _ => anyhow::bail!("核心名称无效"),
        }
        .into());
    }
    let release: Release = serde_json::from_slice(&std::fs::read(path)?)?;
    release.validate(platform().unwrap_or(PLATFORMS[0]))?;
    Ok(match core {
        "sing-box" => release.sing_box.version,
        "realm" => release.realm.version,
        _ => anyhow::bail!("核心名称无效"),
    })
}
impl Release {
    pub fn validate(&self, platform: &str) -> Result<()> {
        anyhow::ensure!(
            PLATFORMS.contains(&platform) && self.schema == 1 && valid_hash(&self.build),
            "不支持的更新清单"
        );
        anyhow::ensure!(
            !self.version.is_empty() && self.version.len() < 100,
            "更新版本信息无效"
        );
        let mut artifacts = vec![
            ("hproxy-agent", &self.agent),
            ("sing-box", &self.sing_box),
            ("realm", &self.realm),
        ];
        if let Some(server) = &self.server {
            artifacts.push(("hproxy-server", server));
        }
        for (name, a) in artifacts {
            anyhow::ensure!(
                a.file == format!("{name}-{platform}")
                    && valid_hash(&a.sha256)
                    && a.bytes > 0
                    && a.bytes <= 150 * 1024 * 1024,
                "更新文件信息无效"
            );
            anyhow::ensure!(
                !a.version.is_empty() && a.version.len() < 100,
                "更新版本信息无效"
            );
        }
        anyhow::ensure!(
            self.agent.version == self.version
                && self
                    .server
                    .as_ref()
                    .is_none_or(|a| a.version == self.version),
            "主控与 agent 版本不匹配"
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn large_cached_artifact_verification_fits_a_small_address_space() {
        use std::io::Write;
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("large-core");
        let mut file = std::fs::File::create(&path).unwrap();
        file.set_len(128 * 1024 * 1024).unwrap();
        let mut artifact = Artifact {
            file: "core".into(), bytes: 128 * 1024 * 1024, version: "test".into(),
            sha256: "254BCC3FC4F27172636DF4BF32DE9F107F620D559B20D760197E452B97453917".into(),
        };
        assert!(artifact.matches_file(&path).unwrap());
        artifact.bytes -= 1;
        assert!(!artifact.matches_file(&path).unwrap());
        artifact.bytes += 1;
        file.write_all(&[1]).unwrap();
        file.sync_all().unwrap();
        assert!(!artifact.matches_file(&path).unwrap());
        assert!(!artifact.matches_file(&directory.path().join("missing")).unwrap());
    }
}
