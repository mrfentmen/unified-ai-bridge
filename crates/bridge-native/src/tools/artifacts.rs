//! Explicit per-file grants from Chrome's trusted extension controls. Source
//! paths stay in the private registry; model calls accept opaque IDs only.
use super::{parse_args, ToolContext, ToolOutcome};
use crate::{
    app::App,
    failure::ToolFailure,
    fsops::PathLock,
    sandbox::{Access, Decision},
};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::os::{
    fd::{AsRawFd, FromRawFd},
    unix::{ffi::OsStrExt, fs::MetadataExt},
};
use std::{
    fs::File,
    io::{Read, Write},
    path::{Component, Path, PathBuf},
};

pub const MAX_BYTES: u64 = 64 * 1024 * 1024;
const GRANT_SECONDS: i64 = 24 * 60 * 60;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SelectedDownload {
    pub download_id: i32,
    pub filename: String,
    pub bytes: u64,
    pub mime: String,
    /// Host only, never the URL, path, query or fragment.
    pub origin: String,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Artifact {
    pub id: String,
    pub workspace_id: String,
    pub session_id: String,
    pub download_id: i32,
    source: PathBuf,
    device: u64,
    inode: u64,
    name: String,
    bytes: u64,
    sha256: String,
    mime: String,
    origin: String,
    expires_at: i64,
    state: String,
    destination: Option<String>,
}

impl Artifact {
    pub fn metadata(&self) -> Value {
        json!({"artifact_id":self.id,"name":self.name,"bytes":self.bytes,"sha256":self.sha256,
            "mime":self.mime,"origin":self.origin,"expires_at":self.expires_at,
            "state":if self.state=="selected" && self.expires_at<=chrono::Utc::now().timestamp(){"expired"}else{&self.state},
            "destination":self.destination,"workspace_id":self.workspace_id})
    }
    fn available(&self) -> Result<(), ToolFailure> {
        if self.state != "selected" {
            return Err(ToolFailure::Conflict(format!(
                "artifact state is {}; inspect the destination before retrying",
                self.state
            )));
        }
        if self.expires_at <= chrono::Utc::now().timestamp() {
            return Err(ToolFailure::PermissionDenied(
                "artifact grant expired; download and share the file again".into(),
            ));
        }
        Ok(())
    }
}

fn internal(e: impl std::fmt::Display) -> ToolFailure {
    ToolFailure::Internal(e.to_string())
}
fn io(e: std::io::Error) -> ToolFailure {
    ToolFailure::from_io("artifact file operation", e)
}
fn id() -> String {
    let mut bytes = [0; 16];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    hex::encode(bytes)
}

/// Walk from / using directory descriptors. Neither the leaf nor an ancestor
/// can redirect an authorized source/destination through a symlink.
fn open_at(dir: &File, name: &std::ffi::OsStr, flags: i32) -> Result<File, ToolFailure> {
    let name = std::ffi::CString::new(name.as_bytes())
        .map_err(|_| ToolFailure::InvalidArguments("NUL in file name".into()))?;
    // SAFETY: borrowed descriptor and NUL-terminated name live for this call.
    let fd = unsafe {
        libc::openat(
            dir.as_raw_fd(),
            name.as_ptr(),
            flags | libc::O_CLOEXEC | libc::O_NOFOLLOW,
            0o600,
        )
    };
    if fd < 0 {
        return Err(io(std::io::Error::last_os_error()));
    }
    // SAFETY: openat returned a new owned descriptor.
    Ok(unsafe { File::from_raw_fd(fd) })
}

fn directory(path: &Path) -> Result<File, ToolFailure> {
    if !path.is_absolute() {
        return Err(ToolFailure::InvalidArguments(
            "absolute download path required".into(),
        ));
    }
    let mut dir = File::open("/").map_err(io)?;
    for part in path.components() {
        match part {
            Component::RootDir => {}
            Component::Normal(name) => {
                dir = open_at(&dir, name, libc::O_RDONLY | libc::O_DIRECTORY)?
            }
            _ => {
                return Err(ToolFailure::PermissionDenied(
                    "traversal in artifact path".into(),
                ))
            }
        }
    }
    Ok(dir)
}

fn source(path: &Path) -> Result<File, ToolFailure> {
    let dir = directory(
        path.parent()
            .ok_or_else(|| ToolFailure::InvalidArguments("download has no parent".into()))?,
    )?;
    let file = open_at(
        &dir,
        path.file_name()
            .ok_or_else(|| ToolFailure::InvalidArguments("download has no filename".into()))?,
        libc::O_RDONLY | libc::O_NONBLOCK,
    )?;
    let meta = file.metadata().map_err(io)?;
    if !meta.is_file() || meta.nlink() != 1 {
        return Err(ToolFailure::PermissionDenied(
            "download must be a regular file without hard links".into(),
        ));
    }
    if meta.len() > MAX_BYTES {
        return Err(ToolFailure::InvalidArguments(
            "artifact exceeds 64 MiB".into(),
        ));
    }
    Ok(file)
}

fn transfer(input: File, output: &mut impl Write) -> Result<(String, u64), ToolFailure> {
    let mut bounded = input.take(MAX_BYTES + 1);
    let mut hash = Sha256::new();
    let mut count = 0;
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = bounded.read(&mut buffer).map_err(io)?;
        if read == 0 {
            break;
        }
        count += read as u64;
        if count > MAX_BYTES {
            return Err(ToolFailure::InvalidArguments(
                "artifact exceeds 64 MiB".into(),
            ));
        }
        hash.update(&buffer[..read]);
        output.write_all(&buffer[..read]).map_err(io)?;
    }
    Ok((hex::encode(hash.finalize()), count))
}

fn session(
    app: &App,
    session_id: &str,
    workspace_id: &str,
) -> Result<bridge_core::provider::Agent, ToolFailure> {
    let (record, agent) = app
        .session_agent(session_id)
        .map_err(internal)?
        .ok_or_else(|| ToolFailure::PermissionDenied("unknown artifact session".into()))?;
    if record.workspace_id != workspace_id || app.workspace.id() != workspace_id {
        return Err(ToolFailure::PermissionDenied(
            "artifact workspace changed; refresh controls".into(),
        ));
    }
    Ok(agent)
}

pub fn register(
    app: &App,
    session_id: &str,
    workspace_id: &str,
    selected: SelectedDownload,
) -> Result<Value, ToolFailure> {
    let agent = session(app, session_id, workspace_id)?;
    if selected.download_id < 0
        || selected.bytes > MAX_BYTES
        || selected.filename.len() > 4096
        || selected.mime.len() > 128
        || selected.mime.chars().any(char::is_control)
        || selected.origin.len() > 253
        || !selected
            .origin
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b".-".contains(&b))
    {
        return Err(ToolFailure::InvalidArguments(
            "invalid download metadata or file exceeds 64 MiB".into(),
        ));
    }
    let path = PathBuf::from(&selected.filename);
    let name = path
        .file_name()
        .and_then(|s| s.to_str())
        .filter(|s| !s.chars().any(char::is_control))
        .ok_or_else(|| ToolFailure::InvalidArguments("invalid download filename".into()))?
        .to_string();
    // Apply the same private-file exclusions to source paths, independently of
    // the destination policy. An explicit share must not smuggle a secret file.
    for pattern in &app.config.private_file_patterns {
        let matcher = globset::GlobBuilder::new(pattern)
            .case_insensitive(true)
            .literal_separator(true)
            .build()
            .map_err(internal)?
            .compile_matcher();
        if matcher.is_match(&path)
            || path
                .components()
                .any(|p| matcher.is_match(Path::new(p.as_os_str())))
        {
            return Err(ToolFailure::PermissionDenied(
                "download excluded by private-file policy".into(),
            ));
        }
    }
    let file = source(&path)?;
    let meta = file.metadata().map_err(io)?;
    let (sha256, bytes) = transfer(file, &mut std::io::sink())?;
    if selected.bytes != bytes || meta.len() != bytes {
        return Err(ToolFailure::Conflict(
            "download changed; refresh browser downloads".into(),
        ));
    }
    let _lock = PathLock::acquire(
        &app.home.base.join("locks"),
        &format!("share:{workspace_id}:{session_id}:{}", selected.download_id),
    )
    .map_err(internal)?;
    let known = app
        .store
        .artifacts(workspace_id, session_id)
        .map_err(internal)?;
    if let Some(old) = known.iter().find(|a| a.download_id == selected.download_id) {
        if old.sha256 != sha256
            || old.source != path
            || old.device != meta.dev()
            || old.inode != meta.ino()
        {
            return Err(ToolFailure::Conflict(
                "shared download changed; download it again before sharing".into(),
            ));
        }
        return Ok(old.metadata());
    }
    if known.len() >= 100 {
        return Err(ToolFailure::InvalidArguments(
            "100 shared files per session maximum; use a new conversation".into(),
        ));
    }
    let artifact = Artifact {
        id: id(),
        workspace_id: workspace_id.into(),
        session_id: session_id.into(),
        download_id: selected.download_id,
        source: path,
        device: meta.dev(),
        inode: meta.ino(),
        name,
        bytes,
        sha256,
        mime: selected.mime,
        origin: selected.origin,
        expires_at: chrono::Utc::now().timestamp() + GRANT_SECONDS,
        state: "selected".into(),
        destination: None,
    };
    let metadata = artifact.metadata();
    app.audit_path_access(
        agent,
        session_id,
        "artifact.share",
        None,
        "allowed",
        &metadata.to_string(),
    )
    .map_err(internal)?;
    app.store.save_artifact(&artifact).map_err(internal)?;
    app.record_activity(
        agent,
        session_id,
        "artifact.share",
        "read",
        None,
        &format!("Shared {} ({} bytes)", artifact.name, bytes),
        "allowed",
    )
    .map_err(internal)?;
    Ok(metadata)
}

pub fn revoke(
    app: &App,
    session_id: &str,
    workspace_id: &str,
    artifact_id: &str,
) -> Result<Value, ToolFailure> {
    let agent = session(app, session_id, workspace_id)?;
    let _lock = PathLock::acquire(
        &app.home.base.join("locks"),
        &format!("artifact:{artifact_id}"),
    )
    .map_err(internal)?;
    let mut artifact = find(app, session_id, artifact_id)?;
    if artifact.state != "selected" {
        return Err(ToolFailure::Conflict(
            "only pending shares can be revoked; imported files are left untouched".into(),
        ));
    }
    artifact.state = "revoked".into();
    app.store.save_artifact(&artifact).map_err(internal)?;
    app.audit_path_access(
        agent,
        session_id,
        "artifact.revoke",
        None,
        "allowed",
        &artifact.metadata().to_string(),
    )
    .map_err(internal)?;
    Ok(artifact.metadata())
}

fn find(app: &App, session_id: &str, id: &str) -> Result<Artifact, ToolFailure> {
    app.store
        .artifacts(&app.workspace.id(), session_id)
        .map_err(internal)?
        .into_iter()
        .find(|a| a.id == id)
        .ok_or_else(|| {
            ToolFailure::PermissionDenied(
                "artifact was not shared with this session and workspace".into(),
            )
        })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Empty {}
pub async fn list(ctx: &ToolContext<'_>, args: Value) -> Result<ToolOutcome, ToolFailure> {
    let _: Empty = parse_args(args)?;
    session(ctx.app, ctx.session_id, &ctx.app.workspace.id())?;
    let artifacts: Vec<_> = ctx
        .app
        .store
        .artifacts(&ctx.app.workspace.id(), ctx.session_id)
        .map_err(internal)?
        .iter()
        .map(Artifact::metadata)
        .collect();
    Ok(ToolOutcome::read(
        format!("{} explicitly shared browser files", artifacts.len()),
        "Only files selected in the extension controls appear here.",
        None,
    )
    .with_data(json!({"artifacts":artifacts,"max_bytes":MAX_BYTES})))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Import {
    artifact_id: String,
    path: String,
}

/// Temp files are created and published relative to the SAME open directory.
/// linkat fails atomically if any destination exists, including a symlink.
struct Staged {
    dir: File,
    name: std::ffi::CString,
    file: File,
}
impl Staged {
    fn new(dir: File) -> Result<Self, ToolFailure> {
        let name = format!(".bridge-import-{}", id());
        let file = open_at(
            &dir,
            std::ffi::OsStr::new(&name),
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL,
        )?;
        Ok(Self {
            dir,
            name: std::ffi::CString::new(name).unwrap(),
            file,
        })
    }
    fn publish(&self, name: &std::ffi::OsStr) -> Result<(), ToolFailure> {
        let name = std::ffi::CString::new(name.as_bytes()).map_err(internal)?;
        // SAFETY: descriptors and C strings stay alive throughout linkat.
        if unsafe {
            libc::linkat(
                self.dir.as_raw_fd(),
                self.name.as_ptr(),
                self.dir.as_raw_fd(),
                name.as_ptr(),
                0,
            )
        } != 0
        {
            return Err(io(std::io::Error::last_os_error()));
        }
        Ok(())
    }
}
impl Drop for Staged {
    fn drop(&mut self) {
        // SAFETY: the temporary basename is ours, in our held directory.
        unsafe {
            libc::unlinkat(self.dir.as_raw_fd(), self.name.as_ptr(), 0);
        }
    }
}

pub async fn import(ctx: &ToolContext<'_>, args: Value) -> Result<ToolOutcome, ToolFailure> {
    let args: Import = parse_args(args)?;
    session(ctx.app, ctx.session_id, &ctx.app.workspace.id())?;
    let initial_artifact = find(ctx.app, ctx.session_id, &args.artifact_id)?;
    initial_artifact.available()?;
    let initial = ctx.resolve_any(&args.path)?;
    ctx.check_read("import_artifact", &initial)?;
    if let Decision::Deny(reason) = ctx.app.decide(ctx.agent, Access::Write, &initial) {
        return Err(ToolFailure::PermissionDenied(reason));
    }
    if initial.exists {
        return Err(ToolFailure::Conflict(
            "import destination already exists".into(),
        ));
    }
    directory(initial.absolute.parent().ok_or_else(|| {
        ToolFailure::InvalidArguments("destination needs an existing parent directory".into())
    })?)?;
    ctx.app.request_approval(ctx.session_id, ctx.agent, "import_artifact",
        &format!("Copy shared download {} ({} bytes, {})\nSHA256: {}\nSource: {}\nOriginal preserved. No overwrite, extraction, execution or binary undo.", initial_artifact.name, initial_artifact.bytes, initial_artifact.mime, initial_artifact.sha256, initial_artifact.origin),
        Some(initial.display())).await?;
    let _artifact_lock = PathLock::acquire(
        &ctx.app.home.base.join("locks"),
        &format!("artifact:{}", args.artifact_id),
    )
    .map_err(internal)?;
    let _path_lock =
        PathLock::acquire(&ctx.app.workspace.lock_dir, &initial.display()).map_err(internal)?;
    let mut artifact = find(ctx.app, ctx.session_id, &args.artifact_id)?;
    artifact.available()?;
    let target = ctx.resolve_any(&args.path)?;
    ctx.check_read("import_artifact", &target)?;
    ctx.recheck_access(Access::Write, &target, true)?;
    if target.absolute != initial.absolute || target.exists {
        return Err(ToolFailure::Conflict(
            "import destination changed during approval".into(),
        ));
    }
    let input = source(&artifact.source)?;
    let meta = input.metadata().map_err(io)?;
    if meta.dev() != artifact.device || meta.ino() != artifact.inode || meta.len() != artifact.bytes
    {
        return Err(ToolFailure::Conflict(
            "shared source changed; nothing imported".into(),
        ));
    }
    let mut staged = Staged::new(directory(target.absolute.parent().unwrap())?)?;
    let (hash, bytes) = transfer(input, &mut staged.file)?;
    if hash != artifact.sha256 || bytes != artifact.bytes {
        return Err(ToolFailure::Conflict(
            "shared source hash changed; nothing imported".into(),
        ));
    }
    staged.file.sync_all().map_err(io)?;
    // Recheck policy and directory identity immediately before publication.
    ctx.recheck_access(Access::Write, &ctx.resolve_any(&args.path)?, true)?;
    let current = directory(target.absolute.parent().unwrap())?
        .metadata()
        .map_err(io)?;
    let held = staged.dir.metadata().map_err(io)?;
    if current.dev() != held.dev() || current.ino() != held.ino() {
        return Err(ToolFailure::Conflict(
            "destination directory changed".into(),
        ));
    }
    artifact.state = "importing".into();
    artifact.destination = Some(target.display());
    ctx.app.store.save_artifact(&artifact).map_err(internal)?;
    ctx.app
        .audit_path_access(
            ctx.agent,
            ctx.session_id,
            "artifact.prepare",
            Some(target.display()),
            "allowed",
            &artifact.metadata().to_string(),
        )
        .map_err(internal)?;
    if let Err(error) = staged.publish(target.absolute.file_name().unwrap()) {
        artifact.state = "selected".into();
        artifact.destination = None;
        ctx.app.store.save_artifact(&artifact).map_err(internal)?;
        return Err(error);
    }
    staged.dir.sync_all().map_err(|e| {
        internal(format!(
            "file published but directory sync failed; inspect destination before retrying: {e}"
        ))
    })?;
    artifact.state = "imported".into();
    ctx.app.store.save_artifact(&artifact).map_err(|e| {
        internal(format!(
            "file imported but receipt update failed; inspect destination: {e}"
        ))
    })?;
    ctx.app
        .audit_path_access(
            ctx.agent,
            ctx.session_id,
            "tool.import_artifact",
            Some(target.display()),
            "allowed",
            &artifact.metadata().to_string(),
        )
        .map_err(|e| internal(format!("file imported but audit append failed: {e}")))?;
    Ok(ToolOutcome::write(
        format!("Imported {} bytes into {}", bytes, target.display()),
        format!("SHA256: {hash}\nOriginal download preserved. No archive extraction or execution."),
        Some(target.display()),
    )
    .with_data(artifact.metadata()))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn publication_never_replaces_a_file_or_dangling_symlink_and_cleans_staging() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let mut staged = Staged::new(directory(&root).unwrap()).unwrap();
        staged.file.write_all(b"new bytes").unwrap();
        std::fs::write(root.join("collision"), b"keep").unwrap();
        std::os::unix::fs::symlink(root.join("missing"), root.join("link")).unwrap();
        assert!(staged.publish(std::ffi::OsStr::new("collision")).is_err());
        assert!(staged.publish(std::ffi::OsStr::new("link")).is_err());
        assert_eq!(std::fs::read(root.join("collision")).unwrap(), b"keep");
        assert!(!root.join("missing").exists());
        staged.publish(std::ffi::OsStr::new("saved")).unwrap();
        drop(staged);
        assert_eq!(std::fs::read(root.join("saved")).unwrap(), b"new bytes");
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 3);
    }
}
