use serde::{Deserialize, Serialize};
use std::{
    net::{IpAddr, Ipv4Addr, Ipv6Addr},
    path::Path,
    time::Duration,
};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Addresses {
    pub ipv4: Option<String>,
    pub ipv6: Option<String>,
    #[serde(default)]
    pub country_code: Option<String>,
}
impl Addresses {
    pub fn cached(data: &Path) -> Self {
        std::fs::read(data.join("addresses.json"))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }
    /// Cache the last successful family when a discovery service is unavailable.
    async fn refresh(&self, data: &Path, with_region: bool) -> Self {
        let mut addresses = self.clone();
        async fn probe(v6: bool) -> Option<String> {
            let client = reqwest::Client::builder()
                .no_proxy()
                .local_address(if v6 {
                    IpAddr::V6(Ipv6Addr::UNSPECIFIED)
                } else {
                    IpAddr::V4(Ipv4Addr::UNSPECIFIED)
                })
                .timeout(Duration::from_secs(4))
                .build()
                .ok()?;
            let response = client
                .get(if v6 {
                    "https://api6.ipify.org"
                } else {
                    "https://api.ipify.org"
                })
                .send()
                .await
                .ok()?
                .error_for_status()
                .ok()?;
            if response.content_length().is_some_and(|n| n > 100) {
                return None;
            }
            let text = response.text().await.ok()?;
            let ip: IpAddr = text.trim().parse().ok()?;
            if ip.is_ipv6() != v6 || ip.is_unspecified() || ip.is_loopback() {
                return None;
            }
            Some(ip.to_string())
        }
        async fn region() -> Option<String> {
            let response = reqwest::Client::builder()
                .no_proxy()
                .timeout(Duration::from_secs(4))
                .build()
                .ok()?
                .get("https://www.cloudflare.com/cdn-cgi/trace")
                .send()
                .await
                .ok()?
                .error_for_status()
                .ok()?;
            if response.content_length().is_some_and(|n| n > 4096) {
                return None;
            }
            let text = response.text().await.ok()?;
            let code = text
                .lines()
                .find_map(|line| line.strip_prefix("loc="))?
                .trim();
            (code.len() == 2 && code.bytes().all(|b| b.is_ascii_uppercase()) && code != "XX")
                .then(|| code.to_owned())
        }
        let (v4, v6, country) = tokio::join!(probe(false), probe(true), async {
            if with_region {
                region().await
            } else {
                None
            }
        });
        addresses.merge(v4, v6, country);
        if addresses != *self {
            if let Err(error) = hproxy_common::atomic_json(&data.join("addresses.json"), &addresses)
            {
                tracing::warn!(%error,"无法保存地址缓存");
            }
            tracing::info!(ipv4=?addresses.ipv4,ipv6=?addresses.ipv6,"公网地址已更新");
        }
        addresses
    }
    fn merge(&mut self, v4: Option<String>, v6: Option<String>, country: Option<String>) {
        if v4.is_some() {
            self.ipv4 = v4;
        }
        if v6.is_some() {
            self.ipv6 = v6;
        }
        if country.is_some() {
            self.country_code = country;
        }
    }
}

/// Discovery runs independently of WebSocket heartbeats and keeps its cache
/// fresh even while the controller is unreachable. Reconnect requests a check.
pub fn watch(
    data: std::path::PathBuf,
    mut stop: tokio::sync::watch::Receiver<bool>,
) -> (
    tokio::sync::watch::Receiver<Addresses>,
    std::sync::Arc<tokio::sync::Notify>,
    tokio::task::JoinHandle<()>,
) {
    let (tx, rx) = tokio::sync::watch::channel(Addresses::cached(&data));
    let refresh = std::sync::Arc::new(tokio::sync::Notify::new());
    let requested = refresh.clone();
    let task = tokio::spawn(async move {
        let mut first = true;
        let mut interval = tokio::time::interval(Duration::from_secs(30));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                _ = interval.tick() => {},
                _ = requested.notified() => {},
                _ = stop.changed() => break,
            }
            if *stop.borrow() {
                break;
            }
            let previous = tx.borrow().clone();
            let next = tokio::select! {
                result = previous.refresh(&data, first) => result,
                _ = stop.changed() => break,
            };
            first = false;
            if next != previous && tx.send(next).is_err() {
                break;
            }
        }
    });
    (rx, refresh, task)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn failed_probe_keeps_last_good_family_and_region() {
        let mut a = Addresses {
            ipv4: Some("192.0.2.1".into()),
            ipv6: Some("2001:db8::1".into()),
            country_code: Some("US".into()),
        };
        a.merge(Some("192.0.2.2".into()), None, None);
        assert_eq!(a.ipv4.as_deref(), Some("192.0.2.2"));
        assert_eq!(a.ipv6.as_deref(), Some("2001:db8::1"));
        assert_eq!(a.country_code.as_deref(), Some("US"));
        let old = a.clone();
        a.merge(None, None, None);
        assert_eq!(a, old);
    }
}
