//! Bounded application logs; journald remains available when no log directory is set.
use anyhow::Result;
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

pub const MAX_BYTES: u64 = 10 * 1024 * 1024;
const ARCHIVES: usize = 3;

#[derive(Clone)]
pub struct RollingLog(Arc<Mutex<State>>);
struct State {
    path: PathBuf,
    file: Option<File>,
    bytes: u64,
    limit: u64,
}
fn open(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}
impl RollingLog {
    pub fn new(directory: &Path, name: &str) -> Result<Self> {
        anyhow::ensure!(
            !name.contains(['/', '\\']) && name.ends_with(".log"),
            "日志文件名无效"
        );
        crate::maintenance::private_dir(directory)?;
        let path = directory.join(name);
        let file = open(&path)?;
        let bytes = file.metadata()?.len();
        Ok(Self(Arc::new(Mutex::new(State {
            path,
            file: Some(file),
            bytes,
            limit: MAX_BYTES,
        }))))
    }
}
impl Write for RollingLog {
    fn write(&mut self, mut bytes: &[u8]) -> io::Result<usize> {
        let total = bytes.len();
        let mut state = self
            .0
            .lock()
            .map_err(|_| io::Error::other("日志锁不可用"))?;
        while !bytes.is_empty() {
            if state.bytes >= state.limit {
                // Close the handle before renaming, including on Windows.
                state.file.take();
                let archive = |n| state.path.with_extension(format!("log.{n}"));
                let oldest = archive(ARCHIVES);
                if oldest.exists() {
                    fs::remove_file(oldest)?;
                }
                for n in (1..ARCHIVES).rev() {
                    let path = archive(n);
                    if path.exists() {
                        fs::rename(path, archive(n + 1))?;
                    }
                }
                fs::rename(&state.path, archive(1))?;
                state.bytes = 0;
            }
            if state.file.is_none() {
                state.file = Some(open(&state.path)?);
            }
            let size = bytes.len().min((state.limit - state.bytes) as usize);
            state.file.as_mut().unwrap().write_all(&bytes[..size])?;
            state.bytes += size as u64;
            bytes = &bytes[size..];
        }
        Ok(total)
    }
    fn flush(&mut self) -> io::Result<()> {
        let mut state = self
            .0
            .lock()
            .map_err(|_| io::Error::other("日志锁不可用"))?;
        if let Some(file) = &mut state.file {
            file.flush()?;
        }
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rotation_is_bounded_even_for_a_single_large_write_and_survives_restart() {
        let dir = tempfile::tempdir().unwrap();
        let mut log = RollingLog::new(dir.path(), "agent.log").unwrap();
        log.0.lock().unwrap().limit = 32;
        log.write_all(&[b'x'; 400]).unwrap();
        log.flush().unwrap();
        drop(log);
        let files = fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap())
            .collect::<Vec<_>>();
        assert_eq!(files.len(), 4);
        assert!(files.iter().all(|f| f.metadata().unwrap().len() <= 32));
        let mut log = RollingLog::new(dir.path(), "agent.log").unwrap();
        log.write_all(b"new message").unwrap();
        assert!(fs::read(dir.path().join("agent.log"))
            .unwrap()
            .ends_with(b"new message"));
        assert!(RollingLog::new(dir.path(), "../other.log").is_err());
    }
}
