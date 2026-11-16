//! Bounded binary recovery, sharing the existing change journal. Original inodes
//! are retained in workspace trash; a private independent snapshot protects undo.
//! Replacement is a journalled detach/publish, not an atomic compare-and-swap.
use super::{
    artifacts::{directory, open_at},
    binary::{self, Stage},
    parse_args,
    recovery::Change,
    ToolContext, ToolOutcome,
};
use crate::{
    app::App,
    failure::ToolFailure,
    fsops::{self, PathLock},
    sandbox::{Access, ResolvedPath},
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    ffi::{CString, OsStr},
    fs::File,
    io::{Read, Write},
    os::{
        fd::AsRawFd,
        unix::{ffi::OsStrExt, fs::MetadataExt},
    },
};
const MIB: u64 = 1024 * 1024;
pub const MAX_BYTES: u64 = 64 * MIB;
static SLOTS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(1);

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    pub max_storage_mib: u64,
    pub retention_days: u32,
    pub max_snapshots: usize,
    pub reserve_mib: u64,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            max_storage_mib: 512,
            retention_days: 30,
            max_snapshots: 1000,
            reserve_mib: 64,
        }
    }
}
impl Settings {
    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            (64..=4096).contains(&self.max_storage_mib),
            "storage must be 64–4096 MiB"
        );
        anyhow::ensure!(
            (1..=365).contains(&self.retention_days),
            "retention must be 1–365 days"
        );
        anyhow::ensure!(
            (1..=2000).contains(&self.max_snapshots),
            "snapshot count must be 1–2000"
        );
        anyhow::ensure!(
            (64..=4096).contains(&self.reserve_mib),
            "free-space reserve must be 64–4096 MiB"
        );
        Ok(())
    }
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Backup {
    pub bytes: u64,
}
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Control {
    Status {},
    Settings { settings: Settings },
    Cleanup { change_ids: Vec<String> },
}
fn io(e: impl std::fmt::Display) -> ToolFailure {
    ToolFailure::Io(e.to_string())
}
fn conflict(s: impl Into<String>) -> ToolFailure {
    ToolFailure::Conflict(s.into())
}
fn name(s: &OsStr) -> Result<CString, ToolFailure> {
    if s.as_bytes().contains(&b'/') || s.as_bytes() == b".." || s.as_bytes() == b"." {
        return Err(io("invalid recovery basename"));
    }
    CString::new(s.as_bytes()).map_err(io)
}
fn valid_id(id: &str) -> Result<(), ToolFailure> {
    if id.len() != 32 || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(io("invalid recovery ID"));
    }
    Ok(())
}
fn unique() -> String {
    use rand::RngCore;
    let mut b = [0u8; 16];
    rand::rngs::OsRng.fill_bytes(&mut b);
    hex::encode(b)
}
fn child(dir: &File, part: &str) -> Result<File, ToolFailure> {
    let n = name(OsStr::new(part))?;
    // SAFETY: held directory and owned basename are valid for each syscall.
    if unsafe { libc::mkdirat(dir.as_raw_fd(), n.as_ptr(), 0o700) } != 0
        && std::io::Error::last_os_error().kind() != std::io::ErrorKind::AlreadyExists
    {
        return Err(io(std::io::Error::last_os_error()));
    }
    open_at(dir, OsStr::new(part), libc::O_RDONLY | libc::O_DIRECTORY)
}
fn private(app: &App) -> Result<File, ToolFailure> {
    let base = directory(&app.home.base.canonicalize().map_err(io)?)?;
    let root = child(&base, "recovery")?;
    let dir = child(&root, &app.workspace.id())?;
    if dir.metadata().map_err(io)?.mode() & 0o077 != 0 {
        return Err(io("private recovery directory must have mode 0700"));
    }
    Ok(dir)
}
fn trash(app: &App) -> Result<File, ToolFailure> {
    let root = directory(&app.workspace.root)?;
    child(&child(&root, ".bridge")?, "trash")
}
fn rename(from: &File, src: &OsStr, to: &File, dst: &OsStr) -> Result<(), ToolFailure> {
    let src = name(src)?;
    let dst = name(dst)?;
    // SAFETY: both directory descriptors and strings remain owned during rename.
    #[cfg(target_os = "macos")]
    let r = unsafe {
        libc::renameatx_np(
            from.as_raw_fd(),
            src.as_ptr(),
            to.as_raw_fd(),
            dst.as_ptr(),
            libc::RENAME_EXCL,
        )
    };
    #[cfg(target_os = "linux")]
    let r = unsafe {
        libc::renameat2(
            from.as_raw_fd(),
            src.as_ptr(),
            to.as_raw_fd(),
            dst.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    let r = -1;
    if r != 0 {
        return Err(io(std::io::Error::last_os_error()));
    }
    from.sync_all().map_err(io)?;
    to.sync_all().map_err(io)?;
    Ok(())
}
fn bytes(dir: &File, n: &OsStr) -> Result<Vec<u8>, ToolFailure> {
    let input = open_at(dir, n, libc::O_RDONLY | libc::O_NONBLOCK)?;
    let meta = input.metadata().map_err(io)?;
    if !meta.is_file() || meta.nlink() != 1 {
        return Err(ToolFailure::PermissionDenied(
            "binary recovery requires regular files without hard links".into(),
        ));
    }
    if meta.len() > MAX_BYTES {
        return Err(ToolFailure::InvalidArguments(
            "binary recovery limit is 64 MiB per file".into(),
        ));
    }
    let mut b = Vec::new();
    input.take(MAX_BYTES + 1).read_to_end(&mut b).map_err(io)?;
    if b.len() as u64 > MAX_BYTES {
        return Err(io("file grew beyond 64 MiB"));
    }
    Ok(b)
}
fn snapshot(path: &ResolvedPath) -> Result<Option<Vec<u8>>, ToolFailure> {
    let dir = directory(
        path.absolute
            .parent()
            .ok_or_else(|| io("file has no parent"))?,
    )?;
    // The no-follow open is authoritative. A previously missing path may now exist.
    let n = path
        .absolute
        .file_name()
        .ok_or_else(|| io("file has no name"))?;
    let c = name(n)?;
    let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
    if unsafe {
        libc::fstatat(
            dir.as_raw_fd(),
            c.as_ptr(),
            stat.as_mut_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    } != 0
    {
        let err = std::io::Error::last_os_error();
        if err.kind() == std::io::ErrorKind::NotFound {
            return Ok(None);
        }
        return Err(io(err));
    }
    Ok(Some(bytes(&dir, n)?))
}
fn hash(v: &Option<Vec<u8>>) -> Option<String> {
    v.as_ref().map(|b| fsops::sha256_bytes(b))
}
fn target(
    ctx: &ToolContext<'_>,
    tool: &str,
    raw: &str,
    access: Access,
) -> Result<ResolvedPath, ToolFailure> {
    let p = ctx.resolve_any(raw)?;
    if p.relative.components().any(|c| {
        let s = c.as_os_str().to_string_lossy();
        s.eq_ignore_ascii_case(".bridge")
            || s.eq_ignore_ascii_case(".git")
            || s.chars().any(char::is_control)
    }) {
        return Err(ToolFailure::PermissionDenied(
            "binary recovery excludes bridge metadata and Git internals".into(),
        ));
    }
    ctx.check_read(tool, &p)?;
    ctx.recheck_access(access, &p, true)?;
    directory(
        p.absolute
            .parent()
            .ok_or_else(|| io("destination needs an existing parent"))?,
    )?;
    Ok(p)
}
fn capacity(dir: &File, needed: u64, reserve: u64) -> Result<(), ToolFailure> {
    let mut stat = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    if unsafe { libc::fstatvfs(dir.as_raw_fd(), stat.as_mut_ptr()) } != 0 {
        return Err(io("cannot inspect recovery free disk"));
    }
    let stat = unsafe { stat.assume_init() };
    let free = (stat.f_bavail as u64).saturating_mul(stat.f_frsize as u64);
    if free < needed.saturating_add(reserve) {
        return Err(io(format!(
            "insufficient free disk: {free} bytes available, {} required including reserve",
            needed.saturating_add(reserve)
        )));
    }
    Ok(())
}
// Enumerate through a descriptor, not a path which may have been replaced.
fn inventory(dir: &File) -> Result<(u64, usize), ToolFailure> {
    use std::os::fd::IntoRawFd;
    let cloned = dir.try_clone().map_err(io)?.into_raw_fd();
    let stream = unsafe { libc::fdopendir(cloned) };
    if stream.is_null() {
        unsafe { libc::close(cloned) };
        return Err(io(std::io::Error::last_os_error()));
    }
    struct Entries(*mut libc::DIR);
    impl Drop for Entries {
        fn drop(&mut self) {
            unsafe { libc::closedir(self.0) };
        }
    }
    let stream = Entries(stream);
    let mut size = 0u64;
    let mut count = 0;
    // dup shares the directory cursor, so reset it for each inventory.
    unsafe { libc::rewinddir(stream.0) };
    loop {
        #[cfg(target_os = "macos")]
        unsafe {
            *libc::__error() = 0;
        }
        #[cfg(target_os = "linux")]
        unsafe {
            *libc::__errno_location() = 0;
        }
        let entry = unsafe { libc::readdir(stream.0) };
        if entry.is_null() {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error().unwrap_or(0) != 0 {
                return Err(io(error));
            }
            break;
        }
        let raw = unsafe { std::ffi::CStr::from_ptr((*entry).d_name.as_ptr()) }.to_bytes();
        if raw == b"." || raw == b".." {
            continue;
        }
        count += 1;
        if count > 2000 {
            return Err(io(
                "recovery snapshot count exceeds hard limit; inspect storage",
            ));
        }
        let file = open_at(
            dir,
            OsStr::from_bytes(raw),
            libc::O_RDONLY | libc::O_NONBLOCK,
        )?;
        let meta = file.metadata().map_err(io)?;
        if !meta.is_file() || meta.nlink() != 1 {
            return Err(io("unexpected recovery storage entry; cleanup refused"));
        }
        size = size
            .checked_add(meta.len())
            .ok_or_else(|| io("recovery size overflow"))?;
    }
    Ok((size, count))
}
fn budget(dir: &File, bin: &File, settings: &Settings, new_bytes: u64) -> Result<(), ToolFailure> {
    settings.validate().map_err(io)?;
    let (saved, count) = inventory(dir)?;
    let used = saved.saturating_add(inventory(bin)?.0);
    if count >= settings.max_snapshots
        || used.saturating_add(new_bytes.saturating_mul(2)) > settings.max_storage_mib * MIB
    {
        return Err(io("recovery storage budget exhausted; use explicit cleanup or adjust settings before retrying"));
    }
    Ok(())
}
fn record(ctx: &ToolContext<'_>, c: &Change, event: &str) -> Result<(), ToolFailure> {
    ctx.app.store.save_file_change(c).map_err(io)?;
    ctx.app
        .audit_path_access(
            ctx.agent,
            ctx.session_id,
            event,
            Some(c.path.clone()),
            &c.state,
            &c.metadata().to_string(),
        )
        .map_err(io)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Replace {
    source: String,
    destination: String,
    source_sha256: String,
    expected_sha256: String,
}
pub async fn replace(ctx: &ToolContext<'_>, args: Value) -> Result<ToolOutcome, ToolFailure> {
    let args: Replace = parse_args(args)?;
    let _slot = SLOTS.try_acquire().map_err(|_| {
        conflict("another binary recovery operation is awaiting approval or running")
    })?;
    let source = binary::read_source(ctx, "replace_file", &args.source, MAX_BYTES)?;
    if fsops::sha256_bytes(&source) != args.source_sha256 {
        return Err(conflict("source hash changed; inspect it again"));
    }
    mutate(
        ctx,
        "replace_file",
        &args.destination,
        Some(&args.expected_sha256),
        Some(&source),
        Some((&args.source, &args.source_sha256)),
        None,
    )
    .await
}
pub async fn delete(
    ctx: &ToolContext<'_>,
    path: &str,
    expected: &str,
) -> Result<ToolOutcome, ToolFailure> {
    let _slot = SLOTS.try_acquire().map_err(|_| {
        conflict("another binary recovery operation is awaiting approval or running")
    })?;
    mutate(ctx, "delete_file", path, Some(expected), None, None, None).await
}
pub async fn restore(ctx: &ToolContext<'_>, c: Change) -> Result<ToolOutcome, ToolFailure> {
    let _slot = SLOTS.try_acquire().map_err(|_| {
        conflict("another binary recovery operation is awaiting approval or running")
    })?;
    target(
        ctx,
        "restore_change",
        &c.path,
        if c.before_sha256.is_some() {
            Access::Write
        } else {
            Access::Remove
        },
    )?;
    if c.state != "committed" {
        return Err(conflict(format!(
            "change {} is {}; inspect and reconcile before restoring",
            c.id, c.state
        )));
    }
    valid_id(&c.id)?;
    let saved = bytes(&private(ctx.app)?, OsStr::new(&c.id))?;
    if saved.len() as u64
        != c.binary_backup
            .as_ref()
            .ok_or_else(|| io("missing snapshot"))?
            .bytes
        || c.before_sha256
            .as_ref()
            .is_some_and(|h| *h != fsops::sha256_bytes(&saved))
    {
        return Err(io("recovery backup failed SHA256 verification"));
    }
    if c.before_sha256.is_none() && !saved.is_empty() {
        return Err(io("unexpected recovery data"));
    }
    mutate(
        ctx,
        "restore_change",
        &c.path,
        c.after_sha256.as_deref(),
        c.before_sha256.as_ref().map(|_| saved.as_slice()),
        None,
        Some(&c.id),
    )
    .await
}
async fn mutate(
    ctx: &ToolContext<'_>,
    tool: &str,
    raw: &str,
    expected: Option<&str>,
    content: Option<&[u8]>,
    source: Option<(&str, &str)>,
    restore_id: Option<&str>,
) -> Result<ToolOutcome, ToolFailure> {
    let access = if content.is_some() {
        Access::Write
    } else {
        Access::Remove
    };
    let initial = target(ctx, tool, raw, access)?;
    let before = snapshot(&initial)?;
    if hash(&before).as_deref() != expected {
        return Err(conflict(
            "destination changed or expected hash missing; inspect it again",
        ));
    }
    if before.is_none() && content.is_none() {
        return Err(conflict("file does not exist"));
    }
    let after = content.map(fsops::sha256_bytes);
    let operation = source
        .map(|(p, h)| format!("{tool} from {p} · source SHA256 {h}"))
        .unwrap_or_else(|| tool.to_string());
    ctx.app.request_approval(ctx.session_id,ctx.agent,tool,&format!("{}\nBefore: {} bytes · {}\nAfter: {} bytes · {}\nKeep original in recoverable trash and a private snapshot. Replacement briefly detaches the old file; conflicts preserve both versions for inspection.",operation,before.as_ref().map_or(0,Vec::len),expected.unwrap_or("absent"),content.map_or(0,<[u8]>::len),after.as_deref().unwrap_or("absent")),Some(initial.display())).await?;
    let _path = PathLock::acquire(&ctx.app.workspace.lock_dir, &initial.display()).map_err(io)?;
    let _storage = PathLock::acquire(
        &ctx.app.home.base.join("locks"),
        &format!("binary-recovery:{}", ctx.app.workspace.id()),
    )
    .map_err(io)?;
    let p = target(ctx, tool, raw, access)?;
    if p.absolute != initial.absolute || hash(&snapshot(&p)?).as_deref() != expected {
        return Err(conflict("destination changed while awaiting approval"));
    }
    if let Some((raw, h)) = source {
        if fsops::sha256_bytes(&binary::read_source(ctx, tool, raw, MAX_BYTES)?) != h {
            return Err(conflict("source changed while awaiting approval"));
        }
    }
    let parent = directory(p.absolute.parent().ok_or_else(|| io("missing parent"))?)?;
    let backups = private(ctx.app)?;
    if let Some(id) = restore_id {
        let original = ctx
            .app
            .store
            .file_change(id, &ctx.app.workspace.id())
            .map_err(io)?
            .ok_or_else(|| conflict("recovery receipt disappeared"))?;
        if original.state != "committed" {
            return Err(conflict(
                "backup was cleaned or changed while awaiting approval",
            ));
        }
        let retained = bytes(&backups, OsStr::new(id))?;
        if content.map(fsops::sha256_bytes) != original.before_sha256
            || (!retained.is_empty() && content.is_none())
            || content.is_some_and(|b| b != retained)
        {
            return Err(conflict("backup changed while awaiting approval"));
        }
    }
    let settings = ctx
        .app
        .store
        .recovery_settings(&ctx.app.workspace.id())
        .map_err(io)?;
    let n = before.as_ref().map_or(0, Vec::len) as u64;
    budget(&backups, &trash(ctx.app)?, &settings, n)?;
    let needed = n.saturating_add(content.map_or(0, |b| b.len()) as u64);
    // Both locations are checked conservatively even if they share a volume.
    capacity(&backups, needed, settings.reserve_mib * MIB)?;
    capacity(&parent, needed, settings.reserve_mib * MIB)?;
    let id = unique();
    let mut c = Change {
        id: id.clone(),
        workspace_id: ctx.app.workspace.id(),
        created_at: App::now(),
        agent: ctx.agent.dir().into(),
        path: p.display(),
        operation: tool.into(),
        before_sha256: expected.map(str::to_owned),
        after_sha256: after,
        state: "prepared".into(),
        before_base64: None,
        binary_backup: Some(Backup { bytes: n }),
        trash: expected.map(|_| format!(".bridge/trash/{id}")),
    };
    let mut backup = open_at(
        &backups,
        OsStr::new(&id),
        libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL,
    )?;
    backup
        .write_all(before.as_deref().unwrap_or_default())
        .map_err(io)?;
    backup.sync_all().map_err(io)?;
    backups.sync_all().map_err(io)?;
    record(ctx, &c, "change.prepare")?;
    commit(ctx, &mut c, &p, &parent, content, || Ok(()))?;
    Ok(ToolOutcome::write(
        "recoverable binary change committed",
        format!(
            "{} · change {}; original retained. Use list_changes / restore_change.",
            p.display(),
            id
        ),
        Some(p.display()),
    )
    .with_data(c.metadata()))
}

fn commit(
    ctx: &ToolContext<'_>,
    c: &mut Change,
    p: &ResolvedPath,
    parent: &File,
    content: Option<&[u8]>,
    before_publish: impl FnOnce() -> Result<(), ToolFailure>,
) -> Result<(), ToolFailure> {
    let id = c.id.clone();
    let expected = c.before_sha256.clone();
    let result = (|| -> Result<(), ToolFailure> {
        let mut stage = if let Some(data) = content {
            let mut s = Stage::new(parent.try_clone().map_err(io)?, false)?;
            s.file.write_all(data).map_err(io)?;
            s.file.sync_all().map_err(io)?;
            Some(s)
        } else {
            None
        };
        let current = directory(p.absolute.parent().unwrap())?
            .metadata()
            .map_err(io)?;
        let held = parent.metadata().map_err(io)?;
        if (current.dev(), current.ino()) != (held.dev(), held.ino()) {
            return Err(conflict("destination parent changed"));
        }
        if expected.is_some() {
            let bin = trash(ctx.app)?;
            rename(
                &parent,
                p.absolute.file_name().unwrap(),
                &bin,
                OsStr::new(&id),
            )?;
            c.state = "original_detached".into();
            record(ctx, c, "change.detached")?;
            if fsops::sha256_bytes(&bytes(&bin, OsStr::new(&id))?) != expected.as_deref().unwrap() {
                let returned = rename(
                    &bin,
                    OsStr::new(&id),
                    &parent,
                    p.absolute.file_name().unwrap(),
                );
                return Err(conflict(format!(
                    "original changed during detach; returning original: {}. Inspect change {}",
                    if returned.is_ok() {
                        "returned without overwrite"
                    } else {
                        "blocked; retained in trash"
                    },
                    id
                )));
            }
        }
        before_publish()?;
        if let Some(s) = stage.as_mut() {
            s.publish(&p.absolute)?;
        }
        Ok(())
    })();
    if let Err(e) = result {
        c.state = "needs_reconciliation".into();
        let recorded = record(ctx, c, "change.interrupted");
        return Err(io(format!("{e}; change {id} needs reconciliation; original/snapshot retained. Do not retry blindly. Journal: {recorded:?}")));
    }
    c.state = "committed".into();
    record(ctx, c, &format!("tool.{}", c.operation)).map_err(|e| {
        io(format!(
            "file changed; change {id} journal finalization failed: {e}; inspect before retrying"
        ))
    })?;
    Ok(())
}

fn eligible(c: &Change, s: &Settings) -> bool {
    c.state == "committed"
        && chrono::DateTime::parse_from_rfc3339(&c.created_at).is_ok_and(|d| {
            d.timestamp() <= chrono::Utc::now().timestamp() - i64::from(s.retention_days) * 86400
        })
}
pub fn control(app: &App, workspace: &str, action: Control) -> anyhow::Result<Value> {
    anyhow::ensure!(
        workspace == app.workspace.id(),
        "workspace changed; refresh recovery controls"
    );
    let _lock = PathLock::acquire(
        &app.home.base.join("locks"),
        &format!("binary-recovery:{workspace}"),
    )?;
    match action {
        Control::Settings { settings } => {
            settings.validate()?;
            app.audit_workspace(
                "recovery.settings",
                json!({"workspace_id":workspace,"settings":settings}),
            )?;
            app.store.save_recovery_settings(workspace, &settings)?;
        }
        Control::Cleanup { change_ids } => {
            anyhow::ensure!(
                !change_ids.is_empty() && change_ids.len() <= 100,
                "select 1–100 expired backups"
            );
            anyhow::ensure!(
                change_ids
                    .iter()
                    .collect::<std::collections::HashSet<_>>()
                    .len()
                    == change_ids.len(),
                "duplicate cleanup IDs"
            );
            let settings = app.store.recovery_settings(workspace)?;
            // Validate every selected record before touching any data. Interrupted
            // operations and modified originals require manual reconciliation.
            let changes = change_ids
                .iter()
                .map(|id| {
                    valid_id(id)?;
                    let c = app
                        .store
                        .file_change(id, workspace)?
                        .ok_or_else(|| anyhow::anyhow!("unknown change"))?;
                    anyhow::ensure!(
                        c.binary_backup.is_some() && eligible(&c, &settings),
                        "backup is unexpired or uncertain; cleanup refused"
                    );
                    Ok(c)
                })
                .collect::<anyhow::Result<Vec<_>>>()?;
            let backups = private(app)?;
            let bin = trash(app)?;
            for c in &changes {
                let saved = bytes(&backups, OsStr::new(&c.id))?;
                anyhow::ensure!(
                    saved.len() as u64 == c.binary_backup.as_ref().unwrap().bytes
                        && c.before_sha256
                            .as_ref()
                            .is_none_or(|h| *h == fsops::sha256_bytes(&saved)),
                    "snapshot corrupt; cleanup refused"
                );
                if let Some(h) = &c.before_sha256 {
                    anyhow::ensure!(
                        fsops::sha256_bytes(&bytes(&bin, OsStr::new(&c.id))?) == *h,
                        "retained original changed; cleanup refused"
                    );
                }
            }
            app.audit_workspace(
                "recovery.cleanup_requested",
                json!({"workspace_id":workspace,"change_ids":change_ids}),
            )?;
            for mut c in changes {
                c.state = "cleanup_pending".into();
                app.store.save_file_change(&c)?;
                // Each disposal revalidates after isolating the exact entry in
                // a private directory. Unexpected content is never unlinked.
                if let Some(h) = &c.before_sha256 {
                    dispose(&bin, &c.id, h)?;
                }
                let h = c
                    .before_sha256
                    .clone()
                    .unwrap_or_else(|| fsops::sha256_bytes(&[]));
                dispose(&backups, &c.id, &h)?;
                c.state = "backup_removed".into();
                app.store.save_file_change(&c)?;
                app.audit_workspace("recovery.cleaned", c.metadata())?;
            }
        }
        Control::Status {} => {}
    }
    let settings = app.store.recovery_settings(workspace)?;
    let (saved, count) = inventory(&private(app)?)?;
    let reserved = saved.saturating_add(inventory(&trash(app)?)?.0);
    let expired: Vec<_> = app
        .store
        .binary_changes(workspace)?
        .into_iter()
        .filter(|c| eligible(c, &settings))
        .take(100)
        .map(|c| c.metadata())
        .collect();
    Ok(
        json!({"workspace_id":workspace,"settings":settings,"reserved_bytes":reserved,"snapshots":count,"expired":expired,"cleanup":"Explicit only; originals and private snapshots removed together. Uncertain or modified backups are preserved. Existing text checkpoints are not included."}),
    )
}
fn dispose(dir: &File, id: &str, expected: &str) -> Result<(), ToolFailure> {
    let isolated = format!("cleanup-{}", unique());
    let held = child(dir, &isolated)?;
    rename(dir, OsStr::new(id), &held, OsStr::new("entry"))?;
    let result = bytes(&held, OsStr::new("entry")).and_then(|b| {
        if fsops::sha256_bytes(&b) == expected {
            Ok(())
        } else {
            Err(conflict("backup changed during cleanup"))
        }
    });
    if let Err(e) = result {
        let returned = rename(&held, OsStr::new("entry"), dir, OsStr::new(id));
        if returned.is_err() {
            return Err(io(format!(
                "{e}; preserved in {isolated}/entry; reconciliation required"
            )));
        }
        let n = name(OsStr::new(&isolated))?;
        unsafe { libc::unlinkat(dir.as_raw_fd(), n.as_ptr(), libc::AT_REMOVEDIR) };
        return Err(e);
    }
    let n = name(OsStr::new("entry"))?;
    if unsafe { libc::unlinkat(held.as_raw_fd(), n.as_ptr(), 0) } != 0 {
        return Err(io(std::io::Error::last_os_error()));
    }
    held.sync_all().map_err(io)?;
    let n = name(OsStr::new(&isolated))?;
    if unsafe { libc::unlinkat(dir.as_raw_fd(), n.as_ptr(), libc::AT_REMOVEDIR) } != 0 {
        return Err(io(std::io::Error::last_os_error()));
    }
    dir.sync_all().map_err(io)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        config::{Config, HomePaths},
        workspace::Workspace,
    };
    use bridge_core::{provider::Agent, workspace::Profile};
    fn fixture(t: &tempfile::TempDir) -> std::sync::Arc<App> {
        let base = t.path().join("home");
        let root = t.path().join("workspace");
        let home = HomePaths {
            base: base.clone(),
            config_file: base.join("config.toml"),
            database: base.join("bridge.sqlite3"),
            logs_dir: base.join("logs"),
            default_project: root.clone(),
            native_hosts_dir: base.join("hosts"),
            install_dir: base.join("bin"),
        };
        let workspace = Workspace::initialize_profile(&root, Profile::General).unwrap();
        let config = Config {
            project_root: workspace.root.clone(),
            ..Default::default()
        };
        home.save_config(&config).unwrap();
        App::open_with_config(home, config, workspace, true).unwrap()
    }
    #[test]
    fn detach_failure_and_independent_writer_preserve_original_and_journal_uncertainty() {
        for mode in ["failure", "editor"] {
            let t = tempfile::tempdir().unwrap();
            let app = fixture(&t);
            let ctx = ToolContext::new(&app, Agent::ChatGpt, "unit-session");
            let data = vec![41; 2 * 1024 * 1024];
            let out = vec![73; 2 * 1024 * 1024];
            let path = app.workspace.root.join("files/target.bin");
            std::fs::write(&path, &data).unwrap();
            let p = ctx.resolve_existing("files/target.bin").unwrap();
            let id = unique();
            let mut c = Change {
                id: id.clone(),
                workspace_id: app.workspace.id(),
                created_at: App::now(),
                agent: "CHATGPT".into(),
                path: p.display(),
                operation: "replace_file".into(),
                before_sha256: Some(fsops::sha256_bytes(&data)),
                after_sha256: Some(fsops::sha256_bytes(&out)),
                state: "prepared".into(),
                before_base64: None,
                binary_backup: Some(Backup {
                    bytes: data.len() as u64,
                }),
                trash: Some(format!(".bridge/trash/{id}")),
            };
            let dir = private(&app).unwrap();
            let mut saved = open_at(
                &dir,
                OsStr::new(&id),
                libc::O_CREAT | libc::O_EXCL | libc::O_WRONLY,
            )
            .unwrap();
            saved.write_all(&data).unwrap();
            saved.sync_all().unwrap();
            record(&ctx, &c, "change.prepare").unwrap();
            let err = commit(
                &ctx,
                &mut c,
                &p,
                &directory(path.parent().unwrap()).unwrap(),
                Some(&out),
                || {
                    if mode == "editor" {
                        std::fs::write(&path, b"independent editor").unwrap();
                        Ok(())
                    } else {
                        Err(io("injected interruption after detach"))
                    }
                },
            )
            .unwrap_err();
            assert!(err.to_string().contains("needs reconciliation"));
            assert_eq!(
                app.store
                    .file_change(&id, &app.workspace.id())
                    .unwrap()
                    .unwrap()
                    .state,
                "needs_reconciliation"
            );
            assert_eq!(bytes(&trash(&app).unwrap(), OsStr::new(&id)).unwrap(), data);
            assert_eq!(bytes(&dir, OsStr::new(&id)).unwrap(), data);
            if mode == "editor" {
                assert_eq!(std::fs::read(&path).unwrap(), b"independent editor");
            } else {
                assert!(!path.exists());
            }
            assert!(std::fs::read_dir(path.parent().unwrap())
                .unwrap()
                .all(|e| !e
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".bridge-archive-")));
            assert!(app.store.verify_audit().unwrap().valid);
        }
    }
    #[test]
    fn free_space_storage_limits_and_unknown_cleanup_entries_fail_closed() {
        let t = tempfile::tempdir().unwrap();
        let app = fixture(&t);
        let dir = private(&app).unwrap();
        let bin = trash(&app).unwrap();
        assert!(capacity(&dir, 0, 0).is_ok());
        assert!(capacity(&dir, u64::MAX - 1, 1).is_err());
        let file = open_at(
            &dir,
            OsStr::new("orphan"),
            libc::O_CREAT | libc::O_WRONLY | libc::O_EXCL,
        )
        .unwrap();
        file.set_len(65 * MIB).unwrap();
        let settings = Settings {
            max_storage_mib: 64,
            ..Default::default()
        };
        assert!(budget(&dir, &bin, &settings, 1).is_err());
        assert!(Settings {
            reserve_mib: 0,
            ..Default::default()
        }
        .validate()
        .is_err());
        child(&dir, "unreconciled").unwrap();
        assert!(inventory(&dir).is_err());
    }
    #[test]
    fn dispose_preserves_a_modified_backup() {
        let t = tempfile::tempdir().unwrap();
        let dir = directory(&t.path().canonicalize().unwrap()).unwrap();
        std::fs::write(t.path().join("backup"), b"later content").unwrap();
        assert!(dispose(&dir, "backup", &fsops::sha256_bytes(b"old content")).is_err());
        assert_eq!(
            std::fs::read(t.path().join("backup")).unwrap(),
            b"later content"
        );
        assert_eq!(std::fs::read_dir(t.path()).unwrap().count(), 1);
    }
}
