//! Local Git inspection with explicit repository boundaries, inert configuration,
//! bounded subprocess output and policy-filtered paths. No network or writes.
use super::{parse_args, ToolContext, ToolOutcome};
use crate::{
    failure::ToolFailure,
    sandbox::{Access, Decision, Existence, ResolvedPath, Sandbox},
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    path::PathBuf,
    process::Stdio,
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncRead, AsyncReadExt},
    process::Command,
};
fn invalid(s: impl Into<String>) -> ToolFailure {
    ToolFailure::InvalidArguments(s.into())
}
fn unsupported(s: impl Into<String>) -> ToolFailure {
    ToolFailure::Unsupported(s.into())
}
fn io(e: impl std::fmt::Display) -> ToolFailure {
    ToolFailure::Io(e.to_string())
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PathArgs {
    #[serde(default)]
    path: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DiffArgs {
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    staged: bool,
    #[serde(default)]
    files: Option<Vec<String>>,
}
struct Repo {
    path: ResolvedPath,
    git: PathBuf,
    disabled: Vec<String>,
    policy: Sandbox,
}
impl Repo {
    async fn open(ctx: &ToolContext<'_>, raw: Option<&str>) -> Result<Self, ToolFailure> {
        let default = if ctx.app.workspace.profile == bridge_core::workspace::Profile::Research {
            ctx.agent.workspace_relative()
        } else {
            ".".into()
        };
        let path = ctx.resolve_existing(raw.unwrap_or(&default))?;
        ctx.check_read("git", &path)?;
        if !path.is_dir {
            return Err(invalid("repository path must be a directory"));
        }
        super::artifacts::directory(&path.absolute)?;
        let git = path.absolute.join(".git");
        super::artifacts::directory(&git).map_err(|_|unsupported("select a repository root with an ordinary .git directory inside the workspace; parent search, bare repositories and linked worktrees are not supported"))?;
        let mut count = 0;
        let end = Instant::now() + Duration::from_secs(2);
        for entry in walkdir::WalkDir::new(&git).follow_links(false) {
            count += 1;
            if count > 10000 || Instant::now() >= end {
                return Err(unsupported("Git metadata exceeds the bounded 10,000-entry inspection; this repository needs a larger-repository adapter"));
            }
            let entry = entry.map_err(io)?;
            if entry.file_type().is_symlink() {
                return Err(ToolFailure::PermissionDenied(
                    "symlinks in Git metadata are refused".into(),
                ));
            }
            if !entry.file_type().is_dir() && !entry.file_type().is_file() {
                return Err(unsupported("non-regular Git metadata is unsupported"));
            }
        }
        for relative in [
            "commondir",
            "objects/info/alternates",
            "objects/info/http-alternates",
        ] {
            if git.join(relative).exists() {
                return Err(ToolFailure::PermissionDenied("shared or alternate Git object stores are outside this tool's supported root boundary".into()));
            }
        }
        // Reject includes before invoking Git: repository setup can inspect
        // configuration even before a subcommand's --no-includes is applied.
        {
            use std::io::Read;
            let dir = super::artifacts::directory(&git)?;
            let file = super::artifacts::open_at(
                &dir,
                std::ffi::OsStr::new("config"),
                libc::O_RDONLY | libc::O_NONBLOCK,
            )?;
            if !file.metadata().map_err(io)?.is_file() {
                return Err(unsupported("Git config must be regular"));
            }
            let mut config = Vec::new();
            file.take(65537).read_to_end(&mut config).map_err(io)?;
            if config.len() > 65536 {
                return Err(unsupported("Git config exceeds 64 KiB"));
            }
            let text = std::str::from_utf8(&config)
                .map_err(|_| unsupported("Git config must be UTF-8"))?;
            let compact = text
                .chars()
                .filter(|c| !c.is_whitespace() && *c != '\\')
                .collect::<String>()
                .to_ascii_lowercase();
            if compact.contains("[include") {
                return Err(unsupported(
                    "Git config includes are not loaded by bridge inspection",
                ));
            }
        }
        let mut repo = Self {
            path,
            git,
            disabled: vec![],
            policy: ctx.app.sandbox(),
        };
        let names = repo
            .run(
                &[
                    "config",
                    "--local",
                    "--no-includes",
                    "--null",
                    "--name-only",
                    "--list",
                ],
                32000,
            )
            .await?;
        let mut drivers = std::collections::BTreeSet::new();
        for key in names.split(|b| *b == 0).filter(|b| !b.is_empty()) {
            let key = std::str::from_utf8(key)
                .map_err(|_| unsupported("non-UTF8 Git configuration key"))?;
            if key.chars().any(char::is_control) {
                return Err(unsupported("control characters in Git configuration key"));
            }
            let lower = key.to_ascii_lowercase();
            if lower.starts_with("include.")
                || lower.starts_with("includeif.")
                || lower == "extensions.partialclone"
                || lower.ends_with(".promisor")
                || lower.ends_with(".partialclonefilter")
            {
                return Err(unsupported("Git config includes and partial clones require separate support; inspection will not load outside config or fetch missing objects"));
            }
            if let Some(rest) = key.get(7..).filter(|_| lower.starts_with("filter.")) {
                if let Some((driver, _)) = rest.rsplit_once('.') {
                    if driver.len() > 200 || drivers.len() >= 64 {
                        return Err(unsupported("Git filter configuration exceeds safe limits"));
                    }
                    drivers.insert(driver.to_string());
                }
            }
        }
        for driver in drivers {
            for attr in ["clean", "smudge", "process"] {
                repo.disabled.push(format!("filter.{driver}.{attr}="));
            }
            repo.disabled
                .push(format!("filter.{driver}.required=false"));
        }
        let declared = repo.run(&["rev-parse", "--show-toplevel"], 4096).await?;
        let declared = PathBuf::from(String::from_utf8(declared).map_err(io)?.trim());
        if declared != repo.path.absolute {
            return Err(ToolFailure::PermissionDenied(
                "Git worktree does not match the selected root".into(),
            ));
        }
        Ok(repo)
    }
    async fn run(&self, args: &[&str], limit: usize) -> Result<Vec<u8>, ToolFailure> {
        let mut command = Command::new("/usr/bin/git");
        command
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_OPTIONAL_LOCKS", "0")
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_NO_REPLACE_OBJECTS", "1")
            .env("GIT_NO_LAZY_FETCH", "1")
            .env("GIT_ATTR_NOSYSTEM", "1")
            .arg("--no-pager")
            .arg("--literal-pathspecs");
        for item in [
            "core.fsmonitor=false",
            "core.hooksPath=/dev/null",
            "core.attributesFile=/dev/null",
            "core.excludesFile=/dev/null",
            "diff.orderFile=/dev/null",
            "submodule.recurse=false",
            "diff.renames=false",
        ] {
            command.arg("-c").arg(item);
        }
        for item in &self.disabled {
            command.arg("-c").arg(item);
        }
        command
            .arg("--git-dir")
            .arg(&self.git)
            .arg("--work-tree")
            .arg(&self.path.absolute)
            .arg("-C")
            .arg(&self.path.absolute)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .process_group(0);
        let mut child = command
            .spawn()
            .map_err(|e| unsupported(format!("Git unavailable: {e}")))?;
        let pid = child.id();
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| io("Git stdout missing"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| io("Git stderr missing"))?;
        let result = tokio::time::timeout(Duration::from_secs(15), async {
            let (out, err) = tokio::try_join!(bounded(stdout, limit), bounded(stderr, 8192))?;
            let status = child.wait().await.map_err(io)?;
            if !status.success() {
                return Err(unsupported(format!(
                    "Git inspection failed: {}",
                    String::from_utf8_lossy(&err)
                        .chars()
                        .take(2000)
                        .collect::<String>()
                )));
            }
            Ok(out)
        })
        .await;
        match result {
            Ok(Ok(v)) => Ok(v),
            other => {
                if let Some(pid) = pid {
                    unsafe { libc::kill(-(pid as i32), libc::SIGKILL) };
                }
                let _ = child.kill().await;
                match other {
                    Ok(Err(e)) => Err(e),
                    _ => Err(io("Git inspection exceeded its 15-second limit")),
                }
            }
        }
    }
    fn readable(&self, ctx: &ToolContext<'_>, raw: &str) -> Option<String> {
        if raw.is_empty() || raw.len() > 2048 || raw.chars().any(char::is_control) {
            return None;
        }
        let absolute = self.path.absolute.join(raw);
        let relative = absolute.strip_prefix(ctx.workspace_root()).ok()?;
        let path = self
            .policy
            .resolve(&relative.to_string_lossy(), Existence::Any)
            .ok()?;
        if path.relative.components().any(|p| {
            p.as_os_str().to_string_lossy().eq_ignore_ascii_case(".git")
                || p.as_os_str()
                    .to_string_lossy()
                    .eq_ignore_ascii_case(".bridge")
        }) {
            return None;
        }
        (self.policy.decide(ctx.agent, Access::Read, &path) == Decision::Allow)
            .then(|| path.display())
    }
    async fn entries(&self) -> Result<Vec<(String, String)>, ToolFailure> {
        let out = self
            .run(
                &[
                    "status",
                    "--porcelain=v1",
                    "-z",
                    "--untracked-files=normal",
                    "--ignore-submodules=all",
                    "--no-renames",
                ],
                128000,
            )
            .await?;
        let mut entries = vec![];
        for row in out.split(|b| *b == 0).filter(|b| !b.is_empty()) {
            if row.len() < 4 || row[2] != b' ' {
                return Err(unsupported("unrecognized Git status format"));
            }
            let state = std::str::from_utf8(&row[..2]).map_err(io)?.to_string();
            let path = std::str::from_utf8(&row[3..])
                .map_err(|_| unsupported("non-UTF8 Git filenames are not supported"))?
                .to_string();
            entries.push((state, path));
            if entries.len() > 2000 {
                return Err(unsupported("Git status exceeds 2000 changed paths; narrow this repository before inspection"));
            }
        }
        Ok(entries)
    }
}
async fn bounded(reader: impl AsyncRead + Unpin, limit: usize) -> Result<Vec<u8>, ToolFailure> {
    let mut out = vec![];
    reader
        .take(limit as u64 + 1)
        .read_to_end(&mut out)
        .await
        .map_err(io)?;
    if out.len() > limit {
        return Err(unsupported(format!(
            "Git output exceeded {limit} bytes; no partial diff is presented as complete"
        )));
    }
    Ok(out)
}

pub async fn status(ctx: &ToolContext<'_>, args: Value) -> Result<ToolOutcome, ToolFailure> {
    let args: PathArgs = parse_args(args)?;
    let repo = Repo::open(ctx, args.path.as_deref()).await?;
    let raw = repo.entries().await?;
    let mut hidden = 0;
    let mut entries = vec![];
    let mut bytes = 0;
    let mut truncated = false;
    for (state, path) in raw {
        if let Some(path) = repo.readable(ctx, &path) {
            let entry = json!({"path":path,"status":state});
            let size = entry.to_string().len();
            if entries.len() >= 100 || bytes + size > 24000 {
                truncated = true;
            } else {
                bytes += size;
                entries.push(entry);
            }
        } else {
            hidden += 1;
        }
    }
    let text = entries
        .iter()
        .map(|e| {
            format!(
                "{} {}",
                e["status"].as_str().unwrap(),
                e["path"].as_str().unwrap()
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    Ok(ToolOutcome::read("bounded Git status",format!("Repository: {}\n{}\n{} protected paths omitted; truncated={truncated}. External filters and filesystem monitors are disabled for inspection.",repo.path.display(),if text.is_empty(){"No visible changes."}else{&text},hidden),Some(repo.path.display())).with_data(json!({"entries":entries,"hidden_paths":hidden,"truncated":truncated,"clean":entries.is_empty()&&hidden==0&&!truncated})))
}
pub async fn diff(ctx: &ToolContext<'_>, args: Value) -> Result<ToolOutcome, ToolFailure> {
    let args: DiffArgs = parse_args(args)?;
    let repo = Repo::open(ctx, args.path.as_deref()).await?;
    let raw = repo.entries().await?;
    let mut hidden = 0;
    let mut selected = vec![];
    let requested = if let Some(files) = args.files {
        if files.is_empty() || files.len() > 100 {
            return Err(invalid("select 1–100 workspace-relative files"));
        }
        let mut names = std::collections::BTreeSet::new();
        for raw in files {
            let p = ctx.resolve_any(&raw)?;
            ctx.check_read("git_diff", &p)?;
            let rel = p
                .absolute
                .strip_prefix(&repo.path.absolute)
                .map_err(|_| invalid("selected file is outside this repository"))?
                .to_string_lossy()
                .to_string();
            if repo.readable(ctx, &rel).is_none() {
                return Err(ToolFailure::PermissionDenied(
                    "selected Git file is protected".into(),
                ));
            }
            names.insert(rel);
        }
        Some(names)
    } else {
        None
    };
    for (state, path) in raw {
        let byte = state.as_bytes()[if args.staged { 0 } else { 1 }];
        if byte == b' ' || byte == b'?' {
            continue;
        }
        if requested.as_ref().is_some_and(|s| !s.contains(&path)) {
            continue;
        }
        if repo.readable(ctx, &path).is_some() {
            selected.push(path);
        } else {
            hidden += 1;
        }
    }
    if selected.len() > 100 {
        return Err(invalid(
            "diff exceeds 100 paths; use files to select a smaller review",
        ));
    }
    let visible_paths: Vec<_> = selected
        .iter()
        .filter_map(|p| repo.readable(ctx, p))
        .collect();
    if serde_json::to_vec(&visible_paths).map_err(io)?.len() > 16000 {
        return Err(invalid(
            "diff path metadata exceeds 16,000 bytes; select fewer files",
        ));
    }
    if selected.is_empty() {
        return Ok(ToolOutcome::read(
            "no visible Git differences",
            format!(
                "No {} changes in the permitted selection; {hidden} protected paths omitted.",
                if args.staged { "staged" } else { "unstaged" }
            ),
            Some(repo.path.display()),
        )
        .with_data(
            json!({"staged":args.staged,"paths":[],"hidden_paths":hidden,"truncated":false}),
        ));
    }
    let mut flags = vec![
        "diff",
        "--no-ext-diff",
        "--no-textconv",
        "--no-color",
        "--no-renames",
        "--ignore-submodules=all",
    ];
    if args.staged {
        flags.push("--cached");
    }
    flags.push("--");
    flags.extend(selected.iter().map(String::as_str));
    let output = repo.run(&flags, 48000).await?;
    let text = String::from_utf8_lossy(&output);
    if text.len() > 48000 {
        return Err(unsupported(
            "Git diff exceeds 48,000 bytes after text decoding; select fewer files",
        ));
    }
    Ok(ToolOutcome::read(
        "reviewed-path Git diff",
        format!(
            "{} diff; {hidden} protected paths omitted. External filters disabled.\n{text}",
            if args.staged { "Staged" } else { "Unstaged" }
        ),
        Some(repo.path.display()),
    )
    .with_data(
        json!({"staged":args.staged,"paths":visible_paths,"hidden_paths":hidden,"truncated":false}),
    ))
}
pub async fn inspect(ctx: &ToolContext<'_>, args: Value) -> Result<ToolOutcome, ToolFailure> {
    let args: PathArgs = parse_args(args)?;
    let repo = Repo::open(ctx, args.path.as_deref()).await?;
    let head = repo
        .run(&["symbolic-ref", "--short", "-q", "HEAD"], 4096)
        .await
        .ok()
        .and_then(|b| String::from_utf8(b).ok())
        .map(|s| s.trim().to_string());
    let mut entries = vec![];
    let mut truncated = false;
    let mut bytes = 0;
    for (seen, entry) in std::fs::read_dir(&repo.path.absolute)
        .map_err(io)?
        .enumerate()
    {
        if seen >= 200 {
            truncated = true;
            break;
        }
        if entries.len() >= 100 {
            truncated = true;
            break;
        }
        let entry = entry.map_err(io)?;
        let Some(raw) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if let Some(path) = repo.readable(ctx, &raw) {
            let size = serde_json::to_vec(&path).map_err(io)?.len();
            if bytes + size > 24000 {
                truncated = true;
                break;
            }
            bytes += size;
            entries.push(path);
        }
    }
    entries.sort();
    let mut readme = None;
    for candidate in ["README.md", "README.txt", "readme.md"] {
        if let Some(path) = repo.readable(ctx, candidate) {
            if let Ok(bytes) =
                super::binary::read_source(ctx, "inspect_repository", &path, 1024 * 1024)
            {
                if let Ok(text) = std::str::from_utf8(&bytes) {
                    readme = Some(text.chars().take(2000).collect::<String>());
                    break;
                }
            }
        }
    }
    let data = json!({"path":repo.path.display(),"branch":head,"entries":entries,"entries_truncated":truncated,"readme_excerpt":readme});
    Ok(ToolOutcome::read(
        "repository inspection",
        serde_json::to_string_pretty(&data).map_err(io)?,
        Some(repo.path.display()),
    )
    .with_data(data))
}
