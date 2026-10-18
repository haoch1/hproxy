use std::net::SocketAddr;
use std::sync::{Arc, RwLock};
use std::collections::HashMap;

use tokio::net::UdpSocket;

pub type Association = (SocketAddr, bool);
pub struct SockMap(RwLock<HashMap<Association, Arc<UdpSocket>>>);

impl SockMap {
    pub fn new() -> Self {
        Self(RwLock::new(HashMap::new()))
    }

    #[inline]
    pub fn find(&self, addr: &Association) -> Option<Arc<UdpSocket>> {
        // fetch the lock

        let sockmap = self.0.read().unwrap();

        sockmap.get(addr).cloned()

        // drop the lock
    }

    #[inline]
    pub fn insert(&self, addr: Association, socket: Arc<UdpSocket>) {
        // fetch the lock
        let mut sockmap = self.0.write().unwrap();

        let _ = sockmap.insert(addr, socket);

        // drop the lock
    }

    #[inline]
    pub fn find_or_insert<E, F>(&self, addr: &Association, f: F) -> Result<Arc<UdpSocket>, E>
    where
        F: Fn() -> Result<Arc<UdpSocket>, E>,
    {
        match self.find(addr) {
            Some(x) => Ok(x),
            None => {
                let socket = f()?;
                self.insert(*addr, Arc::clone(&socket));
                Ok(socket)
            }
        }
    }

    #[inline]
    pub fn remove(&self, addr: &Association) {
        // fetch the lock
        let mut sockmap = self.0.write().unwrap();

        let _ = sockmap.remove(addr);

        // drop the lock
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn old_family_timeout_cannot_remove_new_family_association() {
        let map = SockMap::new();
        let client = "127.0.0.1:12345".parse().unwrap();
        let v4 = Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap());
        let v6 = Arc::new(UdpSocket::bind("[::1]:0").await.unwrap());
        map.insert((client,false),v4.clone());
        map.insert((client,true),v6.clone());
        assert!(Arc::ptr_eq(&map.find_or_insert::<(),_>(&(client,false),|| Err(())).unwrap(),&v4));
        map.remove(&(client,false));
        assert!(map.find(&(client,false)).is_none());
        assert!(Arc::ptr_eq(&map.find(&(client,true)).unwrap(),&v6));
    }
}
