use anyhow::{Context, Result};
use std::sync::Arc;
use std::time::Instant;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{mpsc, Semaphore};

use bridge_core::{MAX_MESSAGE_BYTES, PROTOCOL_VERSION};

use crate::app::App;
use crate::config::HomePaths;
use crate::store::{RequestClaim, ResponseRequest};
use crate::tools;
use crate::wire::{ExtToHost, HostToExt, ToolResultMessage};

struct HostLog {
    path: std::path::PathBuf,
}

impl HostLog {
    fn new(path: std::path::PathBuf) -> Self {
        Self { path }
    }
    fn write(&self, level: &str, message: &str) {
        use std::io::Write;
        if let Some(parent) = self.path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        // Serialize rotation across host processes; audit.jsonl is never rotated.
        let lock_path = self.path.with_extension("log.lock");
        let Ok(lock) = std::fs::OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .open(lock_path)
        else {
            return;
        };
        if fs2::FileExt::lock_exclusive(&lock).is_err() {
            return;
        }
        if std::fs::metadata(&self.path).is_ok_and(|m| m.len() > 5 * 1024 * 1024) {
            for index in (1..=3).rev() {
                let destination = self.path.with_extension(format!("log.{index}"));
                let source = if index == 1 {
                    self.path.clone()
                } else {
                    self.path.with_extension(format!("log.{}", index - 1))
                };
                if source.exists() {
                    let _ = std::fs::rename(source, destination);
                }
            }
        }
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
        {
            let _ = writeln!(
                file,
                "{} [{level}] {}",
                chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
                message
            );
        }
    }
}

/// Chrome launches the binary with no arguments. stdout carries only binary
/// framed messages; diagnostics go to a log file and stderr.
pub async fn run() -> Result<()> {
    let home = HomePaths::discover()?;
    home.ensure()?;
    let log = HostLog::new(home.logs_dir.join("native-host.log"));
    log.write("info", "native host starting");

    let mut app = match App::load(home.clone(), true) {
        Ok(app) => app,
        Err(err) => {
            log.write("error", &format!("cannot load bridge configuration: {err}"));
            let mut stdout = tokio::io::stdout();
            write_message(
                &mut stdout,
                &HostToExt::Error {
                    code: "setup_required".into(),
                    message: format!(
                        "Unified AI Bridge is not configured. Run setup with a project directory. ({err})"
                    ),
                    request_key: None,
                },
            )
            .await?;
            return Ok(());
        }
    };

    let (tx, rx) = mpsc::channel::<HostToExt>(256);
    let writer_tx = tx.clone();
    let writer = tokio::spawn(async move { writer_loop(writer_tx, rx).await });

    let mut events = app.events.subscribe();
    let event_tx = tx.clone();
    let event_log = log.path.clone();
    let event_forwarder = tokio::spawn(async move {
        let log = HostLog::new(event_log);
        loop {
            match events.recv().await {
                Ok(event) => {
                    if event_tx.send(HostToExt::Event { event }).await.is_err() {
                        break;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(count)) => {
                    log.write("warn", &format!("event stream lagged by {count} messages"));
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    });

    let mut stdin = tokio::io::stdin();
    let hello: std::sync::Arc<std::sync::atomic::AtomicBool> =
        Arc::new(std::sync::atomic::AtomicBool::new(false));
    let semaphore = Arc::new(Semaphore::new(8));
    let mut session_count = 0usize;
    let mut connected_extension_version = None;
    let in_flight = Arc::new(std::sync::atomic::AtomicUsize::new(0));

    loop {
        let frame = match read_frame(&mut stdin).await? {
            Some(frame) => frame,
            None => break,
        };
        let message = match serde_json::from_slice::<ExtToHost>(&frame) {
            Ok(message) => message,
            Err(err) => {
                log.write("warn", &format!("invalid extension message: {err}"));
                let request_key = serde_json::from_slice::<serde_json::Value>(&frame)
                    .ok()
                    .and_then(|v| {
                        v.get("request_key")
                            .and_then(|k| k.as_str())
                            .map(str::to_owned)
                    });
                let _ = tx
                    .send(HostToExt::Error {
                        code: "invalid_message".into(),
                        message: format!("could not parse extension message: {err}"),
                        request_key,
                    })
                    .await;
                continue;
            }
        };
        if !is_ready(&hello)
            && !matches!(&message, ExtToHost::Hello { .. } | ExtToHost::Ping { .. })
        {
            let key = serde_json::from_slice::<serde_json::Value>(&frame)
                .ok()
                .and_then(|value| value["request_key"].as_str().map(str::to_owned));
            send_error(
                &tx,
                "hello_required",
                "send a compatible hello before bridge operations".into(),
                key,
            )
            .await;
            continue;
        }
        match message {
            ExtToHost::Hello {
                protocol,
                extension_version,
            } => {
                if protocol != PROTOCOL_VERSION {
                    hello.store(false, std::sync::atomic::Ordering::SeqCst);
                    log.write(
                        "error",
                        &format!(
                            "protocol mismatch: extension {protocol}, host {PROTOCOL_VERSION}"
                        ),
                    );
                    send_error(
                        &tx,
                        "protocol_mismatch",
                        format!(
                            "extension protocol {protocol} does not match host protocol {PROTOCOL_VERSION}; rebuild the extension and host together"
                        ),
                        None,
                    )
                    .await;
                    continue;
                }
                hello.store(true, std::sync::atomic::Ordering::SeqCst);
                connected_extension_version = Some(extension_version.clone());
                log.write("info", &format!("extension hello ({extension_version})"));
                let maybe_audit = app.store.verify_audit().map(|v| v.valid).unwrap_or(false);
                let _ = maybe_audit;
                let _ = tx
                    .send(HostToExt::HelloAck {
                        protocol: PROTOCOL_VERSION,
                        host_version: env!("CARGO_PKG_VERSION").into(),
                        interactive: true,
                        workspace_root: app.workspace.root.to_string_lossy().to_string(),
                        project_name: app.workspace.project_name(),
                        native_host_name: bridge_core::NATIVE_HOST_NAME.into(),
                        tool_timeout_secs: app.config.tool_timeout_secs,
                        workspace_id: app.workspace.id(),
                        profile: app.workspace.profile,
                    })
                    .await;
            }
            ExtToHost::RegisterSession {
                provider,
                tab_id,
                window_id,
                url,
                conversation,
                auto_connect,
            } => {
                if !is_ready(&hello) {
                    send_error(
                        &tx,
                        "hello_required",
                        "send hello before registering a session".into(),
                        None,
                    )
                    .await;
                    continue;
                }
                let provider = match provider.parse::<bridge_core::provider::Provider>() {
                    Ok(provider) => provider,
                    Err(err) => {
                        send_error(&tx, "unsupported_provider", err.to_string(), None).await;
                        continue;
                    }
                };
                match app.register_session(
                    provider,
                    tab_id,
                    window_id,
                    &url,
                    conversation,
                    connected_extension_version.clone(),
                ) {
                    Ok(opened) => {
                        session_count = app.store.count_sessions().unwrap_or(session_count + 1);
                        log.write(
                            "info",
                            &format!(
                                "registered {} tab {} as {} conversation={:?} workspace={:?} extension={:?}",
                                provider.display(),
                                tab_id,
                                opened.session.id,
                                opened.session.conversation,
                                app.workspace.root,
                                opened.session.extension_version
                            ),
                        );
                        let _ = tx
                            .send(HostToExt::SessionRegistered {
                                session: opened.session,
                                instructions: opened.instructions,
                                auto_connect,
                                capabilities: serde_json::to_value(app.capabilities())?,
                            })
                            .await;
                    }
                    Err(err) => {
                        log.write("warn", &format!("session registration rejected: {err}"));
                        send_error(&tx, "registration_failed", err.to_string(), None).await;
                    }
                }
            }
            ExtToHost::ToolRequest {
                session_id,
                workspace_id,
                request_key,
                response_id,
                task_id,
                call_index,
                tool,
                arguments,
            } => {
                if !is_ready(&hello) {
                    send_error(
                        &tx,
                        "hello_required",
                        "send hello before tool calls".into(),
                        Some(request_key),
                    )
                    .await;
                    continue;
                }
                if request_key.is_empty() || request_key.len() > 200 {
                    send_error(
                        &tx,
                        "invalid_request_key",
                        "request_key is empty or too long".into(),
                        None,
                    )
                    .await;
                    continue;
                }
                let Some((session, agent)) = app.session_agent(&session_id).ok().flatten() else {
                    send_error(
                        &tx,
                        "unknown_session",
                        "this browser session is not registered with the native host".into(),
                        Some(request_key),
                    )
                    .await;
                    continue;
                };
                if workspace_id != app.workspace.id() || session.workspace_id != workspace_id {
                    send_error(&tx, "workspace_changed", "Workspace changed. Refresh session instructions before submitting more calls.".into(), Some(request_key)).await;
                    continue;
                }
                let Some(conversation) = session.conversation.as_deref() else {
                    send_error(
                        &tx,
                        "conversation_required",
                        "Wait for the website to assign a conversation ID.".into(),
                        Some(request_key),
                    )
                    .await;
                    continue;
                };
                if !bridge_core::execution::valid_turn_id(&response_id)
                    || !bridge_core::execution::valid_turn_id(&task_id)
                    || request_key
                        != bridge_core::execution::request_key(
                            &session.provider,
                            conversation,
                            &response_id,
                            call_index,
                        )
                {
                    send_error(
                        &tx,
                        "invalid_response_identity",
                        "Invalid browser response identity or request key.".into(),
                        Some(request_key),
                    )
                    .await;
                    continue;
                }
                let scope =
                    serde_json::to_string(&(&app.workspace.root, &session.provider, conversation))?;
                let claim = match app.store.claim_response(ResponseRequest {
                    scope: &scope,
                    response: &response_id,
                    task: &task_id,
                    index: call_index,
                    tool: &tool,
                    arguments: &arguments,
                    response_limit: app.config.max_calls_per_response,
                    task_limit: app.config.max_calls_per_task,
                    timeout_secs: app.config.tool_timeout_secs,
                }) {
                    Ok(claim) => claim,
                    Err(err) => {
                        send_error(&tx, "store_error", err.to_string(), Some(request_key)).await;
                        continue;
                    }
                };
                let rejected = matches!(&claim, RequestClaim::Rejected(_));
                match claim {
                    RequestClaim::Completed(json) | RequestClaim::Rejected(json) => {
                        if let Ok(result) = serde_json::from_str::<ToolResultMessage>(&json) {
                            if rejected {
                                let reason = result.error.as_deref().unwrap_or("request rejected");
                                let path =
                                    arguments.get("path").and_then(serde_json::Value::as_str);
                                let _ = app.audit_denial(
                                    agent,
                                    &session_id,
                                    &tool,
                                    path,
                                    reason,
                                    "denied",
                                );
                                let _ = app.record_activity(
                                    agent,
                                    &session_id,
                                    &tool,
                                    "denial",
                                    path,
                                    reason,
                                    "denied",
                                );
                                log.write("warn", &format!("tool {tool} denied session={session_id} provider={} conversation={conversation} workspace={:?} request={request_key} code={:?} reason={reason}", session.provider, app.workspace.root, result.code));
                            } else {
                                log.write("info", &format!("replayed cached result request={request_key} session={session_id} response={response_id}"));
                            }
                            let _ = tx
                                .send(HostToExt::ToolResult {
                                    session_id: session_id.clone(),
                                    request_key,
                                    result,
                                    duration_ms: 0,
                                })
                                .await;
                        } else {
                            send_error(
                                &tx,
                                "cached_result_corrupt",
                                "cached result could not be read".into(),
                                Some(request_key),
                            )
                            .await;
                        }
                    }
                    RequestClaim::InProgress => {
                        send_error(
                            &tx,
                            "request_in_progress",
                            "this request is already running; wait for its result".into(),
                            Some(request_key),
                        )
                        .await;
                    }
                    RequestClaim::New => {
                        let app = app.clone();
                        let tx = tx.clone();
                        let semaphore = semaphore.clone();
                        let log_path = log.path.clone();
                        let tool_for_task = tool.clone();
                        in_flight.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        let in_flight = in_flight.clone();
                        tokio::spawn(async move {
                            let _running = RunningRequest(in_flight);
                            let started = Instant::now();
                            let path = arguments
                                .get("destination")
                                .or_else(|| arguments.get("path"))
                                .and_then(serde_json::Value::as_str)
                                .map(str::to_owned);
                            let execution = async {
                                let _permit = semaphore.acquire_owned().await;
                                tools::dispatch(&app, agent, &session_id, &tool_for_task, arguments)
                                    .await
                            };
                            let result = match tokio::time::timeout(
                                std::time::Duration::from_secs(app.config.tool_timeout_secs),
                                execution,
                            )
                            .await
                            {
                                Ok(result) => result,
                                Err(_) => {
                                    let reason = format!("Tool execution exceeded {} seconds. Check activity before requesting another write; this request will not run again.", app.config.tool_timeout_secs);
                                    let _ = app.audit_denial(
                                        agent,
                                        &session_id,
                                        &tool_for_task,
                                        path.as_deref(),
                                        &reason,
                                        "timeout",
                                    );
                                    ToolResultMessage::error(
                                        &tool_for_task,
                                        "request_timeout",
                                        reason,
                                    )
                                }
                            };
                            let duration_ms = started.elapsed().as_millis() as u64;
                            let json = serde_json::to_string(&result).unwrap_or_default();
                            let status = if result.status == "ok" {
                                "complete"
                            } else {
                                "error"
                            };
                            if let Err(err) =
                                app.store
                                    .finish_response(&scope, &response_id, call_index, &json)
                            {
                                HostLog::new(log_path.clone()).write(
                                    "error",
                                    &format!(
                                        "could not persist terminal response {request_key}: {err}"
                                    ),
                                );
                            }
                            HostLog::new(log_path).write(
                                "info",
                                &format!(
                                    "tool {} for {} finished in {}ms ({}) session={} provider={} tab={} conversation={:?} workspace={:?} path={:?} request={} bytes={}",
                                    tool_for_task,
                                    agent.dir(),
                                    duration_ms,
                                    if status == "complete" { "ok" } else { "error" },
                                    session_id, session.provider, session.tab_id, session.conversation,
                                    app.workspace.root, path, request_key, json.len()
                                ),
                            );
                            let _ = tx
                                .send(HostToExt::ToolResult {
                                    session_id,
                                    request_key,
                                    result,
                                    duration_ms,
                                })
                                .await;
                        });
                    }
                }
            }
            ExtToHost::ApprovalDecision { approval_id, allow } => {
                let resolved = app.approvals.resolve(&approval_id, allow);
                log.write(
                    "info",
                    &format!("approval {approval_id} resolved={allow} known={resolved}"),
                );
                let _ = tx
                    .send(HostToExt::ApprovalResolved {
                        approval_id,
                        allowed: resolved && allow,
                    })
                    .await;
            }
            ExtToHost::UnregisterSession { session_id } => {
                if let Err(err) = app.end_session(&session_id) {
                    log.write("warn", &format!("unregister session failed: {err}"));
                } else {
                    let _ = session_count.saturating_sub(1);
                }
            }
            ExtToHost::Status => {
                let sessions = app.store.list_sessions(30).unwrap_or_default();
                let activity = app.store.recent_activity(40).unwrap_or_default();
                let audit_valid = app.store.verify_audit().map(|v| v.valid).unwrap_or(false);
                let _ = tx
                    .send(HostToExt::StatusReply {
                        bridge: "connected".into(),
                        host_version: env!("CARGO_PKG_VERSION").into(),
                        workspace_root: app.workspace.root.to_string_lossy().to_string(),
                        sessions,
                        recent_activity: activity,
                        approvals: app.approvals.pending_requests(),
                        audit_valid,
                        protocol: PROTOCOL_VERSION,
                        extension_build: connected_extension_version.clone().unwrap_or_default(),
                        active_id: app.workspace.id(),
                        workspaces: app.store.workspaces().unwrap_or_default(),
                        capabilities: serde_json::to_value(app.capabilities())?,
                        in_flight: in_flight.load(std::sync::atomic::Ordering::SeqCst)
                            + crate::jobs::active_count(&app).unwrap_or(1),
                        database_path: app.home.database.display().to_string(),
                        log_path: app
                            .home
                            .logs_dir
                            .join("native-host.log")
                            .display()
                            .to_string(),
                    })
                    .await;
            }
            ExtToHost::ListWorkspaces { request_key } => match app.store.workspaces() {
                Ok(workspaces) => {
                    let _ = tx
                        .send(HostToExt::WorkspaceList {
                            request_key,
                            active_id: app.workspace.id(),
                            workspaces,
                        })
                        .await;
                }
                Err(err) => {
                    send_error(&tx, "workspace_error", err.to_string(), Some(request_key)).await
                }
            },
            ExtToHost::CreateWorkspace {
                request_key,
                label,
                path,
                profile,
            } => match app.create_workspace(&label, &path, profile) {
                Ok(info) => {
                    let _ = tx
                        .send(HostToExt::ControlResult {
                            request_key,
                            result: serde_json::to_value(info)?,
                        })
                        .await;
                }
                Err(err) => {
                    send_error(&tx, "workspace_error", err.to_string(), Some(request_key)).await
                }
            },
            ExtToHost::SetActiveWorkspace {
                request_key,
                workspace_id,
                confirm_in_flight,
            } => {
                if (in_flight.load(std::sync::atomic::Ordering::SeqCst) > 0
                    || crate::jobs::active_count(&app).unwrap_or(1) > 0)
                    && !confirm_in_flight
                {
                    send_error(&tx,"confirm_in_flight","Tools are still running. Confirm the switch; those calls will finish in their original workspace.".into(),Some(request_key)).await;
                    continue;
                }
                match app.select_workspace(&workspace_id) {
                    Ok(next) => {
                        log.write(
                            "info",
                            &format!(
                                "workspace switched by user from {:?} to {:?}",
                                app.workspace.root, next.workspace.root
                            ),
                        );
                        app = next;
                        send_workspace_changed(&tx, &app, request_key).await;
                    }
                    Err(err) => {
                        send_error(&tx, "workspace_error", err.to_string(), Some(request_key)).await
                    }
                }
            }
            ExtToHost::ForgetWorkspace {
                request_key,
                workspace_id,
            } => {
                let result = app.store.forget_workspace(&workspace_id).and_then(|_| {
                    app.audit_workspace("workspace.forget", serde_json::json!({"id":workspace_id}))
                });
                send_control(
                    &tx,
                    request_key,
                    result.map(|_| serde_json::json!({"forgotten":workspace_id})),
                )
                .await;
            }
            ExtToHost::SetWorkspacePolicy {
                request_key,
                read_only,
                permission_mode,
            } => {
                let result = (|| -> Result<Arc<App>> {
                    let mut info = app
                        .store
                        .workspaces()?
                        .into_iter()
                        .find(|entry| entry.id == app.workspace.id())
                        .context("active workspace missing")?;
                    app.audit_workspace("workspace.policy",serde_json::json!({"id":info.id,"read_only":read_only,"permission_mode":permission_mode}))?;
                    info.read_only = read_only;
                    info.permission_mode = permission_mode;
                    app.store.save_workspace(&info)?;
                    let mut workspace = app.workspace.clone();
                    workspace.read_only = read_only;
                    workspace.permission_mode = permission_mode;
                    Ok(app.with_workspace(workspace))
                })();
                match result {
                    Ok(next) => {
                        app = next;
                        send_workspace_changed(&tx, &app, request_key).await;
                    }
                    Err(err) => {
                        send_error(&tx, "workspace_error", err.to_string(), Some(request_key)).await
                    }
                }
            }
            ExtToHost::RescanWorkspace { request_key } => {
                let result = crate::workspace::Workspace::initialize_profile(
                    &app.workspace.root,
                    app.workspace.profile,
                )
                .and_then(|_| {
                    app.audit_workspace(
                        "workspace.rescan",
                        serde_json::json!({"id":app.workspace.id()}),
                    )?;
                    Ok(serde_json::json!({"rescanned":app.workspace.root}))
                });
                send_control(&tx, request_key, result).await;
            }
            ExtToHost::OpenWorkspace { request_key } => {
                let result = std::process::Command::new("/usr/bin/open")
                    .arg(&app.workspace.root)
                    .status()
                    .context("open workspace in Finder")
                    .and_then(|status| {
                        anyhow::ensure!(status.success(), "Finder could not open workspace");
                        Ok(serde_json::json!({"opened":app.workspace.root}))
                    });
                send_control(&tx, request_key, result).await;
            }
            ExtToHost::PickWorkspaceFolder { request_key } => {
                let tx = tx.clone();
                tokio::spawn(async move {
                    let result = tokio::time::timeout(
                        std::time::Duration::from_secs(110),
                        crate::workspace::pick_folder(),
                    )
                    .await
                    .context("folder picker timed out")
                    .and_then(|result| result)
                    .map(|path| serde_json::json!({"path":path}));
                    send_control(&tx, request_key, result).await;
                });
            }
            ExtToHost::GrantCommand {
                request_key,
                command,
            } => match app.grant_command(command) {
                Ok(next) => {
                    app = next;
                    send_control(&tx, request_key, Ok(serde_json::json!({"granted":true}))).await;
                }
                Err(err) => {
                    send_error(
                        &tx,
                        "command_grant_error",
                        err.to_string(),
                        Some(request_key),
                    )
                    .await
                }
            },
            ExtToHost::RevokeCommand {
                request_key,
                command_id,
            } => match app.revoke_command(&command_id) {
                Ok(next) => {
                    app = next;
                    send_control(
                        &tx,
                        request_key,
                        Ok(serde_json::json!({"revoked":command_id})),
                    )
                    .await;
                }
                Err(err) => {
                    send_error(
                        &tx,
                        "command_grant_error",
                        err.to_string(),
                        Some(request_key),
                    )
                    .await
                }
            },
            ExtToHost::ListCommandGrants { request_key } => {
                send_control(&tx,request_key,Ok(serde_json::json!({"commands":app.config.workspace_commands.get(&app.workspace.id()).cloned().unwrap_or_default()}))).await;
            }
            ExtToHost::RecoveryControl { request_key, workspace_id, action } => {
                send_control(&tx, request_key, crate::tools::large_recovery::control(&app, &workspace_id, action)).await;
            }
            ExtToHost::JobControl {
                request_key,
                workspace_id,
                action,
            } => {
                log.write(
                    "info",
                    &format!(
                        "panel job control {:?} workspace={} request={}",
                        action, workspace_id, request_key
                    ),
                );
                send_control(
                    &tx,
                    request_key,
                    crate::jobs::control(&app, &workspace_id, action),
                )
                .await;
            }
            ExtToHost::Preflight { request_key } => {
                send_control(&tx, request_key, crate::tools::system::preflight(&app)).await;
            }
            ExtToHost::CreateWriteGrant {
                request_key,
                workspace_id,
                grant,
            } => {
                send_control(
                    &tx,
                    request_key,
                    crate::grants::create(&app, &workspace_id, grant),
                )
                .await;
            }
            ExtToHost::RevokeWriteGrant {
                request_key,
                workspace_id,
                session_id,
            } => {
                send_control(
                    &tx,
                    request_key,
                    crate::grants::revoke(&app, &workspace_id, &session_id),
                )
                .await;
            }
            ExtToHost::ListWriteGrants { request_key } => {
                send_control(
                    &tx,
                    request_key,
                    app.store
                        .write_grants(&app.workspace.id())
                        .map(|grants| serde_json::json!({"grants":grants})),
                )
                .await;
            }
            ExtToHost::ActivityDetail {
                request_key,
                activity_id,
            } => {
                send_control(&tx, request_key, app.store.activity_result(activity_id)).await;
            }
            ExtToHost::CommandPresets { request_key } => {
                send_control(
                    &tx,
                    request_key,
                    Ok(serde_json::json!({"presets":crate::tools::commands::presets(&app)})),
                )
                .await;
            }
            ExtToHost::IntegrationList { request_key } => {
                send_control(
                    &tx,
                    request_key,
                    Ok(crate::integrations::configured_list(&app)),
                )
                .await;
            }
            ExtToHost::IntegrationSave {
                request_key,
                server,
            } => match crate::integrations::save(&app, server) {
                Ok(next) => {
                    app = next;
                    send_control(&tx, request_key, Ok(serde_json::json!({"saved":true}))).await;
                }
                Err(err) => {
                    send_error(&tx, "integration_error", err.to_string(), Some(request_key)).await
                }
            },
            ExtToHost::IntegrationRemove {
                request_key,
                server_id,
            } => match crate::integrations::remove(&app, &server_id) {
                Ok(next) => {
                    app = next;
                    send_control(
                        &tx,
                        request_key,
                        Ok(serde_json::json!({"removed":server_id})),
                    )
                    .await;
                }
                Err(err) => {
                    send_error(&tx, "integration_error", err.to_string(), Some(request_key)).await
                }
            },
            ExtToHost::IntegrationDiscover {
                request_key,
                server_id,
            } => {
                let app = app.clone();
                let tx = tx.clone();
                tokio::spawn(async move {
                    let result = crate::integrations::discover(&app, &server_id)
                        .await
                        .map_err(anyhow::Error::from);
                    let _ = app.audit_workspace(
                        "integration.discover",
                        serde_json::json!({"server":server_id,"success":result.is_ok()}),
                    );
                    send_control(&tx, request_key, result).await;
                });
            }
            request @ (ExtToHost::RegisterArtifact { .. } | ExtToHost::RevokeArtifact { .. }) => {
                let request_key = match &request {
                    ExtToHost::RegisterArtifact { request_key, .. }
                    | ExtToHost::RevokeArtifact { request_key, .. } => request_key.clone(),
                    _ => unreachable!(),
                };
                if !request_key.starts_with("panel-") || request_key.len() > 100 {
                    send_error(
                        &tx,
                        "invalid_request_key",
                        "Artifact grants require extension controls.".into(),
                        Some(request_key),
                    )
                    .await;
                    continue;
                }
                let app = app.clone();
                let tx = tx.clone();
                let semaphore = semaphore.clone();
                let log_path = log.path.clone();
                tokio::spawn(async move {
                    let _permit = semaphore.acquire_owned().await;
                    let result = tokio::task::spawn_blocking(move || {
                        let (session_id, outcome) = match request {
                            ExtToHost::RegisterArtifact {
                                session_id,
                                workspace_id,
                                download,
                                ..
                            } => {
                                let outcome = tools::artifacts::register(
                                    &app,
                                    &session_id,
                                    &workspace_id,
                                    download,
                                );
                                (session_id, outcome)
                            }
                            ExtToHost::RevokeArtifact {
                                session_id,
                                workspace_id,
                                artifact_id,
                                ..
                            } => {
                                let outcome = tools::artifacts::revoke(
                                    &app,
                                    &session_id,
                                    &workspace_id,
                                    &artifact_id,
                                );
                                (session_id, outcome)
                            }
                            _ => unreachable!(),
                        };
                        if let Err(error) = &outcome {
                            if let Ok(Some((_, agent))) = app.session_agent(&session_id) {
                                let _ = app.audit_denial(
                                    agent,
                                    &session_id,
                                    "artifact.grant",
                                    None,
                                    &error.to_string(),
                                    "denied",
                                );
                            }
                        }
                        HostLog::new(log_path).write(
                            "info",
                            &format!(
                                "artifact control session={session_id} workspace={:?} result={}",
                                app.workspace.root,
                                if outcome.is_ok() { "ok" } else { "error" }
                            ),
                        );
                        outcome.map_err(anyhow::Error::from)
                    })
                    .await
                    .unwrap_or_else(|error| {
                        Err(anyhow::anyhow!("artifact control failed: {error}"))
                    });
                    send_control(&tx, request_key, result).await;
                });
            }
            ExtToHost::RunTool {
                request_key,
                workspace_id,
                session_id,
                tool,
                arguments,
            } => {
                let Some((session, agent)) = app.session_agent(&session_id).ok().flatten() else {
                    send_error(
                        &tx,
                        "unknown_session",
                        "Select a connected provider session.".into(),
                        Some(request_key),
                    )
                    .await;
                    continue;
                };
                if workspace_id != app.workspace.id() || session.workspace_id != workspace_id {
                    send_error(
                        &tx,
                        "workspace_changed",
                        "Workspace changed; refresh controls.".into(),
                        Some(request_key),
                    )
                    .await;
                    continue;
                }
                if !request_key.starts_with("panel-") || request_key.len() > 100 {
                    send_error(
                        &tx,
                        "invalid_request_key",
                        "Invalid panel request.".into(),
                        Some(request_key),
                    )
                    .await;
                    continue;
                }
                let scope = format!("panel:{workspace_id}:{session_id}");
                let claim = app.store.claim_response(ResponseRequest {
                    scope: &scope,
                    response: &request_key,
                    task: &request_key,
                    index: 0,
                    tool: &tool,
                    arguments: &arguments,
                    response_limit: 1,
                    task_limit: 1,
                    timeout_secs: app.config.tool_timeout_secs,
                });
                match claim {
                    Ok(RequestClaim::New) => {}
                    Ok(RequestClaim::Completed(result) | RequestClaim::Rejected(result)) => {
                        send_control(
                            &tx,
                            request_key,
                            serde_json::from_str(&result).map_err(Into::into),
                        )
                        .await;
                        continue;
                    }
                    other => {
                        send_error(
                            &tx,
                            "request_pending",
                            format!("Request already pending or could not be recorded: {other:?}"),
                            Some(request_key),
                        )
                        .await;
                        continue;
                    }
                }
                app.audit_workspace("user.run_tool",serde_json::json!({"request":request_key,"session":session_id,"tool":tool,"workspace":workspace_id}))?;
                let app = app.clone();
                let tx = tx.clone();
                let semaphore = semaphore.clone();
                let log_path = log.path.clone();
                in_flight.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let count = in_flight.clone();
                tokio::spawn(async move {
                    let _running = RunningRequest(count);
                    let started = Instant::now();
                    let path = arguments
                        .get("destination")
                        .or_else(|| arguments.get("path"))
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_owned);
                    let execution = async {
                        let _permit = semaphore.acquire_owned().await;
                        tools::dispatch(&app, agent, &session_id, &tool, arguments).await
                    };
                    let result = tokio::time::timeout(
                        std::time::Duration::from_secs(app.config.tool_timeout_secs),
                        execution,
                    )
                    .await
                    .unwrap_or_else(|_| {
                        let _ = app.audit_denial(
                            agent,
                            &session_id,
                            &tool,
                            path.as_deref(),
                            "Panel tool execution timed out",
                            "timeout",
                        );
                        ToolResultMessage::error(
                            &tool,
                            "request_timeout",
                            "Operation timed out; inspect activity before retrying.".into(),
                        )
                    });
                    let mut value = serde_json::to_value(&result).unwrap_or_default();
                    value["duration_ms"] = serde_json::json!(started.elapsed().as_millis() as u64);
                    HostLog::new(log_path.clone()).write("info",&format!("panel tool {tool} {} session={session_id} agent={} provider={} tab={} workspace={:?} path={path:?} request={request_key} duration_ms={} bytes={}",result.status,agent.dir(),session.provider,session.tab_id,app.workspace.root,started.elapsed().as_millis(),value.to_string().len()));
                    if let Err(err) =
                        app.store
                            .finish_response(&scope, &request_key, 0, &value.to_string())
                    {
                        HostLog::new(log_path).write(
                            "error",
                            &format!(
                                "panel result persistence failed request={request_key}: {err}"
                            ),
                        );
                    }
                    send_control(&tx, request_key, Ok(value)).await;
                });
            }
            ExtToHost::Ping { nonce } => {
                let _ = tx.send(HostToExt::Pong { nonce }).await;
            }
        }
        let _ = session_count;
    }

    log.write("info", "stdin closed; native host stopping");
    drop(tx);
    let _ = tokio::time::timeout(std::time::Duration::from_secs(2), writer).await;
    event_forwarder.abort();
    Ok(())
}

struct RunningRequest(Arc<std::sync::atomic::AtomicUsize>);
impl Drop for RunningRequest {
    fn drop(&mut self) {
        self.0.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }
}

async fn send_control(
    tx: &mpsc::Sender<HostToExt>,
    request_key: String,
    result: Result<serde_json::Value>,
) {
    match result {
        Ok(result) => {
            let _ = tx
                .send(HostToExt::ControlResult {
                    request_key,
                    result,
                })
                .await;
        }
        Err(err) => send_error(tx, "workspace_error", err.to_string(), Some(request_key)).await,
    }
}

async fn send_workspace_changed(tx: &mpsc::Sender<HostToExt>, app: &App, request_key: String) {
    let _ = tx
        .send(HostToExt::WorkspaceChanged {
            request_key,
            workspace_id: app.workspace.id(),
            root: app.workspace.root.display().to_string(),
            profile: app.workspace.profile,
            migrated: false,
            workspaces: app.store.workspaces().unwrap_or_default(),
            capabilities: serde_json::to_value(app.capabilities()).unwrap_or_default(),
            sessions: app.store.list_sessions(100).unwrap_or_default(),
        })
        .await;
}

fn is_ready(hello: &std::sync::atomic::AtomicBool) -> bool {
    hello.load(std::sync::atomic::Ordering::SeqCst)
}

async fn send_error(
    tx: &mpsc::Sender<HostToExt>,
    code: &str,
    message: String,
    request_key: Option<String>,
) {
    let _ = tx
        .send(HostToExt::Error {
            code: code.into(),
            message,
            request_key,
        })
        .await;
}

async fn writer_loop(
    _tx: mpsc::Sender<HostToExt>,
    mut rx: mpsc::Receiver<HostToExt>,
) -> Result<()> {
    let mut stdout = tokio::io::stdout();
    while let Some(message) = rx.recv().await {
        write_message(&mut stdout, &message).await?;
    }
    Ok(())
}

async fn read_frame<R: tokio::io::AsyncRead + Unpin>(reader: &mut R) -> Result<Option<Vec<u8>>> {
    let mut length = [0u8; 4];
    match reader.read_exact(&mut length).await {
        Ok(_) => {}
        Err(err) if err.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(err) => return Err(err).context("read native message length"),
    }
    let length = u32::from_le_bytes(length) as usize;
    if length == 0 {
        anyhow::bail!("native message length is zero");
    }
    if length > MAX_MESSAGE_BYTES {
        anyhow::bail!("native message length {length} exceeds {MAX_MESSAGE_BYTES}");
    }
    let mut body = vec![0u8; length];
    reader
        .read_exact(&mut body)
        .await
        .context("read native message body")?;
    Ok(Some(body))
}

async fn write_message<W: tokio::io::AsyncWrite + Unpin>(
    writer: &mut W,
    message: &HostToExt,
) -> Result<()> {
    let body = serde_json::to_vec(message).context("serialize host message")?;
    if body.len() > MAX_MESSAGE_BYTES {
        let fallback = serde_json::to_vec(&HostToExt::Error {
            code: "message_too_large".into(),
            message: format!(
                "host response is {} bytes; limit is {MAX_MESSAGE_BYTES}",
                body.len()
            ),
            request_key: None,
        })?;
        writer
            .write_all(&(fallback.len() as u32).to_le_bytes())
            .await?;
        writer.write_all(&fallback).await?;
        writer.flush().await?;
        return Ok(());
    }
    writer.write_all(&(body.len() as u32).to_le_bytes()).await?;
    writer.write_all(&body).await?;
    writer.flush().await?;
    Ok(())
}

/// Helper for diagnostics: read a framed message from a byte slice.
pub fn decode_frame(bytes: &[u8]) -> Result<(usize, HostToExt)> {
    if bytes.len() < 4 {
        anyhow::bail!("frame too short");
    }
    let length = u32::from_le_bytes(bytes[..4].try_into()?) as usize;
    if bytes.len() < 4 + length {
        anyhow::bail!("frame body truncated");
    }
    let message = serde_json::from_slice(&bytes[4..4 + length])?;
    Ok((4 + length, message))
}
