use std::io::Result;
use tokio::net::TcpStream;

#[inline]
pub async fn run_relay(mut local: TcpStream, mut remote: TcpStream) -> Result<()> {
    let counters = crate::telemetry::CURRENT.try_with(|s| (s.up.clone(), s.down.clone())).ok();
    #[cfg(target_os = "linux")]
    {
        use std::io::ErrorKind;
        let a = realm_io::CopyBuffer::new(realm_io::Pipe::new()?).with_counter(counters.as_ref().map(|s| s.0.clone()));
        let b = realm_io::CopyBuffer::new(realm_io::Pipe::new()?).with_counter(counters.as_ref().map(|s| s.1.clone()));
        match realm_io::bidi_copy_buf(&mut local, &mut remote, a, b).await {
            Ok(_) => Ok(()),
            Err(ref e) if e.kind() == ErrorKind::InvalidInput => {
                memory_copy(&mut local, &mut remote, counters).await
            }
            Err(e) => Err(e),
        }
    }

    #[cfg(not(target_os = "linux"))]
    {
        memory_copy(&mut local, &mut remote, counters).await
    }
}

async fn memory_copy(local: &mut TcpStream, remote: &mut TcpStream, counters: Option<(std::sync::Arc<std::sync::atomic::AtomicU64>,std::sync::Arc<std::sync::atomic::AtomicU64>)>) -> Result<()> {
    let a = realm_io::CopyBuffer::new(vec![0u8; realm_io::buf_size()].into_boxed_slice()).with_counter(counters.as_ref().map(|s|s.0.clone()));
    let b = realm_io::CopyBuffer::new(vec![0u8; realm_io::buf_size()].into_boxed_slice()).with_counter(counters.as_ref().map(|s|s.1.clone()));
    realm_io::bidi_copy_buf(local,remote,a,b).await.map(|_|())
}
