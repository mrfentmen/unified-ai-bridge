use anyhow::{Context, Result};
use fs2::FileExt;
use rand::rngs::OsRng;
use rand::RngCore;
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

pub fn sha256_file(path: &Path) -> Result<(String, u64)> {
    let mut file =
        File::open(path).with_context(|| format!("open {} for hashing", path.display()))?;
    let mut hasher = Sha256::new();
    let mut total = 0u64;
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        total += read as u64;
    }
    Ok((hex::encode(hasher.finalize()), total))
}

pub fn sha256_bytes(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// Acquire a cross-process advisory lock keyed by a workspace-relative path.
/// Tools never block indefinitely on a path that another agent is editing.
pub struct PathLock {
    _file: File,
}

impl PathLock {
    pub fn acquire(lock_dir: &Path, key: &str) -> Result<Self> {
        fs::create_dir_all(lock_dir)
            .with_context(|| format!("create lock directory {}", lock_dir.display()))?;
        let digest = sha256_bytes(key.as_bytes());
        let path = lock_dir.join(format!("{}.lock", &digest[..32]));
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .open(&path)
            .with_context(|| format!("open path lock {}", path.display()))?;
        file.try_lock_exclusive()
            .with_context(|| format!("lock {}", path.display()))?;
        Ok(Self { _file: file })
    }
}

/// Keep a hard-link checkpoint of the old inode before an atomic rename
/// replaces it. If the filesystem rejects hard links, fall back to copying.
pub fn checkpoint(
    checkpoint_dir: &Path,
    relative: &Path,
    source: &Path,
    stamp: &str,
) -> Result<Option<PathBuf>> {
    if !source.is_file() {
        return Ok(None);
    }
    fs::create_dir_all(checkpoint_dir)?;
    let flat = relative
        .to_string_lossy()
        .replace(['/', '\\'], "__")
        .replace(|c: char| c.is_control() || c == ' ', "_");
    let flat: String = flat.chars().take(120).collect();
    let digest = sha256_file(source)?.0;
    let target = checkpoint_dir.join(format!(
        "{stamp}__{}__{}.bak",
        &digest[..12],
        if flat.is_empty() { "file" } else { &flat }
    ));
    if target.exists() {
        return Ok(Some(target));
    }
    match fs::hard_link(source, &target) {
        Ok(()) => {}
        Err(_) => {
            fs::copy(source, &target)
                .with_context(|| format!("copy checkpoint for {}", source.display()))?;
        }
    }
    if let Ok(file) = File::open(&target) {
        let _ = file.sync_all();
    }
    Ok(Some(target))
}

#[derive(Debug)]
pub struct AtomicWrite {
    pub sha256: String,
    pub bytes: u64,
    pub checkpoint: Option<PathBuf>,
}

pub fn atomic_write(
    checkpoint_dir: &Path,
    lock_dir: &Path,
    relative: &Path,
    target: &Path,
    content: &[u8],
    create_only: bool,
) -> Result<AtomicWrite> {
    let key = relative.to_string_lossy().to_string();
    let _lock = PathLock::acquire(lock_dir, &key)?;
    atomic_write_locked(checkpoint_dir, relative, target, content, create_only)
}

/// Caller must already hold the path lock. Used by read-modify-write edits so
/// the read, replacement, checkpoint, and rename are one critical section.
pub fn atomic_write_locked(
    checkpoint_dir: &Path,
    relative: &Path,
    target: &Path,
    content: &[u8],
    create_only: bool,
) -> Result<AtomicWrite> {
    if create_only && target.exists() {
        anyhow::bail!("refusing to replace an existing file because create_only is set");
    }
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("create parent directory {}", parent.display()))?;
    }
    let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%S%.3fZ").to_string();
    let checkpoint_path = if target.exists() {
        let meta = fs::symlink_metadata(target)?;
        if meta.file_type().is_symlink() || !meta.is_file() {
            anyhow::bail!("target is not a regular file: {}", target.display());
        }
        checkpoint(checkpoint_dir, relative, target, &stamp)?
    } else {
        None
    };

    let file_name = target
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("file");
    let mut random = [0u8; 8];
    OsRng.fill_bytes(&mut random);
    let temp = target.with_file_name(format!(
        ".{file_name}.uab-{}-{}.tmp",
        std::process::id(),
        hex::encode(random)
    ));
    let write_result = (|| -> Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)
            .with_context(|| format!("create temporary file {}", temp.display()))?;
        file.write_all(content)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temp, target)
            .with_context(|| format!("atomically replace {}", target.display()))?;
        if let Some(parent) = target.parent() {
            if let Ok(dir) = File::open(parent) {
                let _ = dir.sync_all();
            }
        }
        Ok(())
    })();
    if write_result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    write_result?;

    Ok(AtomicWrite {
        sha256: sha256_bytes(content),
        bytes: content.len() as u64,
        checkpoint: checkpoint_path,
    })
}

/// Convert an absolute checkpoint path to a workspace-relative display path.
pub fn display_relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn atomic_write_checkpoints_previous_content() {
        let temp = tempdir().unwrap();
        let root = temp.path();
        let target = root.join("notes/a.md");
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(&target, b"old").unwrap();
        let write = atomic_write(
            &root.join("checkpoints"),
            &root.join("locks"),
            Path::new("notes/a.md"),
            &target,
            b"new",
            false,
        )
        .unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"new");
        assert_eq!(write.bytes, 3);
        let checkpoint = write.checkpoint.unwrap();
        assert_eq!(std::fs::read(checkpoint).unwrap(), b"old");
    }

    #[test]
    fn create_only_keeps_existing_file() {
        let temp = tempdir().unwrap();
        let target = temp.path().join("x.md");
        std::fs::write(&target, b"keep").unwrap();
        let err = atomic_write(
            &temp.path().join("checkpoints"),
            &temp.path().join("locks"),
            Path::new("x.md"),
            &target,
            b"replace",
            true,
        )
        .unwrap_err();
        assert!(err.to_string().contains("create_only"));
        assert_eq!(std::fs::read(&target).unwrap(), b"keep");
    }
}
