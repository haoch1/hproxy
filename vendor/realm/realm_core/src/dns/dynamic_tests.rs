use super::*;
use std::sync::{
    Arc,
    atomic::{AtomicU8, Ordering},
};
use std::time::Duration;
use tokio::net::UdpSocket;

#[tokio::test]
async fn cached_domain_refreshes_ip_and_family_without_recreating_resolver() {
    // A local authoritative fixture advertises a day-long TTL. The resolver's
    // configured cap must still make changed addresses visible while it runs.
    let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let address = socket.local_addr().unwrap();
    let state = Arc::new(AtomicU8::new(0));
    let answers = state.clone();
    let task = tokio::spawn(async move {
        let mut query = [0; 2048];
        loop {
            let (len, peer) = socket.recv_from(&mut query).await.unwrap();
            let mut end = 12;
            while end < len && query[end] != 0 {
                end += query[end] as usize + 1;
            }
            end += 1;
            let kind = u16::from_be_bytes([query[end], query[end + 1]]);
            let version = answers.load(Ordering::Relaxed);
            let record: Option<Vec<u8>> = match (kind, version) {
                (1, 0) => Some(vec![127, 0, 0, 1]),
                (1, 1) => Some(vec![127, 0, 0, 2]),
                (28, 2) => Some(std::net::Ipv6Addr::LOCALHOST.octets().to_vec()),
                _ => None,
            };
            let mut response = query[..2].to_vec();
            response.extend_from_slice(&[0x81, 0x80, 0, 1, 0, u8::from(record.is_some()), 0, 0, 0, 0]);
            response.extend_from_slice(&query[12..end + 4]);
            if let Some(record) = record {
                response.extend_from_slice(&[0xc0, 0x0c]);
                response.extend_from_slice(&kind.to_be_bytes());
                response.extend_from_slice(&[0, 1]);
                response.extend_from_slice(&86400u32.to_be_bytes());
                response.extend_from_slice(&(record.len() as u16).to_be_bytes());
                response.extend_from_slice(&record);
            }
            socket.send_to(&response, peer).await.unwrap();
        }
    });
    let ns = config::NameServerConfig::new(
        address.ip(),
        true,
        vec![{
            let mut c = config::ConnectionConfig::udp();
            c.port = address.port();
            c
        }],
    );
    let conf = ResolverConfig::from_parts(None, vec![], vec![ns]);
    let mut opts = ResolverOpts::default();
    opts.ip_strategy = config::LookupIpStrategy::Ipv4AndIpv6;
    opts.positive_max_ttl = Some(Duration::from_secs(1));
    opts.negative_max_ttl = Some(Duration::from_secs(1));
    opts.timeout = Duration::from_secs(2);
    build(Some(conf), Some(opts));
    let remote = RemoteAddr::DomainName("dynamic.example.".into(), 8443);
    let lookup = || async {
        tokio::time::timeout(Duration::from_secs(3), resolve_addr(&remote))
            .await
            .unwrap()
            .unwrap()
            .iter()
            .collect::<Vec<_>>()
    };
    assert_eq!(lookup().await, vec!["127.0.0.1:8443".parse::<SocketAddr>().unwrap()]);
    state.store(1, Ordering::Relaxed);
    assert_eq!(lookup().await[0].ip(), "127.0.0.1".parse::<std::net::IpAddr>().unwrap());
    tokio::time::sleep(Duration::from_millis(1200)).await;
    assert_eq!(lookup().await, vec!["127.0.0.2:8443".parse::<SocketAddr>().unwrap()]);
    state.store(2, Ordering::Relaxed);
    tokio::time::sleep(Duration::from_millis(1200)).await;
    assert_eq!(lookup().await, vec!["[::1]:8443".parse::<SocketAddr>().unwrap()]);
    task.abort();
}
