//! hproxy telemetry extension. The payload stays in upstream's native I/O path.
//! No HTTP server or packet inspection. A separate thread emits bounded-frequency
//! cumulative snapshots to the supervising agent over its private stdout pipe.
use std::{collections::HashMap,sync::{Arc,Mutex,OnceLock,atomic::{AtomicU64,AtomicI64,Ordering}},time::{SystemTime,UNIX_EPOCH}};
use serde_json::json;
pub fn now()->i64{SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as i64}
pub struct Session {
    pub up:Arc<AtomicU64>,pub down:Arc<AtomicU64>,id:u64,listener:String,source:String,destination:String,network:&'static str,
    started:i64,closed:AtomicI64,result:Mutex<String>,
}
impl Session {
    pub fn finish(&self,result:String){*self.result.lock().unwrap()=result;self.closed.store(now(),Ordering::Release);}
}
static NEXT:AtomicU64=AtomicU64::new(1);
type Sessions=Arc<Mutex<HashMap<u64,Arc<Session>>>>;
static SESSIONS:OnceLock<Sessions>=OnceLock::new();
fn sessions()->&'static Sessions { SESSIONS.get_or_init(||{
    let map:Sessions=Default::default();let worker=map.clone();
    std::thread::spawn(move||{
        use std::io::Write;
        let mut previous=HashMap::new();
        loop{
            std::thread::sleep(std::time::Duration::from_secs(1));
            let snapshot=worker.lock().unwrap().values().cloned().collect::<Vec<_>>();
            for s in snapshot {
                let up=s.up.load(Ordering::Relaxed);let down=s.down.load(Ordering::Relaxed);let closed=s.closed.load(Ordering::Acquire);
                let current=(up,down,closed);
                if previous.get(&s.id)==Some(&current){continue}
                let line=json!({"id":s.id.to_string(),"listener":s.listener,"source":s.source,"destination":s.destination,"network":s.network,"started_at":s.started,"closed_at":if closed==0{None}else{Some(closed)},"upload":up,"download":down,"result":s.result.lock().unwrap().as_str()});
                if writeln!(std::io::stdout().lock(),"HPROXY {line}").is_err(){return}
                if closed!=0 {worker.lock().unwrap().remove(&s.id);previous.remove(&s.id);}else{previous.insert(s.id,current);}
            }
        }
    });map
})}
pub fn enabled()->bool{std::env::var_os("HPROXY_TELEMETRY").is_some()}
pub fn start(listener:String,source:String,destination:String,network:&'static str)->Arc<Session>{
    let id=NEXT.fetch_add(1,Ordering::Relaxed);
    let s=Arc::new(Session{up:Default::default(),down:Default::default(),id,listener,source,destination,network,started:now(),closed:AtomicI64::new(0),result:Mutex::new("active".into())});
    if enabled(){sessions().lock().unwrap().insert(id,s.clone());}s
}
pub fn ready(listener:std::net::SocketAddr,network:&str){if enabled(){println!("HPROXY_READY {network} {listener}");}}
pub fn flush_on_exit(){
    use std::io::Write;
    if let Some(map)=SESSIONS.get(){let snapshots=map.lock().unwrap().values().cloned().collect::<Vec<_>>();for s in snapshots{s.finish("core_stopped".into());let line=json!({"id":s.id.to_string(),"listener":s.listener,"source":s.source,"destination":s.destination,"network":s.network,"started_at":s.started,"closed_at":now(),"upload":s.up.load(Ordering::Relaxed),"download":s.down.load(Ordering::Relaxed),"result":"core_stopped"});writeln!(std::io::stdout().lock(),"HPROXY {line}").ok();}std::io::stdout().flush().ok();}
}
tokio::task_local!{pub static CURRENT:Arc<Session>;}
type UdpMap=Mutex<HashMap<(std::net::SocketAddr,std::net::SocketAddr,bool),Arc<Session>>>;
static UDP:OnceLock<UdpMap>=OnceLock::new();
pub fn udp_start(local:std::net::SocketAddr,peer:std::net::SocketAddr,family:bool,remote:String)->Arc<Session>{let s=start(local.to_string(),peer.to_string(),remote,"udp");UDP.get_or_init(Default::default).lock().unwrap().insert((local,peer,family),s.clone());s}
pub fn udp_get(local:std::net::SocketAddr,peer:std::net::SocketAddr,family:bool)->Option<Arc<Session>>{UDP.get_or_init(Default::default).lock().unwrap().get(&(local,peer,family)).cloned()}
pub fn udp_close(local:std::net::SocketAddr,peer:std::net::SocketAddr,family:bool){if let Some(s)=UDP.get_or_init(Default::default).lock().unwrap().remove(&(local,peer,family)){s.finish("closed".into());}}
