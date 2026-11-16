use std::fs;
use std::path::PathBuf;

use chrono::Utc;
use serde::Deserialize;
use serde_json::Value;
use walkdir::WalkDir;

use bridge_core::provider::Agent;

use crate::failure::ToolFailure;
use crate::tools::{parse_agent_arg, parse_args, ToolContext, ToolOutcome};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AgentArgs {
    agent: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FindingsArgs {
    agent: String,
    #[serde(default)]
    limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProposalArgs {
    title: String,
    body: String,
    #[serde(default)]
    related_paths: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReviewArgs {
    target_agent: String,
    target_path: String,
    verdict: String,
    body: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DisagreementArgs {
    topic: String,
    positions: String,
    analysis: String,
}

pub async fn list_agents(ctx: &ToolContext<'_>) -> Result<ToolOutcome, ToolFailure> {
    let mut output = String::from(
        "# Agent workspaces\n\n| Agent | Workspace | Findings | Proposals |\n|---|---|---:|---:|\n",
    );
    let mut agents = serde_json::Map::new();
    for agent in Agent::ALL {
        let workspace = ctx.workspace_root().join(agent.workspace_relative());
        let findings = count_files(&workspace.join("findings"), &["md", "txt"]);
        let proposals = count_matching(
            &ctx.workspace_root().join("COLLABORATION/proposals"),
            &agent.dir().to_ascii_lowercase(),
        );
        output.push_str(&format!(
            "| {} | `AGENTS/{}/` | {} | {} |\n",
            agent.display(),
            agent.dir(),
            findings,
            proposals
        ));
        agents.insert(
            agent.dir().to_string(),
            serde_json::json!({
                "workspace": agent.workspace_relative(),
                "findings": findings,
                "proposals": proposals,
            }),
        );
    }
    output.push_str(
        "\nEach agent is independent. Published findings are the only readable research content in another agent's workspace.",
    );
    Ok(
        ToolOutcome::read("listed agent workspaces", output, None)
            .with_data(serde_json::json!({"agents": agents})),
    )
}

pub async fn inspect_agent_workspace(
    ctx: &ToolContext<'_>,
    args: Value,
) -> Result<ToolOutcome, ToolFailure> {
    let args: AgentArgs = parse_args(args)?;
    let target = parse_agent_arg(&args.agent)?;
    let base = ctx.workspace_root().join(target.workspace_relative());
    if !base.is_dir() {
        return Err(ToolFailure::NotFound(format!(
            "workspace for {} does not exist",
            target.display()
        )));
    }
    let readme_path = base.join("README.md");
    let readme = if readme_path.is_file() {
        fs::read_to_string(&readme_path)
            .map_err(|err| ToolFailure::Io(format!("read agent README: {err}")))?
    } else {
        "(no README)".into()
    };
    let mut structure = String::new();
    for name in ["notes", "findings", "experiments", "reports", "logs"] {
        let path = base.join(name);
        let files = if path.is_dir() {
            count_files(&path, &[])
        } else {
            0
        };
        structure.push_str(&format!("- {name}/ ({files} files)\n"));
    }
    let output = format!(
        "# {} workspace\n\nStructure:\n{}\nREADME:\n\n{}\n\nOther agents' notes, experiments, reports, and logs are not readable; only findings are exposed through `read_agent_findings`.",
        target.display(),
        structure,
        readme.trim()
    );
    Ok(ToolOutcome::read(
        format!("inspected {}'s workspace structure", target.display()),
        output,
        Some(target.workspace_relative()),
    )
    .with_data(serde_json::json!({
        "agent": target.dir(),
        "workspace": target.workspace_relative(),
        "findings": count_files(&base.join("findings"), &[]),
    })))
}

pub async fn read_agent_findings(
    ctx: &ToolContext<'_>,
    args: Value,
) -> Result<ToolOutcome, ToolFailure> {
    let args: FindingsArgs = parse_args(args)?;
    let target = parse_agent_arg(&args.agent)?;
    let limit = args.limit.unwrap_or(10).clamp(1, 25);
    let base = ctx
        .workspace_root()
        .join(target.workspace_relative())
        .join("findings");
    if !base.is_dir() {
        return Ok(ToolOutcome::read(
            format!("{} has no findings folder", target.display()),
            format!("{} has no published findings yet.", target.display()),
            Some(format!("AGENTS/{}/findings", target.dir())),
        ));
    }
    let mut files: Vec<PathBuf> = WalkDir::new(&base)
        .max_depth(1)
        .follow_links(false)
        .into_iter()
        .filter_map(|entry| entry.ok())
        .filter(|entry| {
            entry.file_type().is_file()
                && entry
                    .path()
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .is_some_and(|ext| matches!(ext.to_ascii_lowercase().as_str(), "md" | "txt"))
        })
        .map(|entry| entry.path().to_path_buf())
        .collect();
    files.sort();
    if files.is_empty() {
        return Ok(ToolOutcome::read(
            format!("{} has no published findings", target.display()),
            format!("{} has no published findings yet.", target.display()),
            Some(format!("AGENTS/{}/findings", target.dir())),
        ));
    }
    let mut output = format!("# {} — published findings\n\n", target.display());
    let mut read = 0usize;
    let mut entries = Vec::new();
    for file in files.into_iter().take(limit) {
        let relative = match file.strip_prefix(ctx.workspace_root()) {
            Ok(relative) => relative.to_string_lossy().to_string(),
            Err(_) => continue,
        };
        let resolved = ctx.resolve_existing(&relative)?;
        // The permission matrix restricts files outside findings; this is the
        // documented cross-agent read path.
        ctx.check_read("read_agent_findings", &resolved)?;
        let text = fs::read_to_string(&file)
            .map_err(|err| ToolFailure::Io(format!("read {}: {err}", relative)))?;
        output.push_str(&format!(
            "\n## `{relative}`\n\n_Modified {} · {} bytes_\n\n{}\n\n---\n",
            fs::metadata(&file)
                .ok()
                .and_then(|meta| meta.modified().ok())
                .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                .and_then(|duration| chrono::DateTime::from_timestamp(duration.as_secs() as i64, 0))
                .map(|dt| dt.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
                .unwrap_or_else(|| "unknown".into()),
            text.len(),
            text
        ));
        entries.push(relative);
        read += 1;
    }
    Ok(ToolOutcome::read(
        format!(
            "read {read} of {} findings from {}",
            entries.len(),
            target.display()
        ),
        output,
        Some(format!("AGENTS/{}/findings", target.dir())),
    )
    .with_data(serde_json::json!({"agent": target.dir(), "files": entries})))
}

pub async fn submit_research_proposal(
    ctx: &ToolContext<'_>,
    args: Value,
) -> Result<ToolOutcome, ToolFailure> {
    let args: ProposalArgs = parse_args(args)?;
    if args.title.trim().is_empty() || args.body.trim().is_empty() {
        return Err(ToolFailure::InvalidArguments(
            "title and body are required".into(),
        ));
    }
    if args.related_paths.len() > 50 {
        return Err(ToolFailure::InvalidArguments(
            "too many related paths".into(),
        ));
    }
    let mut markdown = format!(
        "# Proposal: {}\n\n- **Author:** {}\n- **Created:** {}\n- **Status:** open proposal (not an accepted finding)\n\n{}\n",
        args.title.trim(),
        ctx.agent.display(),
        Utc::now().to_rfc3339(),
        args.body.trim()
    );
    if !args.related_paths.is_empty() {
        markdown.push_str("\n## Related workspace paths\n\n");
        for path in args.related_paths {
            markdown.push_str(&format!("- `{}`\n", path));
        }
    }
    markdown.push_str(
        "\n---\n\nAttribution is preserved. This proposal does not modify any agent's findings.\n",
    );
    let path = unique_path(
        ctx,
        "COLLABORATION/proposals",
        &format!(
            "{}-{}",
            ctx.agent.dir().to_ascii_lowercase(),
            slug(&args.title)
        ),
        "md",
    )?;
    let (resolved, written) = ctx
        .write_text("submit_research_proposal", &path, &markdown, true)
        .await?;
    Ok(ToolOutcome::write(
        format!("submitted proposal {}", resolved.display()),
        format!(
            "Created proposal {} ({} bytes).",
            resolved.display(),
            written.bytes
        ),
        Some(resolved.display()),
    )
    .with_data(serde_json::json!({"path": resolved.display(), "author": ctx.agent.dir()})))
}

pub async fn submit_peer_review(
    ctx: &ToolContext<'_>,
    args: Value,
) -> Result<ToolOutcome, ToolFailure> {
    let args: ReviewArgs = parse_args(args)?;
    let verdict = args.verdict.to_ascii_lowercase();
    if !matches!(
        verdict.as_str(),
        "supports" | "challenges" | "mixed" | "insufficient_evidence"
    ) {
        return Err(ToolFailure::InvalidArguments(
            "verdict must be supports, challenges, mixed, or insufficient_evidence".into(),
        ));
    }
    if args.body.trim().is_empty() {
        return Err(ToolFailure::InvalidArguments(
            "review body is required".into(),
        ));
    }
    let target = parse_agent_arg(&args.target_agent)?;
    let expected_prefix = format!("AGENTS/{}/findings/", target.dir());
    if !args.target_path.starts_with(&expected_prefix) {
        return Err(ToolFailure::PermissionDenied(format!(
            "peer reviews may target only files under {expected_prefix}"
        )));
    }
    let target_path = ctx.resolve_existing(&args.target_path)?;
    ctx.check_read("submit_peer_review", &target_path)?;
    let target_text = fs::read_to_string(&target_path.absolute)
        .map_err(|err| ToolFailure::Io(format!("read reviewed finding: {err}")))?;
    if target_text.len() > 2 * 1024 * 1024 {
        return Err(ToolFailure::InvalidArguments(
            "reviewed finding is too large".into(),
        ));
    }
    let markdown = format!(
        "# Peer review\n\n- **Reviewer:** {}\n- **Target agent:** {}\n- **Target finding:** `{}`\n- **Verdict:** `{}`\n- **Created:** {}\n\n## Review\n\n{}\n\n## Reviewed finding fingerprint\n\n- Length: {} bytes\n- SHA-256: {}\n\n---\n\nThe target finding was not modified. This review is one agent's assessment, not proof.\n",
        ctx.agent.display(),
        target.display(),
        args.target_path,
        verdict,
        Utc::now().to_rfc3339(),
        args.body.trim(),
        target_text.len(),
        crate::fsops::sha256_bytes(target_text.as_bytes())
    );
    let path = unique_path(
        ctx,
        "COLLABORATION/peer_reviews",
        &format!(
            "{}-reviews-{}-{}",
            ctx.agent.dir().to_ascii_lowercase(),
            target.dir().to_ascii_lowercase(),
            slug(&target_path.relative.to_string_lossy())
        ),
        "md",
    )?;
    let (resolved, written) = ctx
        .write_text("submit_peer_review", &path, &markdown, true)
        .await?;
    Ok(ToolOutcome::write(
        format!("submitted peer review of {}", target.display()),
        format!(
            "Created review {} ({} bytes). Verdict: {}. Target unchanged.",
            resolved.display(),
            written.bytes,
            verdict
        ),
        Some(resolved.display()),
    )
    .with_data(serde_json::json!({
        "path": resolved.display(),
        "reviewer": ctx.agent.dir(),
        "target_agent": target.dir(),
        "target_path": args.target_path,
        "verdict": verdict,
    })))
}

pub async fn record_disagreement(
    ctx: &ToolContext<'_>,
    args: Value,
) -> Result<ToolOutcome, ToolFailure> {
    let args: DisagreementArgs = parse_args(args)?;
    if args.topic.trim().is_empty() || args.positions.trim().is_empty() {
        return Err(ToolFailure::InvalidArguments(
            "topic and positions are required".into(),
        ));
    }
    let markdown = format!(
        "# Disagreement: {}\n\n- **Recorded by:** {}\n- **Created:** {}\n- **Status:** preserved disagreement (do not collapse automatically)\n\n## Positions\n\n{}\n\n## Analysis\n\n{}\n\n---\n\nOriginal positions are preserved without declaring a winner. Evidence comparison and verification are separate tasks.\n",
        args.topic.trim(),
        ctx.agent.display(),
        Utc::now().to_rfc3339(),
        args.positions.trim(),
        args.analysis.trim()
    );
    let path = unique_path(
        ctx,
        "COLLABORATION/disagreements",
        &format!(
            "{}-{}",
            ctx.agent.dir().to_ascii_lowercase(),
            slug(&args.topic)
        ),
        "md",
    )?;
    let (resolved, written) = ctx
        .write_text("record_disagreement", &path, &markdown, true)
        .await?;
    Ok(ToolOutcome::write(
        format!("recorded disagreement {}", resolved.display()),
        format!(
            "Created disagreement record {} ({} bytes).",
            resolved.display(),
            written.bytes
        ),
        Some(resolved.display()),
    )
    .with_data(serde_json::json!({"path": resolved.display(), "recorded_by": ctx.agent.dir()})))
}

fn count_files(path: &std::path::Path, extensions: &[&str]) -> usize {
    if !path.is_dir() {
        return 0;
    }
    WalkDir::new(path)
        .max_depth(8)
        .follow_links(false)
        .into_iter()
        .filter_map(|entry| entry.ok())
        .filter(|entry| {
            entry.file_type().is_file()
                && (extensions.is_empty()
                    || entry
                        .path()
                        .extension()
                        .and_then(|ext| ext.to_str())
                        .is_some_and(|ext| {
                            let ext = ext.to_ascii_lowercase();
                            extensions.iter().any(|allowed| *allowed == ext)
                        }))
        })
        .count()
}

fn count_matching(path: &std::path::Path, needle: &str) -> usize {
    if !path.is_dir() {
        return 0;
    }
    fs::read_dir(path)
        .ok()
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.ok())
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .to_ascii_lowercase()
                .contains(needle)
        })
        .count()
}

fn unique_path(
    ctx: &ToolContext<'_>,
    directory: &str,
    stem: &str,
    extension: &str,
) -> Result<String, ToolFailure> {
    let base = ctx.resolve_any(directory)?;
    if !base.exists || base.is_dir {
        let stamp = Utc::now().format("%Y%m%dT%H%M%S%.3fZ");
        let stem = format!("{stamp}-{stem}");
        let mut candidate = format!("{directory}/{stem}.{extension}");
        let mut suffix = 1u32;
        while ctx.resolve_any(&candidate)?.exists {
            candidate = format!("{directory}/{stem}-{suffix}.{extension}");
            suffix += 1;
            if suffix > 100 {
                return Err(ToolFailure::Internal(
                    "could not allocate a unique artifact name".into(),
                ));
            }
        }
        Ok(candidate)
    } else {
        Err(ToolFailure::InvalidArguments(format!(
            "{directory} is a file, not a folder"
        )))
    }
}

fn slug(input: &str) -> String {
    let mut output = String::new();
    for ch in input.chars().take(60) {
        if ch.is_ascii_alphanumeric() {
            output.push(ch.to_ascii_lowercase());
        } else if (ch.is_whitespace() || matches!(ch, '-' | '_' | '.')) && !output.ends_with('-') {
            output.push('-');
        }
    }
    let trimmed = output.trim_matches('-').to_string();
    if trimmed.is_empty() {
        "artifact".into()
    } else {
        trimmed
    }
}
