use std::io::Result;
use std::net::SocketAddr;
use tokio::net::UdpSocket;

// UDP datagrams can be larger than the link MTU. Keep each datagram intact,
// while bounding a receive batch to 256 KiB even on small agents.
pub const PACKET_SIZE: usize = u16::MAX as usize;
pub const MAX_PACKETS: usize = 4;

#[repr(transparent)]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SockAddrStore {
    #[cfg(all(target_os = "linux", feature = "batched-udp"))]
    inner: realm_io::mmsg::SockAddrStore,

    #[cfg(not(all(target_os = "linux", feature = "batched-udp")))]
    inner: std::net::SocketAddr,
}

impl SockAddrStore {
    pub fn new() -> Self {
        Self {
            #[cfg(all(target_os = "linux", feature = "batched-udp"))]
            inner: realm_io::mmsg::SockAddrStore::new(),

            #[cfg(not(all(target_os = "linux", feature = "batched-udp")))]
            inner: {
                use std::net::{IpAddr, Ipv4Addr};
                SocketAddr::new(IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)), 0)
            },
        }
    }
}

impl Default for SockAddrStore {
    fn default() -> Self {
        Self::new()
    }
}

impl From<SocketAddr> for SockAddrStore {
    fn from(value: SocketAddr) -> Self {
        SockAddrStore { inner: value.into() }
    }
}

impl From<SockAddrStore> for SocketAddr {
    fn from(value: SockAddrStore) -> Self {
        value.inner.into()
    }
}

#[derive(Debug, Clone)]
pub struct Packet {
    pub(super) buf: Box<[u8]>,
    pub(super) addr: SockAddrStore,
    pub(super) cursor: u16,
}

#[derive(Debug, Clone, Copy)]
pub struct PacketRef<'buf, 'addr> {
    buf: &'buf [u8],
    addr: &'addr SockAddrStore,
}

impl Packet {
    pub fn new() -> Self {
        Self {
            buf: vec![0u8; PACKET_SIZE].into_boxed_slice(),
            addr: SockAddrStore::new(),
            cursor: 0u16,
        }
    }

    pub fn ref_with_addr<'a>(&self, addr: &'a SockAddrStore) -> PacketRef<'_, 'a> {
        PacketRef {
            buf: &self.buf[..self.cursor as usize],
            addr,
        }
    }
}

#[cfg(not(all(target_os = "linux", feature = "batched-udp")))]
pub use common::{recv_some, send_all_counted};
#[cfg(not(all(target_os = "linux", feature = "batched-udp")))]
mod common {
    use super::*;
    pub async fn recv_some(sock: &UdpSocket, pkts: &mut [Packet]) -> Result<usize> {
        debug_assert!(!pkts.is_empty());
        let pkt = &mut pkts[0];
        let (bytes, addr) = sock.recv_from(&mut pkt.buf).await?;
        pkt.addr.inner = addr;
        pkt.cursor = bytes as u16;
        Ok(1)
    }

    pub async fn send_all_counted<'a, 'b, I>(sock: &UdpSocket, pkts: I, counter: Option<&std::sync::atomic::AtomicU64>) -> Result<()>
    where
        I: ExactSizeIterator<Item = PacketRef<'a, 'b>>,
    {
        for pkt in pkts {
            let bytes = sock.send_to(pkt.buf, &pkt.addr.inner).await?;
            if let Some(c) = counter { c.fetch_add(bytes as u64, std::sync::atomic::Ordering::Relaxed); }
        }
        Ok(())
    }
}

#[cfg(all(target_os = "linux", feature = "batched-udp"))]
pub use linux::{recv_some, send_all_counted};
#[cfg(all(target_os = "linux", feature = "batched-udp"))]
mod linux {
    use super::*;
    use std::io::{IoSlice, IoSliceMut};
    use std::mem::MaybeUninit;
    use realm_io::mmsg::{MmsgHdr, MmsgHdrMut};
    use realm_io::mmsg::{send_mul_pkts, recv_mul_pkts};

    pub async fn recv_some(sock: &UdpSocket, pkts: &mut [Packet]) -> Result<usize> {
        const MAX_PKTS: usize = MAX_PACKETS;
        debug_assert!(pkts.len() <= MAX_PKTS);

        let pkt_amt = pkts.len();
        let mut iovs: MaybeUninit<[IoSliceMut; MAX_PKTS]> = MaybeUninit::uninit();
        let mut msgs: MaybeUninit<[MmsgHdrMut; MAX_PKTS]> = MaybeUninit::uninit();
        let iovs = unsafe { iovs.assume_init_mut() };
        let msgs = unsafe { msgs.assume_init_mut() };

        for ((pkt, iov), msg) in pkts.iter_mut().zip(iovs.iter_mut()).zip(msgs.iter_mut()) {
            *iov = IoSliceMut::new(&mut pkt.buf);
            *msg = MmsgHdrMut::new()
                .with_addr(&mut pkt.addr.inner)
                .with_iovec(std::slice::from_mut(iov))
        }

        let pkt_amt = recv_mul_pkts(sock, &mut msgs[..pkt_amt]).await?;
        {
            let mut bytes: [u16; MAX_PKTS] = unsafe { std::mem::zeroed() };
            for (msg, byte) in msgs.iter().zip(bytes.iter_mut()).take(pkt_amt) {
                *byte = msg.get_ref().nbytes() as u16
            }

            for (pkt, byte) in pkts.iter_mut().zip(bytes).take(pkt_amt) {
                pkt.cursor = byte
            }
        }
        Ok(pkt_amt)
    }

    pub async fn send_all_counted<'a, 'b, I>(sock: &UdpSocket, pkts: I, counter: Option<&std::sync::atomic::AtomicU64>) -> Result<()>
    where
        I: ExactSizeIterator<Item = PacketRef<'a, 'b>>,
    {
        const MAX_PKTS: usize = MAX_PACKETS;
        debug_assert!(pkts.len() <= MAX_PKTS);

        let pkt_amt = pkts.len();
        let mut iovs: MaybeUninit<[IoSlice; MAX_PKTS]> = MaybeUninit::uninit();
        let mut msgs: MaybeUninit<[MmsgHdr; MAX_PKTS]> = MaybeUninit::uninit();
        let iovs = unsafe { iovs.assume_init_mut() };
        let msgs = unsafe { msgs.assume_init_mut() };
        let mut lengths = [0usize; MAX_PKTS];

        for (index, ((pkt, iov), msg)) in pkts.zip(iovs.iter_mut()).zip(msgs.iter_mut()).enumerate() {
            lengths[index] = pkt.buf.len();
            *iov = IoSlice::new(pkt.buf);
            *msg = MmsgHdr::new()
                .with_addr(&pkt.addr.inner)
                .with_iovec(std::slice::from_ref(iov))
        }

        let mut cursor = 0;
        while cursor < pkt_amt {
            let n = send_mul_pkts(sock, &mut msgs[cursor..pkt_amt]).await?;
            if let Some(c) = counter {
                let bytes: usize = lengths[cursor..cursor+n].iter().sum();
                c.fetch_add(bytes as u64, std::sync::atomic::Ordering::Relaxed);
            }
            cursor += n;
        }
        Ok(())
    }
}

#[allow(dead_code)]
pub async fn send_all<'a,'b,I>(sock:&UdpSocket,pkts:I)->Result<()> where I:ExactSizeIterator<Item=PacketRef<'a,'b>> {send_all_counted(sock,pkts,None).await}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn preserves_empty_and_large_udp_datagrams_and_byte_counts() {
        use std::sync::atomic::{AtomicU64, Ordering};
        let source = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let relay = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let receiver = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let destination = SockAddrStore::from(receiver.local_addr().unwrap());
        let count = AtomicU64::new(0);
        let mut packets = vec![Packet::new(); MAX_PACKETS];
        let mut actual = vec![0; PACKET_SIZE];
        for size in [0, 1, 1501, 8192, 60000, 65507] {
            let expected = vec![42; size];
            source.send_to(&expected, relay.local_addr().unwrap()).await.unwrap();
            let received = tokio::time::timeout(std::time::Duration::from_secs(3), recv_some(&relay, &mut packets)).await.unwrap_or_else(|e| panic!("receiving {size} bytes: {e}")).unwrap();
            assert_eq!(received, 1);
            assert_eq!(packets[0].cursor as usize, size);
            send_all_counted(&relay, packets[..received].iter().map(|p| p.ref_with_addr(&destination)), Some(&count)).await.unwrap();
            let (bytes, _) = tokio::time::timeout(std::time::Duration::from_secs(3), receiver.recv_from(&mut actual)).await.unwrap().unwrap();
            assert_eq!(&actual[..bytes], &expected);
        }
        assert_eq!(count.load(Ordering::Relaxed), 1 + 1501 + 8192 + 60000 + 65507);
    }
}
