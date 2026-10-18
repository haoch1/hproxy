//! Only installer-owned children are eligible for collection. Never follow links.
use anyhow::{Context, Result};
use std::{
    fs,
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

pub const SERVER_ROOT: &str = "/opt/hproxy/server";
pub const SERVER_DATA: &str = "/var/lib/hproxy/server";
pub const AGENT_ROOT: &str = "/opt/hproxy/agent";
pub const AGENT_DATA: &str = "/var/lib/hproxy/agent";

pub fn private_dir(path: &Path) -> Result<()> {
    fs::create_dir_all(path)?;
    anyhow::ensure!(
        !fs::symlink_metadata(path)?.file_type().is_symlink(),
        "管理目录不能是符号链接"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// Check the resolved target before recursive removal, including on Windows.
pub fn remove_child(parent: &Path, path: &Path) -> Result<()> {
    if !path.try_exists()? {
        return Ok(());
    }
    let parent = fs::canonicalize(parent)?;
    let meta = fs::symlink_metadata(path)?;
    anyhow::ensure!(
        !meta.file_type().is_symlink(),
        "拒绝清理符号链接：{}",
        path.display()
    );
    let target = fs::canonicalize(path)?;
    anyhow::ensure!(
        target.parent() == Some(parent.as_path()),
        "清理目标不在所属目录内"
    );
    if meta.is_dir() {
        fs::remove_dir_all(target)?;
    } else {
        fs::remove_file(target)?;
    }
    Ok(())
}

pub fn current_release(root: &Path) -> Result<PathBuf> {
    let releases = fs::canonicalize(root.join("releases"))?;
    let current = fs::canonicalize(root.join("current"))?;
    anyhow::ensure!(
        current.parent() == Some(releases.as_path()),
        "当前版本不在发布目录内"
    );
    Ok(current)
}

pub fn prune_releases(root: &Path, protected: &[PathBuf]) -> Result<()> {
    let directory = root.join("releases");
    if !directory.try_exists()? {
        return Ok(());
    }
    let base = fs::canonicalize(&directory)?;
    anyhow::ensure!(
        !fs::symlink_metadata(&directory)?.file_type().is_symlink(),
        "发布目录不能是符号链接"
    );
    let mut keep = protected
        .iter()
        .map(fs::canonicalize)
        .collect::<std::io::Result<Vec<_>>>()?;
    // A link may already have switched while its old process is still draining.
    if let Ok(exe) = std::env::current_exe().and_then(fs::canonicalize) {
        if exe.parent().and_then(Path::parent) == Some(base.as_path()) {
            keep.push(exe.parent().unwrap().into());
        }
    }
    anyhow::ensure!(
        keep.iter().all(|p| p.parent() == Some(base.as_path())),
        "保护版本不在发布目录内"
    );
    for entry in fs::read_dir(&directory)? {
        let entry = entry?;
        if entry.file_type()?.is_dir()
            && crate::release::valid_hash(&entry.file_name().to_string_lossy())
            && !keep.contains(&fs::canonicalize(entry.path())?)
        {
            remove_child(&directory, &entry.path())?;
        }
    }
    Ok(())
}

pub fn prune_staging(downloads: &Path) -> Result<()> {
    let staging = downloads.join("staging");
    if !staging.try_exists()? {
        return Ok(());
    }
    anyhow::ensure!(
        !fs::symlink_metadata(&staging)?.file_type().is_symlink(),
        "暂存目录不能是符号链接"
    );
    for entry in fs::read_dir(&staging)? {
        let entry = entry?;
        if entry.file_type()?.is_dir()
            && crate::release::valid_hash(&entry.file_name().to_string_lossy())
        {
            remove_child(&staging, &entry.path())?;
        }
    }
    Ok(())
}

/// Fixed-name atomic-write/download sidecars cannot grow indefinitely after a crash.
/// A full day of grace is longer than the bounded download/check timeouts.
pub fn prune_temporary_files(directory: &Path) -> Result<()> {
    prune_temporary_at(directory, SystemTime::now())
}
fn prune_temporary_at(directory: &Path, now: SystemTime) -> Result<()> {
    if !directory.try_exists()? {
        return Ok(());
    }
    anyhow::ensure!(
        !fs::symlink_metadata(directory)?.file_type().is_symlink(),
        "临时文件目录不能是符号链接"
    );
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let owned = matches!(
            name.as_str(),
            "agent.tmp"
                | "desired.tmp"
                | "counters.tmp"
                | "addresses.tmp"
                | "update-state.tmp"
                | "update-catalog.tmp"
                | "update-previous-desired.tmp"
                | "removal.tmp"
                | "uninstall.tmp"
                | "release.tmp"
                | "hproxy-server.part"
                | "hproxy-agent.part"
                | "sing-box.part"
                | "realm.part"
                | "sing-box.install.part"
                | "realm.install.part"
        ) || [".part", ".part-journal", ".part-wal", ".part-shm"]
            .iter()
            .any(|suffix| {
                name.strip_prefix("before-update-")
                    .and_then(|s| s.strip_suffix(suffix))
                    .is_some_and(crate::release::valid_hash)
            });
        if entry.file_type()?.is_file()
            && owned
            && now
                .duration_since(entry.metadata()?.modified()?)
                .unwrap_or_default()
                >= Duration::from_secs(86400)
        {
            remove_child(directory, &entry.path())?;
        }
    }
    Ok(())
}

/// Manual backups have other names and are never subject to automatic retention.
pub fn prune_backups(data: &Path) -> Result<()> {
    let backups = data.join("backups");
    if !backups.try_exists()? {
        return Ok(());
    }
    anyhow::ensure!(
        !fs::symlink_metadata(&backups)?.file_type().is_symlink(),
        "备份目录不能是符号链接"
    );
    for prefix in ["before-update-", "rebind-"] {
        let mut candidates = vec![];
        for entry in fs::read_dir(&backups)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            let Some(suffix) = name.strip_prefix(prefix) else {
                continue;
            };
            let kind = entry.file_type()?;
            let owned = if prefix == "before-update-" {
                kind.is_file()
                    && suffix
                        .strip_suffix(".db")
                        .is_some_and(crate::release::valid_hash)
            } else {
                kind.is_dir()
                    && !suffix.is_empty()
                    && suffix.bytes().all(|b| b.is_ascii_digit() || b == b'-')
            };
            if owned {
                candidates.push((entry.metadata()?.modified()?, name, entry.path()));
            }
        }
        candidates.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| b.1.cmp(&a.1)));
        for (_, _, path) in candidates.into_iter().skip(3) {
            remove_child(&backups, &path).context("清理自动备份失败")?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn releases_preserve_current_rollback_and_unrelated_files() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("releases");
        let names = ["a".repeat(64), "b".repeat(64), "c".repeat(64)];
        for name in &names {
            fs::create_dir_all(base.join(name)).unwrap();
        }
        fs::create_dir_all(base.join("manual-archive")).unwrap();
        fs::write(dir.path().join("desired.json"), "configuration").unwrap();
        prune_releases(dir.path(), &[base.join(&names[0]), base.join(&names[1])]).unwrap();
        assert!(base.join(&names[0]).exists() && base.join(&names[1]).exists());
        assert!(!base.join(&names[2]).exists());
        assert!(base.join("manual-archive").exists());
        assert_eq!(
            fs::read_to_string(dir.path().join("desired.json")).unwrap(),
            "configuration"
        );
        assert!(prune_releases(dir.path(), &[dir.path().into()]).is_err());
    }
    #[test]
    fn automatic_backups_are_bounded_manual_backups_and_data_are_kept() {
        let dir = tempfile::tempdir().unwrap();
        let backups = dir.path().join("backups");
        fs::create_dir(&backups).unwrap();
        for n in 0..6 {
            fs::write(backups.join(format!("before-update-{n:064x}.db")), "db").unwrap();
            fs::create_dir(backups.join(format!("rebind-{n}-123"))).unwrap();
        }
        fs::write(backups.join("manual.db"), "manual").unwrap();
        fs::write(dir.path().join("hproxy.db"), "live").unwrap();
        prune_backups(dir.path()).unwrap();
        assert_eq!(fs::read_dir(&backups).unwrap().count(), 7);
        assert_eq!(
            fs::read_to_string(backups.join("manual.db")).unwrap(),
            "manual"
        );
        assert_eq!(
            fs::read_to_string(dir.path().join("hproxy.db")).unwrap(),
            "live"
        );
    }
    #[test]
    fn temporary_downloads_are_removed_within_the_staging_directory_only() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("staging").join("f".repeat(64));
        fs::create_dir_all(&path).unwrap();
        fs::write(path.join("core.part"), "partial").unwrap();
        fs::write(dir.path().join("manual.part"), "keep").unwrap();
        prune_staging(dir.path()).unwrap();
        assert!(!path.exists() && dir.path().join("manual.part").exists());
        assert!(remove_child(&dir.path().join("staging"), dir.path()).is_err());
    }
    #[test]
    fn stale_sidecars_are_removed_but_recent_writes_and_business_files_survive() {
        let dir = tempfile::tempdir().unwrap();
        for name in [
            "counters.tmp",
            "sing-box.install.part",
            "manual.part",
            "counters.json",
            "hproxy.db",
        ] {
            fs::write(dir.path().join(name), "keep data").unwrap();
        }
        let sidecar = dir
            .path()
            .join(format!("before-update-{}.part-journal", "a".repeat(64)));
        fs::write(&sidecar, "unfinished database backup").unwrap();
        prune_temporary_files(dir.path()).unwrap();
        assert!(dir.path().join("counters.tmp").exists());
        prune_temporary_at(dir.path(), SystemTime::now() + Duration::from_secs(90000)).unwrap();
        assert!(
            !dir.path().join("counters.tmp").exists()
                && !dir.path().join("sing-box.install.part").exists()
        );
        assert!(dir.path().join("manual.part").exists());
        assert!(!sidecar.exists());
        assert!(dir.path().join("counters.json").exists() && dir.path().join("hproxy.db").exists());
    }
    #[cfg(unix)]
    #[test]
    fn collector_does_not_follow_symlinks_outside_the_installation() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let base = dir.path().join("releases");
        fs::create_dir(&base).unwrap();
        let link = base.join("a".repeat(64));
        std::os::unix::fs::symlink(outside.path(), &link).unwrap();
        prune_releases(dir.path(), &[]).unwrap();
        assert!(link.exists());
        assert!(remove_child(&base, &link).is_err());
        assert!(outside.path().exists());
    }
}
