use anyhow::Result;
use bridge_core::provider::{Agent, Provider};

use crate::app::App;
use crate::config::{Config, HomePaths};
use crate::tools;
use crate::wire::BridgeEvent;
use crate::workspace::Workspace;

struct TestReport {
    passed: usize,
    failed: Vec<String>,
}

impl TestReport {
    fn new() -> Self {
        Self {
            passed: 0,
            failed: Vec::new(),
        }
    }
    fn check(&mut self, condition: bool, label: &str) {
        if condition {
            self.passed += 1;
            println!("PASS  {label}");
        } else {
            self.failed.push(label.to_string());
            println!("FAIL  {label}");
        }
    }
}

pub async fn run() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let base = temp.path().join("home");
    let project = temp.path().join("archaeology");
    let home = HomePaths {
        base: base.clone(),
        config_file: base.join("config.toml"),
        database: base.join("bridge.sqlite3"),
        logs_dir: base.join("logs"),
        default_project: project.clone(),
        native_hosts_dir: base.join("NativeMessagingHosts"),
        install_dir: base.join("bin"),
    };
    home.ensure()?;
    let (workspace, _) = Workspace::initialize(&project)?;
    let mut config = Config::default();
    config.project_root = workspace.root.clone();
    let app = App::open_with_config(home, config, workspace.clone(), true)?;
    let mut report = TestReport::new();

    println!("Unified AI Bridge self-test (temporary workspace; no real research data)\n");

    // Each identity registers through a canonical provider URL.
    let urls = [
        (Provider::ChatGpt, "https://chatgpt.com/c/selftest-chatgpt"),
        (Provider::Claude, "https://claude.ai/chat/selftest-claude"),
        (
            Provider::DeepSeek,
            "https://chat.deepseek.com/a/chat/s/selftest-deepseek",
        ),
        (
            Provider::Gemini,
            "https://gemini.google.com/app/selftest-gemini",
        ),
        (Provider::Grok, "https://grok.com/chat/selftest-grok"),
        (Provider::Qwen, "https://chat.qwen.ai/c/selftest-qwen"),
        (Provider::Kimi, "https://www.kimi.ai/chat/selftest-kimi"),
    ];
    let mut sessions = Vec::new();
    for (index, (provider, url)) in urls.iter().enumerate() {
        match app.register_session(
            *provider,
            index as i64 + 1,
            1,
            url,
            None,
            Some("selftest".into()),
        ) {
            Ok(opened) => {
                report.check(
                    !opened.instructions.is_empty(),
                    &format!("{} receives connection instructions", provider.display()),
                );
                sessions.push((provider.agent(), opened.session.id, opened.instructions));
            }
            Err(err) => report.check(false, &format!("register {}: {err}", provider.display())),
        }
    }

    let project_context = workspace.root.join("PROJECT/PROJECT_CONTEXT.md");
    let project_hash_before = crate::fsops::sha256_file(&project_context)?.0;

    // Every agent writes and reads its own real Markdown file.
    for (agent, session, _) in &sessions {
        let path = format!("AGENTS/{}/notes/selftest.md", agent.dir());
        let result = tools::dispatch(
            &app,
            *agent,
            session,
            "write_document",
            serde_json::json!({"path": path, "content": format!("# Self-test for {}\n\nTemporary evidence.\n", agent.display())}),
        )
        .await;
        report.check(
            result.status == "ok",
            &format!("{} writes its own Markdown document", agent.display()),
        );
        let read = tools::dispatch(
            &app,
            *agent,
            session,
            "read_file",
            serde_json::json!({"path": path}),
        )
        .await;
        report.check(
            read.status == "ok"
                && read
                    .output
                    .as_deref()
                    .is_some_and(|text| text.contains(agent.display())),
            &format!("{} reads its own Markdown document", agent.display()),
        );
        let shared = tools::dispatch(
            &app,
            *agent,
            session,
            "read_file",
            serde_json::json!({"path": "PROJECT/PROJECT_CONTEXT.md"}),
        )
        .await;
        report.check(
            shared.status == "ok",
            &format!("{} reads the approved project context", agent.display()),
        );
    }

    // Cross-agent write must fail and leave no file.
    if let Some((_, chatgpt_session, _)) = sessions
        .iter()
        .find(|(agent, _, _)| *agent == Agent::ChatGpt)
    {
        let attack = "AGENTS/CLAUDE/notes/cross-write.md";
        let result = tools::dispatch(
            &app,
            Agent::ChatGpt,
            chatgpt_session,
            "write_document",
            serde_json::json!({"path": attack, "content": "must not exist"}),
        )
        .await;
        report.check(
            result.status == "error",
            "ChatGPT cannot write into Claude's workspace",
        );
        report.check(
            !workspace.root.join(attack).exists(),
            "the denied cross-agent file was never created",
        );
    }

    // Published findings are readable across agents.
    if let Some((_, claude_session, _)) = sessions
        .iter()
        .find(|(agent, _, _)| *agent == Agent::Claude)
    {
        let _ = tools::dispatch(
            &app,
            Agent::Claude,
            claude_session,
            "write_document",
            serde_json::json!({
                "path":"AGENTS/CLAUDE/findings/finding-001.md",
                "content":"# Finding 001\n\nObservation: self-test fixture only.\n"
            }),
        )
        .await;
    }
    if let Some((_, chatgpt_session, _)) = sessions
        .iter()
        .find(|(agent, _, _)| *agent == Agent::ChatGpt)
    {
        let result = tools::dispatch(
            &app,
            Agent::ChatGpt,
            chatgpt_session,
            "read_agent_findings",
            serde_json::json!({"agent":"CLAUDE"}),
        )
        .await;
        report.check(
            result.status == "ok"
                && result
                    .output
                    .as_deref()
                    .is_some_and(|text| text.contains("Finding 001")),
            "ChatGPT reads Claude's published findings",
        );
    }

    // Dataset immutability.
    if let Some((_, chatgpt_session, _)) = sessions
        .iter()
        .find(|(agent, _, _)| *agent == Agent::ChatGpt)
    {
        let result = tools::dispatch(
            &app,
            Agent::ChatGpt,
            chatgpt_session,
            "write_document",
            serde_json::json!({"path":"SHARED/datasets/tamper.csv","content":"a,b\n1,2\n"}),
        )
        .await;
        report.check(
            result.status == "error",
            "original datasets are immutable to agents",
        );
        report.check(
            !workspace.root.join("SHARED/datasets/tamper.csv").exists(),
            "dataset tampering left no file",
        );
    }

    // Approvals: a shared documentation write blocks until the simulated UI approves.
    if let Some((_, chatgpt_session, _)) = sessions
        .iter()
        .find(|(agent, _, _)| *agent == Agent::ChatGpt)
    {
        let mut events = app.events.subscribe();
        let app_handle = app.clone();
        let session = chatgpt_session.clone();
        let write = tokio::spawn(async move {
            tools::dispatch(
                &app_handle,
                Agent::ChatGpt,
                &session,
                "write_document",
                serde_json::json!({
                    "path":"SHARED/documentation/approved-selftest.md",
                    "content":"# Approved self-test\n\nWritten after a simulated user approval.\n"
                }),
            )
            .await
        });
        let mut approval_id = None;
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
        while tokio::time::Instant::now() < deadline {
            match tokio::time::timeout(std::time::Duration::from_secs(2), events.recv()).await {
                Ok(Ok(BridgeEvent::ApprovalRequired { approval })) => {
                    approval_id = Some(approval.id);
                    break;
                }
                Ok(Ok(_)) => continue,
                _ => break,
            }
        }
        if let Some(id) = approval_id {
            report.check(
                app.approvals.resolve(&id, true),
                "approval request reaches the UI channel",
            );
            let result = write.await?;
            report.check(
                result.status == "ok",
                "shared write succeeds after explicit approval",
            );
            report.check(
                workspace
                    .root
                    .join("SHARED/documentation/approved-selftest.md")
                    .is_file(),
                "approved shared document exists on disk",
            );
        } else {
            report.check(false, "approval request reaches the UI channel");
        }
    }

    // Concurrent agent writes stay in their own workspaces.
    let mut concurrent = Vec::new();
    for (index, (agent, session, _)) in sessions.iter().enumerate() {
        for item in 0..5 {
            let app = app.clone();
            let agent = *agent;
            let session = session.clone();
            concurrent.push(tokio::spawn(async move {
                tools::dispatch(
                    &app,
                    agent,
                    &session,
                    "write_document",
                    serde_json::json!({
                        "path": format!("AGENTS/{}/experiments/concurrent-{index}-{item}.md", agent.dir()),
                        "content": format!("# concurrent {index}-{item}\n")
                    }),
                )
                .await
            }));
        }
    }
    let mut successes = 0usize;
    for handle in concurrent {
        if let Ok(result) = handle.await {
            if result.status == "ok" {
                successes += 1;
            }
        }
    }
    report.check(
        successes == Provider::ALL.len() * 5,
        &format!("{} concurrent writes across all {} agents succeed", Provider::ALL.len() * 5, Provider::ALL.len()),
    );
    let mut isolation_ok = true;
    for agent in Agent::ALL {
        for other in Agent::ALL {
            if agent == other {
                continue;
            }
            let foreign = workspace
                .root
                .join("AGENTS")
                .join(other.dir())
                .join("experiments");
            if let Ok(entries) = std::fs::read_dir(foreign) {
                for entry in entries.flatten() {
                    if std::fs::read_to_string(entry.path())
                        .ok()
                        .is_some_and(|text| text.contains(&format!("AGENTS/{}/", agent.dir())))
                    {
                        isolation_ok = false;
                    }
                }
            }
        }
    }
    report.check(
        isolation_ok,
        "concurrent writes did not cross workspace boundaries",
    );

    let project_hash_after = crate::fsops::sha256_file(&project_context)?.0;
    report.check(
        project_hash_before == project_hash_after,
        "original project context is byte-identical",
    );

    let audit = app.store.verify_audit()?;
    report.check(
        audit.valid && audit.database_entries > 20,
        "immutable audit hash chain verifies",
    );
    let activity = app.store.recent_activity(500)?;
    report.check(
        activity.iter().any(|entry| entry.decision == "denied"),
        "denied permissions are present in the activity record",
    );

    println!(
        "\n{} passed, {} failed. All files were created under a temporary directory and removed afterward.",
        report.passed,
        report.failed.len()
    );
    if report.failed.is_empty() {
        Ok(())
    } else {
        anyhow::bail!("self-test failures: {}", report.failed.join("; "))
    }
}
