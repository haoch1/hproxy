//! Only connection and payload counters are collected. No host resource sampling.
use hproxy_common::*;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, VecDeque};
use std::path::PathBuf;
use tonic::{codegen::http::uri::PathAndQuery, Request};

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Collector {
    pub counters: BTreeMap<String, Counter>,
    #[serde(default)]
    pub quota_baselines: BTreeMap<String, QuotaBaseline>,
    #[serde(skip)]
    cycles: BTreeMap<String, QuotaCycle>,
    #[serde(default)]
    pub connections: BTreeMap<String, ConnectionLog>,
    #[serde(default)]
    closed: VecDeque<String>,
    #[serde(skip)]
    pub pending: BTreeMap<String, ConnectionLog>,
    #[serde(skip)]
    pub log_changed: std::sync::Arc<tokio::sync::Notify>,
    #[serde(default)]
    pub dropped: u64,
    #[serde(skip)]
    pub cores: Vec<CoreStatus>,
    #[serde(default)]
    forward_totals: BTreeMap<String, Counter>,
    #[serde(default)]
    forward_closed: VecDeque<String>,
    #[serde(default)]
    forward_resources: BTreeMap<String, String>,
    #[serde(skip)]
    allowed: Option<std::collections::BTreeSet<String>>,
}
impl Collector {
    pub fn set_resources(&mut self, resources: &[Resource]) {
        self.allowed = Some(resources.iter().map(|r| r.id.clone()).collect());
        self.cycles = resources.iter().filter_map(|r| r.quota_cycle.clone().map(|cycle| (r.id.clone(), cycle))).collect();
        self.quota_baselines.retain(|id, _| self.cycles.contains_key(id));
        self.refresh_quotas(now());
    }
    fn refresh_quota(&mut self, id: &str, at: i64) {
        let Some(cycle) = self.cycles.get(id) else { return; };
        let Some((start, _)) = cycle.window(at) else { return; };
        let total = self.counters.get(id).cloned().unwrap_or_default();
        self.quota_baselines.entry(id.into()).or_insert_with(|| QuotaBaseline {
            cycle: cycle.clone(), start, counter: Counter::default(),
        }).refresh(cycle, &total, at);
    }
    pub fn refresh_quotas(&mut self, at: i64) {
        for id in self.cycles.keys().cloned().collect::<Vec<_>>() { self.refresh_quota(&id, at); }
    }
    pub fn quota_usage(&self, resource: &Resource) -> Counter {
        let total = self.counters.get(&resource.id).cloned().unwrap_or_default();
        if resource.quota_cycle.is_some() {
            if let Some(baseline) = self.quota_baselines.get(&resource.id) { return baseline.usage(&total); }
        }
        total
    }
    pub fn retain_resources(&mut self, resources: &[Resource]) {
        self.set_resources(resources);
        let allowed = self.allowed.as_ref().unwrap();
        self.counters.retain(|resource, _| allowed.contains(resource));
        self.connections.retain(|_, log| allowed.contains(&log.resource_id));
        self.pending.retain(|_, log| allowed.contains(&log.resource_id));
        self.closed.retain(|key| self.connections.contains_key(key));
        self.forward_resources.retain(|_, value| allowed.contains(value));
        self.forward_totals.retain(|key, _| self.forward_resources.contains_key(key));
        self.forward_closed.retain(|key| self.forward_totals.contains_key(key));
    }
    pub fn record(&mut self, log: ConnectionLog) {
        if self.allowed.as_ref().is_some_and(|ids| !ids.contains(&log.resource_id)) { return; }
        self.refresh_quota(&log.resource_id, now());
        let old = self.connections.get(&log.id);
        let du = log.upload.saturating_sub(old.map_or(0, |x| x.upload));
        let dd = log.download.saturating_sub(old.map_or(0, |x| x.download));
        let count = self.counters.entry(log.resource_id.clone()).or_default();
        count.upload = count.upload.saturating_add(du);
        count.download = count.download.saturating_add(dd);
        let newly_closed = log.closed_at.is_some() && old.is_none_or(|o| o.closed_at.is_none());
        if newly_closed {
            self.closed.push_back(log.id.clone());
        }
        if self.pending.len() < 1000 || self.pending.contains_key(&log.id) {
            self.pending.insert(log.id.clone(), log.clone());
            self.log_changed.notify_one();
        } else {
            self.dropped += 1;
        }
        // Keep native sing-box's 1,000 recent closed IDs plus a margin for reconnect deduplication.
        self.connections.insert(log.id.clone(), log);
        while self.closed.len() > 2048 {
            if let Some(key) = self.closed.pop_front() {
                self.connections.remove(&key);
            }
        }
    }
    pub fn snapshot(&mut self) -> Report {
        self.refresh_quotas(now());
        let keys = self.pending.keys().take(250).cloned().collect::<Vec<_>>();
        let logs = keys
            .into_iter()
            .filter_map(|k| self.pending.remove(&k))
            .collect();
        // A large burst drains in bounded reports, without waiting for the periodic checkpoint.
        if !self.pending.is_empty() {
            self.log_changed.notify_one();
        }
        Report {
            id: id(),
            at: now(),
            counters: self.counters.clone(),
            quota_baselines: self.quota_baselines.clone(),
            logs,
            cores: self.cores.clone(),
            dropped_logs: self.dropped,
        }
    }
}
pub type Shared = std::sync::Arc<std::sync::Mutex<Collector>>;

// Wire-compatible subset of sing-box v1.14.3 daemon/started_service.proto.
// Copyright SagerNet, GPL-3.0-or-later; provenance in THIRD_PARTY_NOTICES.md.
#[derive(Clone, PartialEq, prost::Message)]
struct Subscribe {
    #[prost(int64, tag = "1")]
    interval: i64,
}
#[derive(Clone, PartialEq, prost::Message)]
struct Events {
    #[prost(message, repeated, tag = "1")]
    events: Vec<Event>,
    #[prost(bool, tag = "2")]
    reset: bool,
}
#[derive(Clone, PartialEq, prost::Message)]
struct Event {
    #[prost(int32, tag = "1")]
    kind: i32,
    #[prost(string, tag = "2")]
    id: String,
    #[prost(message, optional, tag = "3")]
    connection: Option<Connection>,
    #[prost(int64, tag = "4")]
    up: i64,
    #[prost(int64, tag = "5")]
    down: i64,
    #[prost(int64, tag = "6")]
    closed: i64,
}
#[derive(Clone, PartialEq, prost::Message)]
struct Connection {
    #[prost(string, tag = "1")]
    id: String,
    #[prost(string, tag = "2")]
    inbound: String,
    #[prost(string, tag = "5")]
    network: String,
    #[prost(string, tag = "6")]
    source: String,
    #[prost(string, tag = "7")]
    destination: String,
    #[prost(string, tag = "8")]
    domain: String,
    #[prost(int64, tag = "12")]
    created: i64,
    #[prost(int64, tag = "13")]
    closed: i64,
    #[prost(int64, tag = "16")]
    up: i64,
    #[prost(int64, tag = "17")]
    down: i64,
}
fn apply_event(shared: &Shared, epoch: &str, e: Event) {
    let mut c = shared.lock().unwrap();
    let key = format!("{epoch}:{}", e.id);
    if let Some(conn) = e.connection {
        let log = ConnectionLog {
            id: key,
            resource_id: conn.inbound,
            network: conn.network,
            source: conn.source,
            destination: conn.destination,
            domain: conn.domain,
            started_at: conn.created,
            closed_at: if conn.closed > 0 {
                Some(conn.closed)
            } else {
                None
            },
            upload: conn.up.max(0) as u64,
            download: conn.down.max(0) as u64,
            result: if conn.closed > 0 { "closed" } else { "active" }.into(),
        };
        c.record(log);
    } else if let Some(mut log) = c.connections.get(&key).cloned() {
        log.upload = log.upload.saturating_add(e.up.max(0) as u64);
        log.download = log.download.saturating_add(e.down.max(0) as u64);
        if e.kind == 2 {
            log.closed_at = Some(e.closed);
            log.result = "closed".into();
        }
        c.record(log);
    }
}
pub async fn sing_stream(port: u16, token: String, epoch: String, shared: Shared) {
    loop {
        let result = async {
            let channel =
                tonic::transport::Endpoint::from_shared(format!("http://127.0.0.1:{port}"))?
                    .connect_timeout(std::time::Duration::from_secs(2))
                    .connect()
                    .await?;
            let mut grpc = tonic::client::Grpc::new(channel);
            grpc.ready().await?;
            let mut request = Request::new(Subscribe {
                interval: 250_000_000,
            });
            request
                .metadata_mut()
                .insert("authorization", format!("Bearer {token}").parse()?);
            let response: tonic::Response<tonic::Streaming<Events>> = grpc
                .server_streaming(
                    request,
                    PathAndQuery::from_static("/daemon.StartedService/SubscribeConnections"),
                    tonic_prost::ProstCodec::default(),
                )
                .await?;
            let mut stream = response.into_inner();
            while let Some(batch) = stream.message().await? {
                for e in batch.events {
                    apply_event(&shared, &epoch, e)
                }
            }
            Ok::<_, anyhow::Error>(())
        }
        .await;
        if let Err(e) = result {
            tracing::debug!(error=%e,"sing-box 连接事件流重连");
        }
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }
}
#[derive(Deserialize)]
struct RealmEvent {
    id: String,
    listener: String,
    closed_at: Option<i64>,
    upload: u64,
    download: u64,
}
pub fn realm_line(line: &str, epoch: &str, mapping: &BTreeMap<String, String>, shared: &Shared) {
    if let Some(body) = line.strip_prefix("HPROXY ") {
        if let Ok(e) = serde_json::from_str::<RealmEvent>(body) {
            if let Some(resource) = mapping.get(&e.listener) {
                let key = format!("{epoch}:{}", e.id);
                let mut collector = shared.lock().unwrap();
                if collector.allowed.as_ref().is_some_and(|ids| !ids.contains(resource)) { return; }
                collector.refresh_quota(resource, now());
                collector.forward_resources.insert(key.clone(), resource.clone());
                let previous = collector.forward_totals.get(&key).cloned().unwrap_or_default();
                let total = collector.counters.entry(resource.clone()).or_default();
                total.upload = total.upload.saturating_add(e.upload.saturating_sub(previous.upload));
                total.download = total.download.saturating_add(e.download.saturating_sub(previous.download));
                collector.forward_totals.insert(key.clone(), Counter { upload: e.upload.max(previous.upload), download: e.download.max(previous.download) });
                if e.closed_at.is_some() && !collector.forward_closed.contains(&key) { collector.forward_closed.push_back(key); }
                while collector.forward_closed.len() > 2048 {
                    if let Some(key) = collector.forward_closed.pop_front() { collector.forward_totals.remove(&key); collector.forward_resources.remove(&key); }
                }
            }
        }
    }
}
pub fn load_collector(path: &PathBuf) -> anyhow::Result<Collector> {
    if path.exists() {
        Ok(serde_json::from_slice(&std::fs::read(path)?)?)
    } else {
        Ok(Collector::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn forward_quota_rollover_counts_only_new_payload_and_never_emits_logs() {
        let at = now();
        let resource = Resource { id:id(),agent_id:id(),name:"forward quota".into(),enabled:true,
            listen_ip:"0.0.0.0".into(),listen_port:8443,address_bindings: Default::default(), public_host:"example.com".into(),
            quota_bytes:Some(200),quota_cycle:Some(QuotaCycle {starts_at:at-3_600_000,months:0,days:1}),
            expires_at:None,spec:Resource::generate("forward").unwrap() };
        let shared=Shared::default();
        shared.lock().unwrap().set_resources(&[resource.clone()]);
        let mapping=BTreeMap::from([("0.0.0.0:8443".into(),resource.id.clone())]);
        let event=|up,down|format!("HPROXY {}",serde_json::json!({"id":"connection","listener":"0.0.0.0:8443","closed_at":null,"upload":up,"download":down}));
        realm_line(&event(100,200),"epoch",&mapping,&shared);
        assert!(!resource.active(shared.lock().unwrap().quota_usage(&resource).total()));
        shared.lock().unwrap().refresh_quotas(at+86_400_000);
        realm_line(&event(150,250),"epoch",&mapping,&shared);
        realm_line(&event(150,250),"epoch",&mapping,&shared);
        let mut collector=shared.lock().unwrap();
        assert_eq!(collector.quota_usage(&resource).total(),100);
        assert_eq!(collector.counters[&resource.id].total(),400);
        assert!(resource.active(collector.quota_usage(&resource).total()));
        assert!(collector.snapshot().logs.is_empty());
    }
    #[test]
    fn quota_rollover_preserves_totals_survives_restart_and_never_renews_expiry() {
        let at = now();
        let mut resource = Resource { id:id(), agent_id:id(), name:"quota".into(), enabled:true,
            listen_ip:"0.0.0.0".into(), listen_port:8443, address_bindings: Default::default(), public_host:"example.com".into(),
            quota_bytes:Some(200), quota_cycle:Some(QuotaCycle { starts_at:at-3_600_000, months:0, days:1 }),
            expires_at:None, spec:Resource::generate("shadowsocks").unwrap() };
        let mut collector = Collector::default();
        collector.counters.insert(resource.id.clone(),Counter { upload:100, download:200 });
        collector.set_resources(&[resource.clone()]);
        assert!(!resource.active(collector.quota_usage(&resource).total()));
        collector.refresh_quotas(at+86_400_000);
        assert_eq!(collector.quota_usage(&resource).total(),0);
        assert_eq!(collector.counters[&resource.id].total(),300);
        assert!(resource.active(collector.quota_usage(&resource).total()));
        // Reconnect/restart and a clock moving backward must not reset twice.
        let mut restarted: Collector = serde_json::from_slice(&serde_json::to_vec(&collector).unwrap()).unwrap();
        restarted.set_resources(&[resource.clone()]);
        restarted.counters.get_mut(&resource.id).unwrap().upload += 50;
        assert_eq!(restarted.quota_usage(&resource).total(),50);
        let cycle = resource.quota_cycle.as_mut().unwrap();
        cycle.days = 45;
        restarted.set_resources(&[resource.clone()]);
        assert_eq!(restarted.quota_usage(&resource).total(),50);
        resource.expires_at = Some(at-1);
        assert!(!resource.active(restarted.quota_usage(&resource).total()));
        restarted.retain_resources(&[]);
        assert!(restarted.quota_baselines.is_empty());
        assert!(restarted.snapshot().quota_baselines.is_empty());
    }
    #[test]
    fn realm_counts_payload_without_collecting_connection_details() {
        let shared = Shared::default();
        let mapping = BTreeMap::from([("0.0.0.0:8443".into(), "forward".into())]);
        let event = |upload, download, closed| format!("HPROXY {}", serde_json::json!({"id":"one","listener":"0.0.0.0:8443","source":"client:1","destination":"target:2","network":"tcp","started_at":1,"closed_at":closed,"upload":upload,"download":download,"result":"completed"}));
        realm_line(&event(10, 20, None::<i64>), "epoch", &mapping, &shared);
        realm_line(&event(15, 28, Some(2)), "epoch", &mapping, &shared);
        realm_line(&event(15, 28, Some(2)), "epoch", &mapping, &shared);
        let mut collector = shared.lock().unwrap();
        assert_eq!(collector.counters["forward"].total(), 43);
        assert!(collector.connections.is_empty());
        assert!(collector.snapshot().logs.is_empty());
        assert!(!serde_json::to_string(&*collector).unwrap().contains("target:2"));
    }
    #[test]
    fn cumulative_events_are_idempotent_and_active_deltas_count() {
        let shared = Shared::default();
        let conn = Connection {
            id: "a".into(),
            inbound: "node".into(),
            source: "1.2.3.4:2".into(),
            destination: "test.com:443".into(),
            created: now(),
            up: 5,
            down: 10,
            ..Default::default()
        };
        let new = Event {
            id: "a".into(),
            connection: Some(conn),
            ..Default::default()
        };
        apply_event(&shared, "epoch", new.clone());
        apply_event(&shared, "epoch", new);
        apply_event(
            &shared,
            "epoch",
            Event {
                id: "a".into(),
                kind: 1,
                up: 3,
                down: 4,
                ..Default::default()
            },
        );
        assert_eq!(shared.lock().unwrap().counters["node"].total(), 22);
    }
}
