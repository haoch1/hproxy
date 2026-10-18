use super::*;
use std::time::{Duration, Instant};

fn log(index: usize) -> ConnectionLog {
    ConnectionLog {
        id: format!("connection-{index:04}"), resource_id: "node".into(), network: "tcp".into(),
        source: "client:1".into(), destination: "target:443".into(), domain: "target".into(),
        started_at: now(), closed_at: None, upload: 10, download: 20, result: "active".into(),
    }
}

#[tokio::test]
async fn log_bursts_wake_reporter_and_drain_bounded_durable_reports_without_five_second_wait() {
    let dir = tempfile::tempdir().unwrap();
    private_dir(&dir.path().join("spool")).unwrap();
    let shared = telemetry::Shared::default();
    let ready = Arc::new(tokio::sync::Notify::new());
    let (stop, receiver) = watch::channel(false);
    let reporter = tokio::spawn(report_loop(dir.path().into(), shared.clone(), ready.clone(), receiver));
    // Let the startup health report finish, so subsequent writes must be event-driven.
    tokio::time::timeout(Duration::from_secs(1), ready.notified()).await.unwrap();
    let started = Instant::now();
    for i in 0..601 { shared.lock().unwrap().record(log(i)); }
    let reports = tokio::time::timeout(Duration::from_millis(1500), async {
        loop {
            ready.notified().await;
            let reports = spool_files(&dir.path().join("spool")).unwrap().into_iter()
                .map(|file| serde_json::from_slice::<Report>(&std::fs::read(file).unwrap()).unwrap())
                .collect::<Vec<_>>();
            if reports.iter().map(|r| r.logs.len()).sum::<usize>() == 601 { break reports; }
        }
    }).await.expect("a burst must drain immediately, not on five-second checkpoints");
    let elapsed = started.elapsed();
    assert!(reports.iter().all(|r| r.logs.len() <= 250));
    let ids = reports.iter().flat_map(|r| r.logs.iter().map(|l| &l.id)).collect::<std::collections::BTreeSet<_>>();
    assert_eq!(ids.len(), 601);
    assert!(shared.lock().unwrap().pending.is_empty());
    // Network reads never consume durable reports; only the matching receipt removes them.
    let first = queued_report(dir.path()).unwrap().unwrap().0;
    assert_eq!(queued_report(dir.path()).unwrap().unwrap().0, first);
    stop.send(true).unwrap();
    reporter.await.unwrap();
    eprintln!("601 connection events persisted in {} ms", elapsed.as_millis());
}

#[test]
fn a_failed_spool_write_preserves_pending_logs_and_cumulative_totals_for_retry() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("spool"), b"blocked directory").unwrap();
    let shared = telemetry::Shared::default();
    shared.lock().unwrap().record(log(1));
    assert!(queue_report(dir.path(), &shared).is_err());
    assert_eq!(shared.lock().unwrap().pending.len(), 1);
    std::fs::remove_file(dir.path().join("spool")).unwrap();
    private_dir(&dir.path().join("spool")).unwrap();
    queue_report(dir.path(), &shared).unwrap();
    let (_, Message::Text(message)) = queued_report(dir.path()).unwrap().unwrap() else { panic!("report frame"); };
    let AgentMessage::Report { report } = serde_json::from_str(&message).unwrap() else { panic!("report payload"); };
    assert_eq!(report.logs.len(), 1);
    assert_eq!(report.logs[0].id, "connection-0001");
    assert_eq!(report.counters["node"].total(), 30);
}

#[tokio::test]
async fn realm_counters_do_not_trigger_live_connection_reports() {
    let shared = telemetry::Shared::default();
    let changed = shared.lock().unwrap().log_changed.clone();
    telemetry::realm_line(
        "HPROXY {\"id\":\"one\",\"listener\":\"0.0.0.0:8443\",\"upload\":10,\"download\":20}",
        "epoch", &std::collections::BTreeMap::from([("0.0.0.0:8443".into(), "forward".into())]), &shared,
    );
    assert_eq!(shared.lock().unwrap().counters["forward"].total(), 30);
    assert!(tokio::time::timeout(Duration::from_millis(30), changed.notified()).await.is_err());
}
