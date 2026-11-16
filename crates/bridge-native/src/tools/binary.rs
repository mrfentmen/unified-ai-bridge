//! Descriptor-anchored, create-only binary output shared by archives and images.
use super::{
    artifacts::{directory, open_at},
    ToolContext,
};
use crate::{
    failure::ToolFailure,
    sandbox::{Access, ResolvedPath},
};
use rand::RngCore;
use serde::Serialize;
use serde_json::json;
use std::{
    ffi::{CString, OsStr},
    fs::File,
    io::{Read, Write},
    os::{
        fd::AsRawFd,
        unix::{ffi::OsStrExt, fs::MetadataExt},
    },
    path::Path,
    time::Instant,
};
const MAX_FILE: u64 = 16 * 1024 * 1024;
fn invalid(s: impl Into<String>) -> ToolFailure {
    ToolFailure::InvalidArguments(s.into())
}
fn io(e: impl std::fmt::Display) -> ToolFailure {
    ToolFailure::Io(e.to_string())
}
fn unique() -> String {
    let mut b = [0u8; 16];
    rand::rngs::OsRng.fill_bytes(&mut b);
    hex::encode(b)
}
fn cstr(s: &OsStr) -> Result<CString, ToolFailure> {
    CString::new(s.as_bytes()).map_err(io)
}
fn deadline(end: Instant) -> Result<(), ToolFailure> {
    if Instant::now() >= end {
        Err(invalid(
            "archive operation exceeded its 15-second processing budget",
        ))
    } else {
        Ok(())
    }
}

/// Read through descriptor-walked ancestors and a no-follow leaf; cap memory
/// even if a local writer grows the file after metadata was inspected.
pub(super) fn read_source(
    ctx: &ToolContext<'_>,
    tool: &str,
    raw: &str,
    limit: u64,
) -> Result<Vec<u8>, ToolFailure> {
    let path = ctx.resolve_existing(raw)?;
    ctx.check_read(tool, &path)?;
    if path.relative.components().any(|p| {
        p.as_os_str()
            .to_string_lossy()
            .eq_ignore_ascii_case(".bridge")
            || p.as_os_str().to_string_lossy().eq_ignore_ascii_case(".git")
    }) {
        return Err(ToolFailure::PermissionDenied(
            "binary sources exclude bridge metadata and Git internals".into(),
        ));
    }
    let parent = directory(
        path.absolute
            .parent()
            .ok_or_else(|| invalid("source needs a parent"))?,
    )?;
    let input = open_at(
        &parent,
        path.absolute
            .file_name()
            .ok_or_else(|| invalid("source needs a filename"))?,
        libc::O_RDONLY | libc::O_NONBLOCK,
    )?;
    let metadata = input.metadata().map_err(io)?;
    if !metadata.is_file() || metadata.nlink() != 1 {
        return Err(ToolFailure::PermissionDenied(
            "binary sources must be regular files without hard links".into(),
        ));
    }
    if metadata.len() > limit {
        return Err(invalid(format!("source exceeds {limit} bytes")));
    }
    let mut bytes = Vec::new();
    input.take(limit + 1).read_to_end(&mut bytes).map_err(io)?;
    if bytes.len() as u64 > limit {
        return Err(invalid("source grew beyond its byte limit"));
    }
    Ok(bytes)
}

#[derive(Clone, Serialize)]
pub(super) struct Entry {
    pub(super) path: String,
    pub(super) directory: bool,
    pub(super) bytes: u64,
}

pub(super) fn target(
    ctx: &ToolContext<'_>,
    tool: &str,
    raw: &str,
) -> Result<ResolvedPath, ToolFailure> {
    let path = ctx.resolve_any(raw)?;
    if path.relative.components().any(|part| {
        let name = part.as_os_str().to_string_lossy();
        name.eq_ignore_ascii_case(".bridge")
            || name.eq_ignore_ascii_case(".git")
            || name.chars().any(char::is_control)
    }) {
        return Err(ToolFailure::PermissionDenied(
            "binary destinations exclude bridge metadata, Git internals and control characters"
                .into(),
        ));
    }
    ctx.check_read(tool, &path)?;
    ctx.recheck_access(Access::Write, &path, true)?;
    if path.exists {
        return Err(ToolFailure::Conflict(
            "binary destination must be new; overwrites are refused".into(),
        ));
    }
    directory(
        path.absolute
            .parent()
            .ok_or_else(|| invalid("destination needs an existing parent"))?,
    )?;
    Ok(path)
}

/// Descriptor-anchored staging. Cleanup names only files/directories created by
/// this operation. Publication never replaces a destination, including symlinks.
pub(super) struct Stage {
    parent: File,
    pub(super) name: CString,
    pub(super) file: File,
    children: Vec<(File, CString, bool)>,
    is_dir: bool,
    published: bool,
}
impl Stage {
    pub(super) fn new(parent: File, is_dir: bool) -> Result<Self, ToolFailure> {
        let name = cstr(OsStr::new(&format!(".bridge-archive-{}", unique())))?;
        let file = if is_dir {
            // SAFETY: parent descriptor and NUL-terminated owned name are valid.
            if unsafe { libc::mkdirat(parent.as_raw_fd(), name.as_ptr(), 0o700) } != 0 {
                return Err(io(std::io::Error::last_os_error()));
            }
            match open_at(
                &parent,
                OsStr::from_bytes(name.as_bytes()),
                libc::O_RDONLY | libc::O_DIRECTORY,
            ) {
                Ok(file) => file,
                Err(e) => {
                    unsafe {
                        libc::unlinkat(parent.as_raw_fd(), name.as_ptr(), libc::AT_REMOVEDIR);
                    };
                    return Err(e);
                }
            }
        } else {
            open_at(
                &parent,
                OsStr::from_bytes(name.as_bytes()),
                libc::O_RDWR | libc::O_CREAT | libc::O_EXCL,
            )?
        };
        Ok(Self {
            parent,
            name,
            file,
            children: vec![],
            is_dir,
            published: false,
        })
    }
    fn child_dir(&mut self, path: &Path) -> Result<File, ToolFailure> {
        let mut dir = self.file.try_clone().map_err(io)?;
        for component in path.components() {
            let name = cstr(component.as_os_str())?;
            let cleanup_parent = dir.try_clone().map_err(io)?;
            // SAFETY: descriptor and basename live throughout mkdirat.
            let created = unsafe { libc::mkdirat(dir.as_raw_fd(), name.as_ptr(), 0o700) } == 0;
            if !created
                && std::io::Error::last_os_error().kind() != std::io::ErrorKind::AlreadyExists
            {
                return Err(io(std::io::Error::last_os_error()));
            }
            if created {
                self.children.push((cleanup_parent, name, false));
            }
            dir = open_at(
                &dir,
                component.as_os_str(),
                libc::O_RDONLY | libc::O_DIRECTORY,
            )?;
        }
        Ok(dir)
    }
    pub(super) fn write_entry(
        &mut self,
        entry: &Entry,
        reader: &mut impl Read,
        end: Instant,
    ) -> Result<String, ToolFailure> {
        let path = Path::new(&entry.path);
        if entry.directory {
            self.child_dir(path)?;
            return Ok(String::new());
        }
        let dir = self.child_dir(path.parent().unwrap_or(Path::new("")))?;
        let basename = path
            .file_name()
            .ok_or_else(|| invalid("missing ZIP filename"))?;
        let mut output = open_at(
            &dir,
            basename,
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL,
        )?;
        self.children.push((dir, cstr(basename)?, true));
        use sha2::{Digest, Sha256};
        let mut hash = Sha256::new();
        let mut total = 0u64;
        let mut buf = [0u8; 65536];
        loop {
            deadline(end)?;
            let n = reader.read(&mut buf).map_err(io)?;
            if n == 0 {
                break;
            }
            total += n as u64;
            if total > entry.bytes || total > MAX_FILE {
                return Err(invalid("ZIP expanded data exceeds its declared size"));
            }
            output.write_all(&buf[..n]).map_err(io)?;
            hash.update(&buf[..n]);
        }
        if total != entry.bytes {
            return Err(invalid("ZIP entry size does not match its data"));
        }
        output.sync_all().map_err(io)?;
        Ok(hex::encode(hash.finalize()))
    }
    pub(super) fn publish(&mut self, destination: &Path) -> Result<(), ToolFailure> {
        let current = directory(
            destination
                .parent()
                .ok_or_else(|| invalid("missing parent"))?,
        )?;
        let a = current.metadata().map_err(io)?;
        let b = self.parent.metadata().map_err(io)?;
        if a.dev() != b.dev() || a.ino() != b.ino() {
            return Err(ToolFailure::Conflict(
                "destination parent changed during binary operation".into(),
            ));
        }
        for (parent, _, _) in &self.children {
            parent.sync_all().map_err(io)?;
        }
        self.file.sync_all().map_err(io)?;
        let name = cstr(
            destination
                .file_name()
                .ok_or_else(|| invalid("missing destination"))?,
        )?;
        // SAFETY: source and destination basenames are anchored to our held
        // directory. Both OS variants atomically refuse any existing name.
        #[cfg(target_os = "macos")]
        let result = unsafe {
            libc::renameatx_np(
                self.parent.as_raw_fd(),
                self.name.as_ptr(),
                self.parent.as_raw_fd(),
                name.as_ptr(),
                libc::RENAME_EXCL,
            )
        };
        #[cfg(target_os = "linux")]
        let result = unsafe {
            libc::renameat2(
                self.parent.as_raw_fd(),
                self.name.as_ptr(),
                self.parent.as_raw_fd(),
                name.as_ptr(),
                libc::RENAME_NOREPLACE,
            )
        };
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        let result = -1;
        if result != 0 {
            return Err(io(std::io::Error::last_os_error()));
        }
        self.published = true;
        self.parent.sync_all().map_err(|e|io(format!("binary output published; directory sync failed, inspect destination before retrying: {e}")))?;
        Ok(())
    }
    pub(super) fn cleanup(&mut self) -> std::io::Result<()> {
        if self.published {
            return Ok(());
        }
        let mut error = None;
        for (dir, name, is_file) in self.children.drain(..).rev() {
            // SAFETY: only tracked basenames in their original held parents
            // are removed. No recursive path walk or symlink following.
            if unsafe {
                libc::unlinkat(
                    dir.as_raw_fd(),
                    name.as_ptr(),
                    if is_file { 0 } else { libc::AT_REMOVEDIR },
                )
            } != 0
            {
                let e = std::io::Error::last_os_error();
                if e.kind() != std::io::ErrorKind::NotFound {
                    error = Some(e);
                }
            }
        }
        if unsafe {
            libc::unlinkat(
                self.parent.as_raw_fd(),
                self.name.as_ptr(),
                if self.is_dir { libc::AT_REMOVEDIR } else { 0 },
            )
        } != 0
        {
            let e = std::io::Error::last_os_error();
            if e.kind() != std::io::ErrorKind::NotFound {
                error = Some(e);
            }
        }
        if let Some(e) = error {
            Err(e)
        } else {
            self.published = true;
            Ok(())
        }
    }
}
impl Drop for Stage {
    fn drop(&mut self) {
        let _ = self.cleanup();
    }
}

pub(super) fn staged_result<T>(
    ctx: &ToolContext<'_>,
    tool: &str,
    path: &str,
    stage: &mut Stage,
    result: Result<T, ToolFailure>,
) -> Result<T, ToolFailure> {
    match result {
        Ok(value) => Ok(value),
        Err(error) => {
            let published = stage.published;
            let cleanup = stage.cleanup();
            let state = if published {
                "published_uncertain"
            } else if cleanup.is_ok() {
                "failed_cleaned"
            } else {
                "cleanup_required"
            };
            let detail = json!({"tool":tool,"state":state,"staging_name":stage.name.to_string_lossy(),"error":error.to_string(),"cleanup_error":cleanup.as_ref().err().map(ToString::to_string)});
            let audit = ctx.app.audit_path_access(
                ctx.agent,
                ctx.session_id,
                "binary.failed",
                Some(path.into()),
                state,
                &detail.to_string(),
            );
            if published || cleanup.is_err() || audit.is_err() {
                return Err(io(format!("{error}; binary output state={state}; staging={}; cleanup={:?}; audit={:?}. Inspect destination before retrying.",stage.name.to_string_lossy(),cleanup.err(),audit.err())));
            }
            Err(error)
        }
    }
}
