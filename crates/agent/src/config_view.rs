use anyhow::{Context, Result};
use serde_json::Value;
use std::path::Path;

// Only core-generated files are viewable; enrollment tokens and telemetry are never exposed.
pub fn read(data: &Path, core: &str) -> Result<Value> {
    anyhow::ensure!(hproxy_common::CORES.contains(&core), "核心名称无效");
    let path = data.join(format!("{core}.json"));
    let metadata = std::fs::symlink_metadata(&path).context("尚未生成配置，请先启用对应节点或转发")?;
    anyhow::ensure!(metadata.is_file() && !metadata.file_type().is_symlink(), "配置文件无效");
    anyhow::ensure!(metadata.len() <= 128 * 1024, "配置文件过大，无法在线查看");
    let bytes = std::fs::read(&path)?;
    anyhow::ensure!(bytes.len() <= 128 * 1024, "配置文件过大，无法在线查看");
    let content: Value = serde_json::from_slice(&bytes).context("配置文件不是有效的 JSON")?;
    anyhow::ensure!(content.is_object(), "配置内容无效");
    Ok(content)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reads_only_generated_core_files_and_preserves_json_values() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("sing-box.json"), r#"{"services":[{"secret":"local-secret"}],"inbounds":[{"password":"node-secret"}]}"#).unwrap();
        let content = read(dir.path(), "sing-box").unwrap();
        assert_eq!(content["services"][0]["secret"], "local-secret");
        assert_eq!(content["inbounds"][0]["password"], "node-secret");
        for core in ["../agent", "agent", "desired", "counters", "sing-box.json"] { assert!(read(dir.path(), core).is_err()); }
        assert!(read(dir.path(), "realm").is_err());
        std::fs::write(dir.path().join("realm.json"), "x".repeat(128 * 1024 + 1)).unwrap();
        assert!(read(dir.path(), "realm").is_err());
    }
}
