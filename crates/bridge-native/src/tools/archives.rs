//! Bounded ZIP operations. Nothing is executed; extraction publishes a new
//! directory only after every entry has been checked and written successfully.
use super::{
    artifacts::{directory, open_at},
    parse_args, ToolContext, ToolOutcome,
};
use crate::{
    failure::ToolFailure,
    fsops::{self, PathLock},
    sandbox::{Access, ResolvedPath},
};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, HashSet},
    ffi::{CString, OsStr},
    fs::File,
    io::{Cursor, Read, Seek, SeekFrom, Write},
    os::{
        fd::AsRawFd,
        unix::{ffi::OsStrExt, fs::MetadataExt},
    },
    path::Path,
    time::{Duration, Instant},
};
use unicode_normalization::UnicodeNormalization;
use zip::{write::SimpleFileOptions, CompressionMethod, ZipArchive, ZipWriter};

const MAX_ARCHIVE: u64 = 32 * 1024 * 1024;
const MAX_FILE: u64 = 16 * 1024 * 1024;
const MAX_EXPANDED: u64 = 64 * 1024 * 1024;
const MAX_ENTRIES: usize = 256;
const MAX_NAMES: usize = 8192;

fn invalid(s: impl Into<String>) -> ToolFailure {
    ToolFailure::InvalidArguments(s.into())
}
fn io(e: impl std::fmt::Display) -> ToolFailure {
    ToolFailure::Io(e.to_string())
}
fn zip_error(e: impl std::fmt::Display) -> ToolFailure {
    invalid(format!("invalid or unsupported ZIP: {e}"))
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
fn read_source(
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
            "archive sources exclude bridge metadata and Git internals".into(),
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
            "archive sources must be regular files without hard links".into(),
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
struct Entry {
    path: String,
    directory: bool,
    bytes: u64,
}
struct Plan {
    entries: Vec<Entry>,
    expanded: u64,
}

fn safe_name(raw: &str, is_dir: bool) -> Result<String, ToolFailure> {
    let name = if is_dir {
        raw.strip_suffix('/').unwrap_or(raw)
    } else {
        raw
    };
    if name.is_empty()
        || name.len() > 240
        || name.contains(['\\', ':'])
        || name.chars().any(char::is_control)
    {
        return Err(invalid("ZIP names must be nonempty, at most 240 bytes, without control characters, backslashes or drive prefixes"));
    }
    let parts: Vec<_> = name.split('/').collect();
    if parts.len() > 12
        || parts
            .iter()
            .any(|s| s.is_empty() || *s == "." || *s == ".." || s.ends_with([' ', '.']))
    {
        return Err(ToolFailure::PermissionDenied(
            "unsafe ZIP path: absolute/traversal/ambiguous path or depth over 12".into(),
        ));
    }
    if parts
        .iter()
        .any(|s| s.eq_ignore_ascii_case(".bridge") || s.eq_ignore_ascii_case(".git"))
    {
        return Err(ToolFailure::PermissionDenied(
            "ZIP operations exclude bridge metadata and Git internals".into(),
        ));
    }
    Ok(name.into())
}
fn validate_names(entries: &[Entry]) -> Result<(), ToolFailure> {
    if entries.is_empty() || entries.len() > MAX_ENTRIES {
        return Err(invalid("ZIP must contain 1–256 entries"));
    }
    if entries.iter().map(|e| e.path.len()).sum::<usize>() > MAX_NAMES {
        return Err(invalid("ZIP path names exceed the 8192-byte budget"));
    }
    let mut seen = BTreeMap::new();
    let mut aliases = BTreeMap::new();
    for entry in entries {
        let key = entry.path.nfc().collect::<String>().to_lowercase();
        if seen.insert(key, entry.directory).is_some() {
            return Err(ToolFailure::Conflict(
                "duplicate or case/Unicode-colliding ZIP paths".into(),
            ));
        }
        let mut prefix = String::new();
        for part in entry.path.split('/') {
            if !prefix.is_empty() {
                prefix.push('/');
            }
            prefix.push_str(part);
            let key = prefix.nfc().collect::<String>().to_lowercase();
            if aliases
                .insert(key, prefix.clone())
                .is_some_and(|old| old != prefix)
            {
                return Err(ToolFailure::Conflict(
                    "case/Unicode-colliding ZIP directory names".into(),
                ));
            }
        }
    }
    for path in seen.keys() {
        let mut parent = Path::new(path).parent();
        while let Some(p) = parent {
            if let Some(false) = seen.get(p.to_string_lossy().as_ref()) {
                return Err(ToolFailure::Conflict(
                    "ZIP file is also a parent directory".into(),
                ));
            }
            parent = p.parent();
        }
    }
    Ok(())
}
/// Bound the directory before the ZIP library allocates its index. This first
/// release intentionally accepts ZIP32 only, with one unambiguous end record,
/// no prefixed executable, and a directory ending at that record. The library
/// remains responsible for decoding headers, compression and CRC validation.
fn directory_count(bytes: &[u8]) -> Result<usize, ToolFailure> {
    let mut markers = bytes
        .windows(4)
        .enumerate()
        .filter(|(_, b)| *b == b"PK\x05\x06");
    let (end, _) = markers
        .next()
        .ok_or_else(|| invalid("ZIP end record missing"))?;
    if markers.next().is_some() {
        return Err(ToolFailure::Unsupported(
            "ZIP has ambiguous end records; ZIP32 with one end record is required".into(),
        ));
    }
    let tail = bytes
        .get(end..)
        .filter(|t| t.len() >= 22)
        .ok_or_else(|| invalid("truncated ZIP end record"))?;
    let u16at = |i| u16::from_le_bytes([tail[i], tail[i + 1]]) as usize;
    let u32at = |i| u32::from_le_bytes([tail[i], tail[i + 1], tail[i + 2], tail[i + 3]]) as usize;
    let count = u16at(10);
    let size = u32at(12);
    let start = u32at(16);
    if u16at(4) != 0
        || u16at(6) != 0
        || u16at(8) != count
        || count == 65535
        || size == u32::MAX as usize
        || start == u32::MAX as usize
    {
        return Err(ToolFailure::Unsupported(
            "multi-disk and ZIP64 archives are unsupported".into(),
        ));
    }
    if count == 0 || count > MAX_ENTRIES {
        return Err(invalid("ZIP must contain 1–256 entries"));
    }
    if tail.len() != 22 + u16at(20)
        || start.checked_add(size) != Some(end)
        || size > 1024 * 1024
        || !bytes.starts_with(b"PK\x03\x04")
    {
        return Err(invalid(
            "unsupported ZIP32 directory layout or oversized headers",
        ));
    }
    let mut pos = start;
    for _ in 0..count {
        let h = bytes
            .get(pos..end)
            .filter(|h| h.len() >= 46 && h.starts_with(b"PK\x01\x02"))
            .ok_or_else(|| invalid("invalid ZIP directory entry"))?;
        let name = u16::from_le_bytes([h[28], h[29]]) as usize;
        let extra = u16::from_le_bytes([h[30], h[31]]) as usize;
        let comment = u16::from_le_bytes([h[32], h[33]]) as usize;
        if name == 0 || name > 241 || extra > 4096 || comment > 4096 {
            return Err(invalid("ZIP entry metadata exceeds supported limits"));
        }
        pos = pos
            .checked_add(46 + name + extra + comment)
            .filter(|p| *p <= end)
            .ok_or_else(|| invalid("ZIP directory length overflow"))?;
    }
    if pos != end {
        return Err(invalid("ZIP directory count does not match entries"));
    }
    Ok(count)
}
fn inspect(bytes: &[u8]) -> Result<(ZipArchive<Cursor<&[u8]>>, Plan), ToolFailure> {
    let count = directory_count(bytes)?;
    let mut archive = ZipArchive::new(Cursor::new(bytes)).map_err(zip_error)?;
    if archive.len() != count {
        return Err(invalid("ZIP entry count changed during decoding"));
    }
    archive.validate_header_consistency().map_err(zip_error)?;
    let mut entries = Vec::new();
    let mut expanded = 0u64;
    for i in 0..archive.len() {
        let f = archive.by_index_data(i).map_err(zip_error)?;
        if f.encrypted() {
            return Err(ToolFailure::Unsupported(
                "encrypted ZIP files are unsupported".into(),
            ));
        }
        if f.is_symlink()
            || f.unix_mode()
                .is_some_and(|mode| !matches!(mode & 0o170000, 0 | 0o100000 | 0o040000))
        {
            return Err(ToolFailure::PermissionDenied(
                "ZIP symlinks and special files are forbidden".into(),
            ));
        }
        if !matches!(
            f.compression(),
            CompressionMethod::Stored | CompressionMethod::Deflated
        ) {
            return Err(ToolFailure::Unsupported(
                "only stored and Deflate ZIP entries are supported".into(),
            ));
        }
        let name = safe_name(&f.name().map_err(zip_error)?, f.is_dir())?;
        if f.size() > MAX_FILE || (f.is_dir() && f.size() != 0) {
            return Err(invalid(
                "ZIP entry exceeds 16 MiB or directory contains data",
            ));
        }
        expanded = expanded
            .checked_add(f.size())
            .ok_or_else(|| invalid("ZIP size overflow"))?;
        if expanded > MAX_EXPANDED {
            return Err(invalid("ZIP expanded size exceeds 64 MiB"));
        }
        entries.push(Entry {
            path: name,
            directory: f.is_dir(),
            bytes: f.size(),
        });
    }
    validate_names(&entries)?;
    Ok((archive, Plan { entries, expanded }))
}

fn target(ctx: &ToolContext<'_>, tool: &str, raw: &str) -> Result<ResolvedPath, ToolFailure> {
    let path = ctx.resolve_any(raw)?;
    safe_name(&path.display(), false)?;
    ctx.check_read(tool, &path)?;
    ctx.recheck_access(Access::Write, &path, true)?;
    if path.exists {
        return Err(ToolFailure::Conflict(
            "archive destination must be new; overwrites are refused".into(),
        ));
    }
    directory(
        path.absolute
            .parent()
            .ok_or_else(|| invalid("destination needs an existing parent"))?,
    )?;
    Ok(path)
}
fn extraction_policy(
    ctx: &ToolContext<'_>,
    destination: &str,
    entries: &[Entry],
) -> Result<(), ToolFailure> {
    for entry in entries {
        let path = ctx.resolve_any(&format!("{destination}/{}", entry.path))?;
        ctx.check_read("extract_archive", &path)?;
        ctx.recheck_access(Access::Write, &path, true)?;
    }
    Ok(())
}

/// Descriptor-anchored staging. Cleanup names only files/directories created by
/// this operation. Publication never replaces a destination, including symlinks.
struct Stage {
    parent: File,
    name: CString,
    file: File,
    children: Vec<(File, CString, bool)>,
    is_dir: bool,
    published: bool,
}
impl Stage {
    fn new(parent: File, is_dir: bool) -> Result<Self, ToolFailure> {
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
    fn write_entry(
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
    fn publish(&mut self, destination: &Path) -> Result<(), ToolFailure> {
        let current = directory(
            destination
                .parent()
                .ok_or_else(|| invalid("missing parent"))?,
        )?;
        let a = current.metadata().map_err(io)?;
        let b = self.parent.metadata().map_err(io)?;
        if a.dev() != b.dev() || a.ino() != b.ino() {
            return Err(ToolFailure::Conflict(
                "destination parent changed during archive operation".into(),
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
        self.parent.sync_all().map_err(|e|io(format!("archive published; directory sync failed, inspect destination before retrying: {e}")))?;
        Ok(())
    }
    fn cleanup(&mut self) -> std::io::Result<()> {
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

fn staged_result<T>(
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
                "archive.failed",
                Some(path.into()),
                state,
                &detail.to_string(),
            );
            if published || cleanup.is_err() || audit.is_err() {
                return Err(io(format!("{error}; archive state={state}; staging={}; cleanup={:?}; audit={:?}. Inspect destination before retrying.",stage.name.to_string_lossy(),cleanup.err(),audit.err())));
            }
            Err(error)
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct List {
    path: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Extract {
    path: String,
    destination: String,
    expected_sha256: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Create {
    paths: Vec<String>,
    destination: String,
}

pub async fn list(ctx: &ToolContext<'_>, args: Value) -> Result<ToolOutcome, ToolFailure> {
    let args: List = parse_args(args)?;
    let bytes = read_source(ctx, "list_archive", &args.path, MAX_ARCHIVE)?;
    let (_, plan) = inspect(&bytes)?;
    Ok(ToolOutcome::read(format!("{} ZIP entries",plan.entries.len()),"Metadata validated. Extraction also verifies every entry's CRC and actual expanded size. Nested archives are never unpacked automatically.",Some(args.path))
        .with_data(json!({"sha256":fsops::sha256_bytes(&bytes),"archive_bytes":bytes.len(),"expanded_bytes":plan.expanded,"entries":plan.entries,"limits":{"entries":MAX_ENTRIES,"archive_bytes":MAX_ARCHIVE,"file_bytes":MAX_FILE,"expanded_bytes":MAX_EXPANDED,"depth":12}})))
}

pub async fn extract(ctx: &ToolContext<'_>, args: Value) -> Result<ToolOutcome, ToolFailure> {
    let args: Extract = parse_args(args)?;
    let original = read_source(ctx, "extract_archive", &args.path, MAX_ARCHIVE)?;
    if fsops::sha256_bytes(&original) != args.expected_sha256 {
        return Err(ToolFailure::Conflict(
            "archive changed; list it again".into(),
        ));
    }
    let (_, plan) = inspect(&original)?;
    let initial = target(ctx, "extract_archive", &args.destination)?;
    extraction_policy(ctx, &initial.display(), &plan.entries)?;
    let description=format!("Extract ZIP {} into new folder {}\nSHA256: {}\n{} expanded bytes. No execution or overwrites. Files receive no executable bits.\n{}",args.path,initial.display(),args.expected_sha256,plan.expanded,serde_json::to_string_pretty(&plan.entries).map_err(io)?);
    if description.len() > 50_000 {
        return Err(invalid("archive review exceeds 50,000 bytes"));
    }
    ctx.app
        .request_approval(
            ctx.session_id,
            ctx.agent,
            "extract_archive",
            &description,
            Some(initial.display()),
        )
        .await?;
    let _lock = PathLock::acquire(&ctx.app.workspace.lock_dir, &initial.display()).map_err(io)?;
    let path = target(ctx, "extract_archive", &args.destination)?;
    extraction_policy(ctx, &path.display(), &plan.entries)?;
    let bytes = read_source(ctx, "extract_archive", &args.path, MAX_ARCHIVE)?;
    if fsops::sha256_bytes(&bytes) != args.expected_sha256 {
        return Err(ToolFailure::Conflict(
            "archive changed during approval".into(),
        ));
    }
    drop(original);
    let (mut archive, _) = inspect(&bytes)?;
    let mut stage = Stage::new(directory(path.absolute.parent().unwrap())?, true)?;
    let mut receipt = json!({"source_sha256":args.expected_sha256,"destination":path.display(),"entry_count":plan.entries.len(),"expanded_bytes":plan.expanded,"state":"prepared","staging_name":stage.name.to_string_lossy()});
    ctx.app
        .audit_path_access(
            ctx.agent,
            ctx.session_id,
            "archive.prepare",
            Some(path.display()),
            "allowed",
            &receipt.to_string(),
        )
        .map_err(io)?;
    let end = Instant::now() + Duration::from_secs(15);
    let staged = (|| {
        let mut files = vec![];
        for (i, entry) in plan.entries.iter().enumerate() {
            deadline(end)?;
            let mut data = archive
                .by_index_with_options(
                    i,
                    zip::ZipReadOptions::new().enforce_header_consistency(true),
                )
                .map_err(zip_error)?;
            let sha = stage.write_entry(entry, &mut data, end)?;
            files.push(json!({"path":entry.path,"bytes":entry.bytes,"directory":entry.directory,"sha256":if entry.directory{None}else{Some(sha)}}));
        }
        target(ctx, "extract_archive", &args.destination)?;
        extraction_policy(ctx, &path.display(), &plan.entries)?;
        stage.publish(&path.absolute)?;
        Ok(files)
    })();
    let files = staged_result(ctx, "extract_archive", &path.display(), &mut stage, staged)?;
    receipt["state"] = json!("completed");
    receipt["entries"] = json!(files);
    ctx.app
        .audit_path_access(
            ctx.agent,
            ctx.session_id,
            "tool.extract_archive",
            Some(path.display()),
            "allowed",
            &receipt.to_string(),
        )
        .map_err(|e| {
            io(format!(
                "files extracted but audit finalization failed; inspect destination: {e}"
            ))
        })?;
    Ok(ToolOutcome::write("ZIP extracted into a new folder","Original archive preserved. Nested archives and executable contents were not run. No automatic binary undo.",Some(path.display())).with_data(receipt))
}

struct LimitedWriter {
    cursor: Cursor<Vec<u8>>,
    end: Instant,
}
impl Write for LimitedWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if Instant::now() >= self.end
            || self.cursor.position().saturating_add(bytes.len() as u64) > MAX_ARCHIVE
        {
            return Err(std::io::Error::other(
                "ZIP output exceeds its byte/time budget",
            ));
        }
        self.cursor.write(bytes)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
impl Seek for LimitedWriter {
    fn seek(&mut self, p: SeekFrom) -> std::io::Result<u64> {
        self.cursor.seek(p)
    }
}

pub async fn create(ctx: &ToolContext<'_>, args: Value) -> Result<ToolOutcome, ToolFailure> {
    let args: Create = parse_args(args)?;
    if args.paths.is_empty() || args.paths.len() > MAX_ENTRIES {
        return Err(invalid(
            "select 1–256 regular files; directories are not recursively included",
        ));
    }
    let initial = target(ctx, "create_archive", &args.destination)?;
    let mut entries = vec![];
    let mut snapshots = vec![];
    let mut total = 0u64;
    let mut sources = HashSet::new();
    for raw in &args.paths {
        let path = ctx.resolve_existing(raw)?;
        let name = safe_name(&path.display(), false)?;
        if !sources.insert(name.clone()) {
            return Err(invalid("duplicate archive source"));
        }
        let bytes = read_source(ctx, "create_archive", raw, MAX_FILE)?;
        total += bytes.len() as u64;
        if total > MAX_EXPANDED {
            return Err(invalid("source files exceed 64 MiB total"));
        }
        entries.push(Entry {
            path: name,
            directory: false,
            bytes: bytes.len() as u64,
        });
        snapshots.push(bytes);
    }
    validate_names(&entries)?;
    let hashes: Vec<_> = snapshots.iter().map(|b| fsops::sha256_bytes(b)).collect();
    let manifest: Vec<_> = entries
        .iter()
        .zip(&hashes)
        .map(|(e, h)| json!({"path":e.path,"bytes":e.bytes,"sha256":h}))
        .collect();
    let description=format!("Create ZIP {} from these exact files ({} bytes). No overwrite. No recursive directory inclusion or execution.\n{}",initial.display(),total,serde_json::to_string_pretty(&manifest).map_err(io)?);
    if description.len() > 50_000 {
        return Err(invalid(
            "archive review exceeds 50,000 bytes; select fewer files",
        ));
    }
    ctx.app
        .request_approval(
            ctx.session_id,
            ctx.agent,
            "create_archive",
            &description,
            Some(initial.display()),
        )
        .await?;
    let _lock = PathLock::acquire(&ctx.app.workspace.lock_dir, &initial.display()).map_err(io)?;
    let path = target(ctx, "create_archive", &args.destination)?;
    for (i, entry) in entries.iter().enumerate() {
        let bytes = read_source(ctx, "create_archive", &entry.path, MAX_FILE)?;
        if fsops::sha256_bytes(&bytes) != hashes[i] {
            return Err(ToolFailure::Conflict(format!(
                "{} changed during approval",
                entry.path
            )));
        }
    }
    let end = Instant::now() + Duration::from_secs(15);
    let mut archive = ZipWriter::new(LimitedWriter {
        cursor: Cursor::new(vec![]),
        end,
    });
    for (entry, bytes) in entries.iter().zip(&snapshots) {
        deadline(end)?;
        archive
            .start_file(
                &entry.path,
                SimpleFileOptions::default()
                    .compression_method(CompressionMethod::Deflated)
                    .compression_level(Some(1))
                    .unix_permissions(0o600),
            )
            .map_err(zip_error)?;
        archive.write_all(bytes).map_err(io)?;
    }
    let bytes = archive.finish().map_err(zip_error)?.cursor.into_inner();
    let sha = fsops::sha256_bytes(&bytes);
    // Check the produced archive with the same reader used for extraction.
    inspect(&bytes)?;
    let mut stage = Stage::new(directory(path.absolute.parent().unwrap())?, false)?;
    let staged = (|| {
        stage.file.write_all(&bytes).map_err(io)?;
        target(ctx, "create_archive", &args.destination)?;
        let mut receipt = json!({"destination":path.display(),"sha256":sha,"bytes":bytes.len(),"entries":manifest,"state":"prepared","staging_name":stage.name.to_string_lossy()});
        ctx.app
            .audit_path_access(
                ctx.agent,
                ctx.session_id,
                "archive.prepare",
                Some(path.display()),
                "allowed",
                &receipt.to_string(),
            )
            .map_err(io)?;
        stage.publish(&path.absolute)?;
        receipt["state"] = json!("completed");
        Ok(receipt)
    })();
    let receipt = staged_result(ctx, "create_archive", &path.display(), &mut stage, staged)?;
    ctx.app
        .audit_path_access(
            ctx.agent,
            ctx.session_id,
            "tool.create_archive",
            Some(path.display()),
            "allowed",
            &receipt.to_string(),
        )
        .map_err(|e| {
            io(format!(
                "archive created but audit finalization failed; inspect destination: {e}"
            ))
        })?;
    Ok(ToolOutcome::write(
        "ZIP created",
        "Source paths are relative to the workspace. Sources preserved; no automatic binary undo.",
        Some(path.display()),
    )
    .with_data(receipt))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn staged_publication_refuses_collisions_and_cleanup_preserves_unrelated_files() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let mut stage = Stage::new(directory(&root).unwrap(), true).unwrap();
        let entry = Entry {
            path: "sub/file.txt".into(),
            directory: false,
            bytes: 4,
        };
        stage
            .write_entry(
                &entry,
                &mut Cursor::new(b"data"),
                Instant::now() + Duration::from_secs(5),
            )
            .unwrap();
        std::fs::write(root.join("exists"), "original").unwrap();
        assert!(stage.publish(&root.join("exists")).is_err());
        std::os::unix::fs::symlink(root.join("missing"), root.join("link")).unwrap();
        assert!(stage.publish(&root.join("link")).is_err());
        let staged_path = root.join(stage.name.to_string_lossy().as_ref());
        // Another local writer's file is not ours to delete, even in our
        // staging directory. Cleanup reports this condition explicitly.
        std::fs::write(staged_path.join("unrelated"), "keep").unwrap();
        assert!(stage.cleanup().is_err());
        assert_eq!(
            std::fs::read_to_string(staged_path.join("unrelated")).unwrap(),
            "keep"
        );
        assert!(!staged_path.join("sub").exists());
        assert_eq!(
            std::fs::read_to_string(root.join("exists")).unwrap(),
            "original"
        );
        assert!(!root.join("missing").exists());
    }

    #[test]
    fn entry_data_and_output_limits_prevent_publishing_unbounded_contents() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let name;
        {
            let mut stage = Stage::new(directory(&root).unwrap(), true).unwrap();
            name = stage.name.to_string_lossy().to_string();
            assert!(stage
                .write_entry(
                    &Entry {
                        path: "small.txt".into(),
                        directory: false,
                        bytes: 1
                    },
                    &mut Cursor::new(b"too long"),
                    Instant::now() + Duration::from_secs(5)
                )
                .is_err());
        }
        assert!(!root.join(name).exists());
        let mut writer = LimitedWriter {
            cursor: Cursor::new(vec![]),
            end: Instant::now() + Duration::from_secs(1),
        };
        writer.seek(SeekFrom::Start(MAX_ARCHIVE)).unwrap();
        assert!(writer.write_all(b"x").is_err());
        assert_eq!(writer.cursor.get_ref().len(), 0);
        writer.end = Instant::now();
        writer.seek(SeekFrom::Start(0)).unwrap();
        assert!(writer.write_all(b"x").is_err());
    }
}
