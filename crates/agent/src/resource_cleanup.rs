use crate::cores::Manager;
use anyhow::{Context, Result};
use hproxy_common::*;
use std::collections::BTreeSet;

/// Reuse the applied configuration, including after an offline reconnect.
/// Shared files retain data for every remaining resource.
pub fn perform(manager: &Manager) -> Result<()> {
    crate::private_dir(&manager.dir.join("spool"))?;
    let allowed: BTreeSet<_> = manager
        .desired
        .resources
        .iter()
        .map(|r| r.id.as_str())
        .collect();
    {
        let mut collector = manager.shared.lock().unwrap();
        collector.retain_resources(&manager.desired.resources);
        atomic_json(&manager.dir.join("counters.json"), &*collector)?;
    }
    for path in crate::spool_files(&manager.dir.join("spool"))? {
        let mut report: Report = serde_json::from_slice(&std::fs::read(&path)?)
            .with_context(|| format!("读取离线队列失败：{}", path.display()))?;
        let before = (report.logs.len(), report.counters.len(), report.quota_baselines.len());
        report
            .logs
            .retain(|log| allowed.contains(log.resource_id.as_str()));
        report
            .counters
            .retain(|id, _| allowed.contains(id.as_str()));
        report.quota_baselines.retain(|id, _| allowed.contains(id.as_str()));
        if before != (report.logs.len(), report.counters.len(), report.quota_baselines.len()) {
            if report.logs.is_empty() && report.counters.is_empty() {
                std::fs::remove_file(&path)?;
            } else {
                atomic_json(&path, &report)?;
            }
        }
    }
    // Rollbacks may retain older parameters, but must never revive a removed node.
    let previous = manager.dir.join("update-previous-desired.json");
    if previous.exists() {
        let mut desired: crate::cores::Desired =
            serde_json::from_slice(&std::fs::read(&previous)?)?;
        let before = desired.resources.len();
        desired
            .resources
            .retain(|r| allowed.contains(r.id.as_str()));
        if before != desired.resources.len() {
            atomic_json(&previous, &desired)?;
        }
    }
    for name in ["desired.tmp", "counters.tmp", "update-previous-desired.tmp"] {
        match std::fs::remove_file(manager.dir.join(name)) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        cores::Desired,
        telemetry::{self, Shared},
    };

    fn resource(port: u16) -> Resource {
        Resource {
            id: id(),
            agent_id: id(),
            name: "fixture".into(),
            enabled: false,
            listen_ip: "127.0.0.1".into(),
            listen_port: port,
            address_bindings: Default::default(), public_host: "192.0.2.1".into(),
            quota_bytes: None, quota_cycle: None,
            expires_at: None,
            spec: Resource::generate("shadowsocks").unwrap(),
        }
    }
    fn log(resource: &Resource) -> ConnectionLog {
        ConnectionLog {
            id: format!("log-{}", resource.id),
            resource_id: resource.id.clone(),
            network: "tcp".into(),
            source: format!("private-source-{}", resource.id),
            destination: "target:443".into(),
            domain: String::new(),
            started_at: now(),
            closed_at: None,
            upload: 10,
            download: 20,
            result: "active".into(),
        }
    }
    async fn fixture() -> (tempfile::TempDir, Manager, Resource, Resource) {
        let dir = tempfile::tempdir().unwrap();
        let mut manager = Manager::new(
            dir.path().join("data"),
            dir.path().join("cores"),
            Shared::default(),
        )
        .unwrap();
        let removed = resource(18001);
        let kept = resource(18002);
        crate::private_dir(&manager.dir.join("spool")).unwrap();
        manager
            .apply(Desired {
                revision: 1,
                resources: vec![removed.clone(), kept.clone()],
                cores: None,
            })
            .await
            .unwrap();
        for r in [&removed, &kept] {
            manager.shared.lock().unwrap().record(log(r));
        }
        let report = manager.shared.lock().unwrap().snapshot();
        crate::spool(&manager.dir.join("spool"), &report, &manager.shared).unwrap();
        (dir, manager, removed, kept)
    }
    #[tokio::test]
    async fn applied_configuration_cleans_local_files_without_losing_other_nodes() {
        let (_dir, mut manager, removed, kept) = fixture().await;
        manager.shared.lock().unwrap().record(log(&removed));
        for name in [
            "sing-box.json",
            "sing-box.candidate.json",
            "sing-box.candidate.tmp",
            "sing-box.tmp",
        ] {
            std::fs::write(manager.dir.join(name), removed.id.as_bytes()).unwrap();
        }
        atomic_json(
            &manager.dir.join("update-previous-desired.json"),
            &manager.desired,
        )
        .unwrap();
        manager
            .apply(Desired {
                revision: 2,
                resources: vec![kept.clone()],
                cores: None,
            })
            .await
            .unwrap();
        assert!(!manager.cleanup_pending);
        let text = std::fs::read_to_string(manager.dir.join("counters.json")).unwrap();
        assert!(!text.contains(&removed.id));
        assert!(text.contains(&kept.id));
        for name in [
            "sing-box.json",
            "sing-box.candidate.json",
            "sing-box.candidate.tmp",
            "sing-box.tmp",
        ] {
            assert!(!manager.dir.join(name).exists());
        }
        let files = crate::spool_files(&manager.dir.join("spool")).unwrap();
        assert_eq!(files.len(), 1);
        let saved: Report = serde_json::from_slice(&std::fs::read(&files[0]).unwrap()).unwrap();
        assert_eq!(saved.logs.len(), 1);
        assert_eq!(saved.logs[0].resource_id, kept.id);
        assert!(!saved.counters.contains_key(&removed.id));
        assert_eq!(saved.counters[&kept.id].total(), 30);
        manager.shared.lock().unwrap().record(log(&removed));
        assert!(!serde_json::to_string(&*manager.shared.lock().unwrap())
            .unwrap()
            .contains(&removed.id));
        let restored = Shared::new(std::sync::Mutex::new(
            telemetry::load_collector(&manager.dir.join("counters.json")).unwrap(),
        ));
        let restarted =
            Manager::new(manager.dir.clone(), manager.core_dir.clone(), restored).unwrap();
        restarted.shared.lock().unwrap().record(log(&removed));
        assert!(!serde_json::to_string(&*restarted.shared.lock().unwrap())
            .unwrap()
            .contains(&removed.id));
        assert!(!std::fs::read_to_string(manager.dir.join("desired.json"))
            .unwrap()
            .contains(&removed.id));
        let previous =
            std::fs::read_to_string(manager.dir.join("update-previous-desired.json")).unwrap();
        assert!(!previous.contains(&removed.id));
        assert!(previous.contains(&kept.id));
    }
    #[tokio::test]
    async fn failed_local_cleanup_retries_using_the_same_applied_configuration() {
        let (_dir, mut manager, _removed, kept) = fixture().await;
        let bad = manager.dir.join("spool/broken.json");
        std::fs::write(&bad, b"broken queued report").unwrap();
        assert!(manager
            .apply(Desired {
                revision: 2,
                resources: vec![kept],
                cores: None
            })
            .await
            .is_err());
        assert!(manager.cleanup_pending);
        std::fs::remove_file(bad).unwrap();
        manager.finish_cleanup().unwrap();
        assert!(!manager.cleanup_pending);
    }
    #[tokio::test]
    async fn removing_the_last_node_or_forward_cleans_its_local_reports() {
        let (_dir, mut manager, removed, mut kept) = fixture().await;
        kept.spec = Resource::generate("forward").unwrap();
        manager
            .apply(Desired {
                revision: 2,
                resources: vec![kept.clone()],
                cores: None,
            })
            .await
            .unwrap();
        assert!(!manager
            .shared
            .lock()
            .unwrap()
            .counters
            .contains_key(&removed.id));
        for name in [
            "realm.json",
            "realm.candidate.json",
            "realm.candidate.tmp",
            "realm.tmp",
        ] {
            std::fs::write(manager.dir.join(name), kept.id.as_bytes()).unwrap();
        }
        manager
            .apply(Desired {
                revision: 3,
                resources: vec![],
                cores: None,
            })
            .await
            .unwrap();
        assert!(crate::spool_files(&manager.dir.join("spool"))
            .unwrap()
            .is_empty());
        assert!(manager.shared.lock().unwrap().counters.is_empty());
        for name in [
            "realm.json",
            "realm.candidate.json",
            "realm.candidate.tmp",
            "realm.tmp",
        ] {
            assert!(!manager.dir.join(name).exists());
        }
    }
}
