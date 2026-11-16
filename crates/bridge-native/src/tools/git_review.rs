//! A reviewed commit on a new branch. The working tree, HEAD and user's index
//! are never written. Prepared outcomes are journalled and never blindly rerun.
use super::{binary, git::Repo, parse_args, ToolContext, ToolOutcome};
use crate::{
    failure::ToolFailure,
    fsops::{self, PathLock},
    sandbox::{Access, Decision},
};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::os::unix::fs::PermissionsExt;

const TOOL: &str = "apply_git_commit";
fn invalid(s: impl Into<String>) -> ToolFailure {
    ToolFailure::InvalidArguments(s.into())
}
fn internal(e: impl std::fmt::Display) -> ToolFailure {
    ToolFailure::Internal(e.to_string())
}
fn conflict(s: impl Into<String>) -> ToolFailure {
    ToolFailure::Conflict(s.into())
}
fn unsupported(s: impl Into<String>) -> ToolFailure {
    ToolFailure::Unsupported(s.into())
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Preview {
    path: Option<String>,
    files: Vec<String>,
    branch: String,
    message: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Id {
    review_id: String,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct File {
    pub path: String,
    relative: String,
    pub sha256: String,
    mode: String,
    content: Vec<u8>,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Review {
    pub id: String,
    pub workspace_id: String,
    pub owner: String,
    path: String,
    pub base: String,
    pub branch: String,
    message: String,
    date: String,
    pub expires_at: i64,
    pub state: String,
    pub files: Vec<File>,
    diff: String,
    pub commit: Option<String>,
    pub error: Option<String>,
}
impl Review {
    fn metadata(&self) -> Value {
        json!({"review_id":self.id,"workspace_id":self.workspace_id,"repository":self.path,
            "base_commit":self.base,"branch":self.branch,"message":self.message,
            "author":"Unified AI Bridge <bridge@localhost>","date":self.date,
            "expires_at":self.expires_at,"state":self.state,"commit":self.commit,"error":self.error,
            "files":self.files.iter().map(|f|json!({"path":f.path,"sha256":f.sha256,"mode":f.mode,"bytes":f.content.len()})).collect::<Vec<_>>(),
            "working_tree_unchanged":true,"index_unchanged":true,"head_unchanged":true})
    }
    fn receipt(&self) -> ToolOutcome {
        ToolOutcome::read(format!("Git review {}: {}",self.id,self.state),
            "Inspect the receipt. No operation was repeated; interrupted publication requires reconciliation.",Some(self.path.clone())).with_data(self.metadata())
    }
    fn text(&self) -> String {
        format!("Create new branch {} from {}.\nAuthor: Unified AI Bridge <bridge@localhost>\nDate: {}\nMessage:\n{}\n\nCommit the selected working-file snapshots below, using a private index. The current branch, user's index and working files stay unchanged. Hooks, signing and content filters are disabled. Nothing is pushed.\n{}\n{}",
            self.branch,self.base,self.date,self.message,
            self.files.iter().map(|f|format!("{} · {} · {} bytes · mode {}",f.path,f.sha256,f.content.len(),f.mode)).collect::<Vec<_>>().join("\n"),self.diff)
    }
}
fn id() -> String {
    let mut b = [0; 16];
    rand::rngs::OsRng.fill_bytes(&mut b);
    hex::encode(b)
}
fn oid(bytes: Vec<u8>) -> Result<String, ToolFailure> {
    let value = String::from_utf8(bytes)
        .map_err(internal)?
        .trim()
        .to_owned();
    if ![40, 64].contains(&value.len()) || !value.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(unsupported("Git did not return a supported object ID"));
    }
    Ok(value)
}
async fn head(repo: &Repo) -> Result<String, ToolFailure> {
    oid(repo
        .run(&["rev-parse", "--verify", "HEAD^{commit}"], 128)
        .await?)
}
async fn repository_scope(
    ctx: &ToolContext<'_>,
    repo: &Repo,
    base: &str,
) -> Result<(), ToolFailure> {
    if ctx.app.workspace.profile == bridge_core::workspace::Profile::Research
        && !repo
            .path
            .relative
            .starts_with(ctx.agent.workspace_relative())
    {
        return Err(ToolFailure::PermissionDenied(
            "research commits require a repository inside the agent's own workspace".into(),
        ));
    }
    let tree = repo
        .run(&["ls-tree", "-r", "-z", "--name-only", base], 1_000_000)
        .await?;
    if tree.iter().filter(|b| **b == 0).count() > 5000 {
        return Err(unsupported("commit baseline exceeds 5,000 files"));
    }
    Ok(())
}
async fn branch_absent(repo: &Repo, branch: &str) -> Result<(), ToolFailure> {
    let full = format!("refs/heads/{branch}");
    if repo.run(&["symbolic-ref", "-q", &full], 8192).await.is_ok() {
        return Err(conflict("the proposed branch is an existing symbolic ref"));
    }
    let out = repo
        .run(&["for-each-ref", "--format=%(refname)", &full], 8192)
        .await?;
    if !out.is_empty() {
        return Err(conflict(
            "the proposed branch already exists or has child refs; choose a new branch",
        ));
    }
    Ok(())
}
fn write_allowed(ctx: &ToolContext<'_>, raw: &str) -> Result<(), ToolFailure> {
    let p = ctx.resolve_existing(raw)?;
    ctx.check_read(TOOL, &p)?;
    if let Decision::Deny(reason) = ctx.app.current_write_decision(ctx.agent, Access::Write, &p) {
        return Err(ToolFailure::PermissionDenied(reason));
    }
    Ok(())
}
fn mode(ctx: &ToolContext<'_>, raw: &str) -> Result<String, ToolFailure> {
    let p = ctx.resolve_existing(raw)?;
    let meta = std::fs::symlink_metadata(&p.absolute).map_err(internal)?;
    if !meta.is_file() || meta.file_type().is_symlink() {
        return Err(unsupported("commit selection must contain regular files"));
    }
    Ok(if meta.permissions().mode() & 0o111 != 0 {
        "100755"
    } else {
        "100644"
    }
    .into())
}
async fn baseline(
    repo: &Repo,
    base: &str,
    relative: &str,
) -> Result<(String, Vec<u8>), ToolFailure> {
    // A new nested file must not implicitly replace a committed parent file or
    // symlink. Such a replacement would remove an unselected baseline path.
    let parents: Vec<String> = std::path::Path::new(relative)
        .ancestors()
        .skip(1)
        .filter(|p| !p.as_os_str().is_empty())
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    if !parents.is_empty() {
        let mut args = vec!["ls-tree", "-z", base, "--"];
        args.extend(parents.iter().map(String::as_str));
        let rows = repo.run(&args, 16000).await?;
        for row in rows.split(|b| *b == 0).filter(|r| !r.is_empty()) {
            if !row.starts_with(b"040000 tree ") {
                return Err(unsupported(
                    "selected file would replace an unselected committed parent file or symlink",
                ));
            }
        }
    }
    let row = repo
        .run(&["ls-tree", "-z", base, "--", relative], 4096)
        .await?;
    if row.is_empty() {
        return Ok(("new".into(), vec![]));
    }
    let text = std::str::from_utf8(&row).map_err(|_| unsupported("non-UTF8 Git tree entry"))?;
    let (info, path) = text
        .trim_end_matches('\0')
        .split_once('\t')
        .ok_or_else(|| unsupported("unexpected Git tree entry"))?;
    let fields: Vec<_> = info.split(' ').collect();
    if fields.len() != 3
        || !["100644", "100755"].contains(&fields[0])
        || fields[1] != "blob"
        || path != relative
    {
        return Err(unsupported("selected baseline is not an ordinary file; trees, submodules and symlinks are unsupported"));
    }
    let object = oid(fields[2].as_bytes().to_vec())?;
    let bytes = repo.run(&["cat-file", "blob", &object], 512000).await?;
    Ok((fields[0].into(), bytes))
}
pub async fn preview(ctx: &ToolContext<'_>, args: Value) -> Result<ToolOutcome, ToolFailure> {
    let args: Preview = parse_args(args)?;
    if args.files.is_empty() || args.files.len() > 20 {
        return Err(invalid("select 1–20 existing UTF-8 working files"));
    }
    if !args.branch.starts_with("bridge/")
        || args.branch.len() > 120
        || !args
            .branch
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"/-_.".contains(&b))
    {
        return Err(invalid("use a new branch under bridge/ (ASCII letters, digits, slash, dash, underscore, dot; at most 120 bytes)"));
    }
    if args.message.trim().is_empty()
        || args.message.len() > 2000
        || args
            .message
            .chars()
            .any(|c| c.is_control() && c != '\n' && c != '\t')
    {
        return Err(invalid(
            "commit message must be 1–2000 bytes without control characters",
        ));
    }
    let repo = Repo::open(ctx, args.path.as_deref()).await?;
    let full = format!("refs/heads/{}", args.branch);
    repo.run(&["check-ref-format", &full], 1024).await?;
    branch_absent(&repo, &args.branch).await?;
    let base = head(&repo).await?;
    repository_scope(ctx, &repo, &base).await?;
    let mut files = vec![];
    let mut diff = String::new();
    let mut total = 0;
    let mut names = std::collections::BTreeSet::new();
    let mut changed = false;
    for raw in args.files {
        write_allowed(ctx, &raw)?;
        let path = ctx.resolve_existing(&raw)?;
        let relative = path
            .absolute
            .strip_prefix(&repo.path.absolute)
            .map_err(|_| invalid("selected file is outside the repository"))?
            .to_string_lossy()
            .into_owned();
        if repo.readable(ctx, &relative).is_none() {
            return Err(ToolFailure::PermissionDenied(
                "selected Git path is protected".into(),
            ));
        }
        if !names.insert(path.display()) {
            return Err(invalid("duplicate commit file"));
        }
        let content = binary::read_source(ctx, TOOL, &path.display(), 512000)?;
        let (old_mode, old) = baseline(&repo, &base, &relative).await?;
        total += content.len() + old.len();
        if total > 512000 {
            return Err(invalid(
                "combined baseline and selected files exceed 512,000 bytes",
            ));
        }
        let before = std::str::from_utf8(&old)
            .map_err(|_| unsupported("binary baseline requires a different review workflow"))?;
        let after = std::str::from_utf8(&content)
            .map_err(|_| unsupported("commit review supports UTF-8 text files only"))?;
        if before.contains('\0') || after.contains('\0') {
            return Err(unsupported(
                "NUL-containing files are unsupported in text commit review",
            ));
        }
        let mode = mode(ctx, &path.display())?;
        changed |= old_mode == "new" || old_mode != mode || old != content;
        diff.push_str(&format!(
            "\n{} · mode {old_mode} → {mode}\n{}",
            path.display(),
            similar::TextDiff::configure()
                .timeout(std::time::Duration::from_millis(250))
                .diff_lines(before, after)
                .unified_diff()
                .context_radius(3)
                .header(&format!("a/{relative}"), &format!("b/{relative}"))
                .to_string()
        ));
        files.push(File {
            path: path.display(),
            relative,
            sha256: fsops::sha256_bytes(&content),
            mode,
            content,
        });
        if diff.len() > 44000 {
            return Err(invalid(
                "commit diff exceeds 44,000 bytes; review a smaller selection",
            ));
        }
    }
    if !changed {
        return Err(invalid("selected working files do not differ from HEAD"));
    }
    let review = Review {
        id: id(),
        workspace_id: ctx.app.workspace.id(),
        owner: crate::jobs::owner(ctx)?,
        path: repo.path.display(),
        base,
        branch: args.branch,
        message: args.message,
        date: crate::app::App::now(),
        expires_at: chrono::Utc::now().timestamp() + 1800,
        state: "previewed".into(),
        files,
        diff,
        commit: None,
        error: None,
    };
    let text = review.text();
    if text.len() > 50000 {
        return Err(invalid(
            "full commit review exceeds 50,000 bytes; select fewer files",
        ));
    }
    validate(ctx, &review).await?;
    ctx.app.store.save_git_review(&review).map_err(internal)?;
    audit(ctx, &review, "git.preview", "allowed")?;
    Ok(ToolOutcome::read(
        "Git commit preview saved; no repository changes",
        text,
        Some(review.path.clone()),
    )
    .with_data(review.metadata()))
}
fn owned(ctx: &ToolContext<'_>, id: &str) -> Result<Review, ToolFailure> {
    let owner = crate::jobs::owner(ctx)?;
    let review = ctx
        .app
        .store
        .git_review(id, &ctx.app.workspace.id())
        .map_err(internal)?
        .filter(|r| r.owner == owner)
        .ok_or_else(|| {
            ToolFailure::PermissionDenied(
                "Git review belongs to another workspace or conversation".into(),
            )
        })?;
    for file in &review.files {
        let path = ctx.resolve_any(&file.path)?;
        ctx.check_read("git_commit_status", &path)?;
    }
    Ok(review)
}
fn audit(
    ctx: &ToolContext<'_>,
    r: &Review,
    action: &str,
    decision: &str,
) -> Result<(), ToolFailure> {
    ctx.app
        .audit_path_access(
            ctx.agent,
            ctx.session_id,
            action,
            Some(r.path.clone()),
            decision,
            &r.metadata().to_string(),
        )
        .map_err(internal)
}
async fn validate(ctx: &ToolContext<'_>, r: &Review) -> Result<Repo, ToolFailure> {
    if chrono::Utc::now().timestamp() >= r.expires_at {
        return Err(ToolFailure::PermissionDenied(
            "Git preview expired; request a fresh preview".into(),
        ));
    }
    let repo = Repo::open(ctx, Some(&r.path)).await?;
    repository_scope(ctx, &repo, &r.base).await?;
    if head(&repo).await? != r.base {
        return Err(conflict("HEAD changed since preview"));
    }
    branch_absent(&repo, &r.branch).await?;
    for file in &r.files {
        write_allowed(ctx, &file.path)?;
        let p = ctx.resolve_existing(&file.path)?;
        if p.absolute != repo.path.absolute.join(&file.relative)
            || repo.readable(ctx, &file.relative).is_none()
        {
            return Err(conflict("selected path changed"));
        }
        let bytes = binary::read_source(ctx, TOOL, &file.path, 512000)?;
        if fsops::sha256_bytes(&bytes) != file.sha256 || mode(ctx, &file.path)? != file.mode {
            return Err(conflict(format!("{} changed since preview", file.path)));
        }
    }
    Ok(repo)
}
pub async fn apply(ctx: &ToolContext<'_>, args: Value) -> Result<ToolOutcome, ToolFailure> {
    let args: Id = parse_args(args)?;
    let initial = owned(ctx, &args.review_id)?;
    let _guard = PathLock::acquire(
        &ctx.app.home.base.join("locks"),
        &format!("git-review:{}", initial.id),
    )
    .map_err(internal)?;
    let mut r = owned(ctx, &initial.id)?;
    if r.state == "committed" {
        return Ok(r.receipt());
    }
    if r.state != "previewed" {
        return Err(conflict("review already claimed, denied or interrupted; inspect git_commit_status before creating a new preview"));
    }
    validate(ctx, &r).await?;
    r.state = "awaiting_approval".into();
    ctx.app.store.save_git_review(&r).map_err(internal)?;
    if let Err(e) = ctx
        .app
        .request_approval(
            ctx.session_id,
            ctx.agent,
            TOOL,
            &r.text(),
            Some(r.path.clone()),
        )
        .await
    {
        r.state = "denied".into();
        r.error = Some(e.to_string());
        ctx.app.store.save_git_review(&r).map_err(internal)?;
        return Err(e);
    }
    let repo = match validate(ctx, &r).await {
        Ok(repo) => repo,
        Err(e) => {
            r.state = "stale".into();
            r.error = Some(e.to_string());
            ctx.app.store.save_git_review(&r).map_err(internal)?;
            return Err(e);
        }
    };
    // The receipt precedes object writes. Dangling objects after a failure are
    // harmless Git storage; we never delete objects to "undo" an uncertain run.
    r.state = "preparing".into();
    ctx.app.store.save_git_review(&r).map_err(internal)?;
    audit(ctx, &r, "git.prepare", "approved")?;
    let result = publish(ctx, &repo, &mut r).await;
    match result {
        Ok(()) => {
            r.state = "committed".into();
            ctx.app.store.save_git_review(&r).map_err(internal)?;
            audit(ctx, &r, "git.commit", "approved")?;
            Ok(ToolOutcome::write("Reviewed commit created on a new branch",format!("{} → {}. Current branch, working files and user's index were preserved. Nothing was pushed.",r.branch,r.commit.as_deref().unwrap_or_default()),Some(r.path.clone())).with_data(r.metadata()))
        }
        Err(e) => {
            r.state = if r.state == "publishing" {
                "needs_reconciliation"
            } else {
                "failed"
            }
            .into();
            r.error = Some(e.to_string());
            ctx.app.store.save_git_review(&r).map_err(internal)?;
            audit(ctx, &r, "git.commit_failed", "error")?;
            Err(e)
        }
    }
}
async fn publish(ctx: &ToolContext<'_>, repo: &Repo, r: &mut Review) -> Result<(), ToolFailure> {
    let temp = tempfile::Builder::new()
        .prefix("git-index-")
        .tempdir_in(&ctx.app.home.base)
        .map_err(internal)?;
    let index = temp.path().join("index");
    repo.run_input(&["read-tree", &r.base], 1024, None, Some(&index), None)
        .await?;
    let mut update = Vec::new();
    for file in &r.files {
        write_allowed(ctx, &file.path)?;
        let hash = oid(repo
            .run_input(
                &["hash-object", "-w", "--no-filters", "--stdin"],
                128,
                Some(&file.content),
                None,
                None,
            )
            .await?)?;
        update.extend_from_slice(format!("{} {}\t{}\0", file.mode, hash, file.relative).as_bytes());
    }
    repo.run_input(
        &["update-index", "-z", "--index-info"],
        1024,
        Some(&update),
        Some(&index),
        None,
    )
    .await?;
    let tree = oid(repo
        .run_input(&["write-tree"], 128, None, Some(&index), None)
        .await?)?;
    let commit = oid(repo
        .run_input(
            &[
                "commit-tree",
                &tree,
                "-p",
                &r.base,
                "--no-gpg-sign",
                "-F",
                "-",
            ],
            128,
            Some(r.message.as_bytes()),
            None,
            Some(&r.date),
        )
        .await?)?;
    r.commit = Some(commit.clone());
    let changed = repo
        .run(
            &[
                "diff-tree",
                "--no-ext-diff",
                "--no-textconv",
                "--no-renames",
                "--no-commit-id",
                "--name-only",
                "-r",
                "-z",
                &r.base,
                &commit,
                "--",
            ],
            48000,
        )
        .await?;
    let mut count = 0;
    for path in changed.split(|b| *b == 0).filter(|p| !p.is_empty()) {
        if !r.files.iter().any(|f| f.relative.as_bytes() == path) {
            return Err(conflict(
                "prepared commit changes an unselected path; no branch was published",
            ));
        }
        count += 1;
    }
    if count == 0 {
        return Err(conflict("prepared commit has no selected changes"));
    }
    // Refresh configuration/metadata policy after preparation and verify the
    // selected files again. The ref transaction checks HEAD and creates only a
    // previously absent branch atomically; it never moves HEAD or existing refs.
    let repo = validate(ctx, r).await?;
    r.state = "publishing".into();
    ctx.app.store.save_git_review(r).map_err(internal)?;
    let transaction = format!(
        "start\nverify HEAD {}\noption no-deref\ncreate refs/heads/{} {}\nprepare\ncommit\n",
        r.base, r.branch, commit
    );
    repo.run_input(
        &["update-ref", "--stdin"],
        2048,
        Some(transaction.as_bytes()),
        None,
        None,
    )
    .await?;
    Ok(())
}
pub async fn status(ctx: &ToolContext<'_>, args: Value) -> Result<ToolOutcome, ToolFailure> {
    let args: Id = parse_args(args)?;
    let mut r = owned(ctx, &args.review_id)?;
    if matches!(
        r.state.as_str(),
        "awaiting_approval" | "preparing" | "publishing" | "needs_reconciliation"
    ) {
        if let Ok(_guard) = PathLock::acquire(
            &ctx.app.home.base.join("locks"),
            &format!("git-review:{}", r.id),
        ) {
            r = owned(ctx, &r.id)?;
            if matches!(
                r.state.as_str(),
                "awaiting_approval" | "preparing" | "publishing" | "needs_reconciliation"
            ) {
                let mut published = false;
                if let Some(commit) = &r.commit {
                    let repo = Repo::open(ctx, Some(&r.path)).await?;
                    let full = format!("refs/heads/{}", r.branch);
                    let out = repo
                        .run(
                            &["for-each-ref", "--format=%(refname) %(objectname)", &full],
                            8192,
                        )
                        .await?;
                    published = String::from_utf8_lossy(&out)
                        .lines()
                        .any(|l| l == format!("{full} {commit}"));
                }
                r.state = if published {
                    "committed"
                } else {
                    "needs_reconciliation"
                }
                .into();
                ctx.app.store.save_git_review(&r).map_err(internal)?;
                audit(
                    ctx,
                    &r,
                    "git.reconcile",
                    if published { "committed" } else { "uncertain" },
                )?;
            }
        }
    }
    Ok(r.receipt())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Log {
    path: Option<String>,
    commit: Option<String>,
    limit: Option<usize>,
    offset: Option<usize>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Show {
    path: Option<String>,
    commit: String,
    file: String,
}
async fn commit_id(repo: &Repo, value: Option<String>) -> Result<String, ToolFailure> {
    let id = match value {
        Some(v) => oid(v.into_bytes())?,
        None => head(repo).await?,
    };
    if repo.run(&["cat-file", "-t", &id], 128).await? != b"commit\n" {
        return Err(invalid(
            "select a full commit object ID returned by git_log",
        ));
    }
    Ok(id)
}
pub async fn log(ctx: &ToolContext<'_>, args: Value) -> Result<ToolOutcome, ToolFailure> {
    let args: Log = parse_args(args)?;
    let limit = args.limit.unwrap_or(10);
    let offset = args.offset.unwrap_or(0);
    if !(1..=20).contains(&limit) || offset > 10000 {
        return Err(invalid("log limit must be 1–20 and offset at most 10,000"));
    }
    let repo = Repo::open(ctx, args.path.as_deref()).await?;
    let base = commit_id(&repo, args.commit).await?;
    let out = repo
        .run(
            &[
                "log",
                "--no-show-signature",
                "--no-decorate",
                "--format=%H%x00%ct%x00%s%x00",
                &format!("--max-count={}", limit + 1),
                &format!("--skip={offset}"),
                &base,
                "--",
            ],
            24000,
        )
        .await?;
    let fields: Vec<_> = out.split(|b| *b == 0).collect();
    let mut entries = vec![];
    for row in fields.chunks(3) {
        if row.len() < 3 {
            if row
                .iter()
                .any(|r| r.iter().any(|b| !b.is_ascii_whitespace()))
            {
                return Err(unsupported("unsupported Git log framing"));
            }
            break;
        }
        let id = oid(row[0].to_vec())?;
        let time = std::str::from_utf8(row[1])
            .map_err(internal)?
            .parse::<i64>()
            .map_err(internal)?;
        let subject =
            std::str::from_utf8(row[2]).map_err(|_| unsupported("non-UTF8 commit subject"))?;
        entries.push(json!({"commit":id,"timestamp":time,"subject":subject}));
    }
    let next = (entries.len() > limit).then_some(offset + limit);
    entries.truncate(limit);
    let data = json!({"commits":entries,"base_commit":base,"next_offset":next});
    Ok(ToolOutcome::read(
        "Local Git history",
        serde_json::to_string_pretty(&data).map_err(internal)?,
        Some(repo.path.display()),
    )
    .with_data(data))
}
pub async fn show(ctx: &ToolContext<'_>, args: Value) -> Result<ToolOutcome, ToolFailure> {
    let args: Show = parse_args(args)?;
    let repo = Repo::open(ctx, args.path.as_deref()).await?;
    let commit = commit_id(&repo, Some(args.commit)).await?;
    let p = ctx.resolve_any(&args.file)?;
    ctx.check_read("git_show", &p)?;
    let relative = p
        .absolute
        .strip_prefix(&repo.path.absolute)
        .map_err(|_| invalid("historical file is outside this repository"))?
        .to_string_lossy()
        .into_owned();
    if repo.readable(ctx, &relative).is_none() {
        return Err(ToolFailure::PermissionDenied(
            "historical Git path is protected".into(),
        ));
    }
    let (mode, bytes) = baseline(&repo, &commit, &relative).await?;
    if mode == "new" {
        return Err(ToolFailure::NotFound(
            "file is absent from that commit".into(),
        ));
    }
    if bytes.len() > 48000 {
        return Err(unsupported(
            "historical file exceeds the 48,000-byte Git show budget",
        ));
    }
    let text = std::str::from_utf8(&bytes)
        .map_err(|_| unsupported("historical file is not UTF-8 text"))?;
    if text.contains('\0') {
        return Err(unsupported("historical file contains binary NUL bytes"));
    }
    Ok(ToolOutcome::read("Historical Git file",text,Some(p.display())).with_data(json!({"commit":commit,"path":p.display(),"mode":mode,"sha256":fsops::sha256_bytes(&bytes),"bytes":bytes.len(),"truncated":false})))
}
