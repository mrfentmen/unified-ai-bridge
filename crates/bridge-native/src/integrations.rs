//! Opt-in local MCP clients. Third-party executables retain the user's OS rights.
use crate::{
    app::App,
    failure::ToolFailure,
    tools::{parse_args, ToolContext, ToolOutcome},
};
use anyhow::{bail, Context, Result};
use rmcp::{
    model::CallToolRequestParams, service::RunningService, transport::TokioChildProcess,
    RoleClient, ServiceExt,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{io::Read, path::PathBuf, process::Stdio, time::Duration};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolGrant {
    pub name: String,
    pub schema_sha256: String,
    pub schema: Value,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerConfig {
    pub id: String,
    pub program: PathBuf,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub executable_sha256: String,
    #[serde(default)]
    pub tools: Vec<ToolGrant>,
    #[serde(default = "deadline")]
    pub timeout_secs: u64,
}
fn deadline() -> u64 {
    60
}
impl ServerConfig {
    pub fn validate(&self) -> Result<()> {
        if self.id.is_empty()
            || self.id.len() > 64
            || !self
                .id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b))
        {
            bail!("server ID must be 1–64 letters, digits, hyphens or underscores");
        }
        if !self.program.is_absolute() || !self.program.is_file() {
            bail!("server must be an installed absolute executable path");
        }
        let mut magic = [0; 4];
        std::fs::File::open(&self.program)
            .context("open MCP executable")?
            .read_exact(&mut magic)
            .context("read MCP executable header")?;
        // Native binaries only: no shell, package manager, or script wrapper.
        if !matches!(
            magic,
            [0xcf, 0xfa, 0xed, 0xfe]
                | [0xce, 0xfa, 0xed, 0xfe]
                | [0xfe, 0xed, 0xfa, 0xcf]
                | [0xca, 0xfe, 0xba, 0xbe]
                | [0xbe, 0xba, 0xfe, 0xca]
                | [0x7f, b'E', b'L', b'F']
        ) {
            bail!("MCP server must be a native binary; script runtimes are not enabled");
        }
        let name = self
            .program
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_ascii_lowercase();
        if [
            "sh",
            "bash",
            "zsh",
            "env",
            "node",
            "nodejs",
            "ruby",
            "perl",
            "php",
            "osascript",
            "swift",
            "java",
            "deno",
            "bun",
        ]
        .contains(&name.as_str())
            || name.starts_with("python")
        {
            bail!("interpreter MCP servers are not enabled");
        }
        if crate::fsops::sha256_file(&self.program)?.0 != self.executable_sha256 {
            bail!("MCP executable changed; review and register its new hash");
        }
        if !(1..=120).contains(&self.timeout_secs)
            || self.args.len() > 32
            || self.args.iter().any(|a| a.len() > 4096 || a.contains('\0'))
            || self.tools.len() > 128
        {
            bail!("MCP configuration exceeds limits");
        }
        let mut names = std::collections::HashSet::new();
        if self.tools.iter().any(|t| {
            t.name.is_empty()
                || t.name.len() > 128
                || !t.schema.is_object()
                || t.schema.to_string().len() > 32_000
                || crate::fsops::sha256_bytes(t.schema.to_string().as_bytes()) != t.schema_sha256
                || !names.insert(&t.name)
        }) {
            bail!("invalid or duplicate tool grant");
        }
        Ok(())
    }
}
fn configured<'a>(app: &'a App, id: &str) -> Result<&'a ServerConfig, ToolFailure> {
    app.config
        .workspace_integrations
        .get(&app.workspace.id())
        .and_then(|s| s.iter().find(|s| s.id == id))
        .ok_or_else(|| {
            ToolFailure::PermissionDenied("integration is not registered in this workspace".into())
        })
}
fn failure(error: impl std::fmt::Display) -> ToolFailure {
    ToolFailure::Io(error.to_string())
}
struct ProcessGroup(Option<u32>);
impl ProcessGroup {
    fn stop(&mut self) {
        if let Some(pid) = self.0.take() {
            unsafe {
                libc::kill(-(pid as i32), libc::SIGKILL);
            }
        }
    }
}
impl Drop for ProcessGroup {
    fn drop(&mut self) {
        self.stop();
    }
}
struct McpClient {
    // Dropped first, including initialization/discovery/validation error paths.
    group: ProcessGroup,
    service: RunningService<RoleClient, ()>,
}
impl McpClient {
    async fn stop(mut self) {
        self.group.stop();
        let _ = self
            .service
            .close_with_timeout(Duration::from_secs(2))
            .await;
    }
}
/// Live transports are private to one workspace/session/server. A lease owns
/// the process while a call is running: dropping a cancelled future kills it.
#[derive(Default)]
pub struct ConnectionPool {
    entries: std::sync::Mutex<std::collections::HashMap<String, std::sync::Arc<ConnectionEntry>>>,
}
struct ConnectionEntry {
    slot: std::sync::Arc<tokio::sync::Mutex<Option<(McpClient, tokio::time::Instant)>>>,
    online: std::sync::atomic::AtomicBool,
    workspace: String,
    session: String,
    cancelled: std::sync::atomic::AtomicBool,
    cancellation: tokio::sync::Notify,
    watch_generation: std::sync::atomic::AtomicU64,
    config: ServerConfig,
}
impl ConnectionEntry {
    fn cancel(&self) {
        self.cancelled
            .store(true, std::sync::atomic::Ordering::SeqCst);
        self.online
            .store(false, std::sync::atomic::Ordering::Relaxed);
        self.cancellation.notify_waiters();
        if let Ok(mut slot) = self.slot.try_lock() {
            slot.take();
        }
    }
    fn check(&self) -> Result<(), ToolFailure> {
        if self.cancelled.load(std::sync::atomic::Ordering::SeqCst) {
            Err(failure(
                "Application connection disconnected. No retry was attempted.",
            ))
        } else {
            Ok(())
        }
    }
    async fn cancelled(&self) -> ToolFailure {
        loop {
            let notified = self.cancellation.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if let Err(error) = self.check() {
                return error;
            }
            notified.await;
        }
    }
}
impl ConnectionPool {
    fn entry(
        &self,
        app: &App,
        session: &str,
        config: &ServerConfig,
    ) -> Result<std::sync::Arc<ConnectionEntry>, ToolFailure> {
        let key =
            bridge_core::browser_files::source_key(&[&app.workspace.id(), session, &config.id]);
        let mut entries = self.entries.lock().expect("MCP pool");
        if let Some(entry) = entries.get(&key) {
            if serde_json::to_string(&entry.config).ok() == serde_json::to_string(config).ok() {
                return Ok(entry.clone());
            }
            entry.cancel();
            entries.remove(&key);
        }
        entries.retain(|_, e| {
            e.online.load(std::sync::atomic::Ordering::Relaxed)
                || std::sync::Arc::strong_count(e) > 1
        });
        if entries.len() >= 32 {
            return Err(failure(
                "Too many live application connections; disconnect an unused session.",
            ));
        }
        let entry = std::sync::Arc::new(ConnectionEntry {
            slot: std::sync::Arc::new(tokio::sync::Mutex::new(None)),
            online: std::sync::atomic::AtomicBool::new(false),
            workspace: app.workspace.id(),
            session: session.into(),
            cancelled: std::sync::atomic::AtomicBool::new(false),
            cancellation: tokio::sync::Notify::new(),
            watch_generation: std::sync::atomic::AtomicU64::new(0),
            config: config.clone(),
        });
        entries.insert(key, entry.clone());
        Ok(entry)
    }
    pub fn disconnect(&self, workspace: &str, server: Option<&str>) {
        self.entries.lock().expect("MCP pool").retain(|_, e| {
            let remove = e.workspace == workspace && server.is_none_or(|id| e.config.id == id);
            if remove {
                e.cancel();
            }
            !remove
        });
    }
    pub fn disconnect_session(&self, session: &str) {
        self.entries.lock().expect("MCP pool").retain(|_, e| {
            if e.session == session {
                e.cancel();
                false
            } else {
                true
            }
        });
    }
    fn connected(&self, workspace: &str, server: &str) -> usize {
        self.entries
            .lock()
            .expect("MCP pool")
            .values()
            .filter(|e| {
                e.workspace == workspace
                    && e.config.id == server
                    && e.online.load(std::sync::atomic::Ordering::Relaxed)
            })
            .count()
    }
}
struct ConnectionLease {
    client: Option<McpClient>,
    entry: std::sync::Arc<ConnectionEntry>,
    slot: tokio::sync::OwnedMutexGuard<Option<(McpClient, tokio::time::Instant)>>,
}
impl Drop for ConnectionLease {
    fn drop(&mut self) {
        if self.client.is_some() {
            self.entry
                .online
                .store(false, std::sync::atomic::Ordering::Relaxed);
        }
    }
}
impl ConnectionLease {
    async fn acquire(
        app: &App,
        entry: std::sync::Arc<ConnectionEntry>,
    ) -> Result<Self, ToolFailure> {
        let config = &entry.config;
        entry.check()?;
        config
            .validate()
            .map_err(|e| ToolFailure::PermissionDenied(e.to_string()))?;
        current_grant(app, config)?;
        let mut slot =
            entry.slot.clone().try_lock_owned().map_err(|_| {
                failure("This conversation already has an active call on that server.")
            })?;
        let client = if let Some((client, last_used)) = slot.take() {
            if client.service.is_closed() || last_used.elapsed() > Duration::from_secs(300) {
                entry
                    .online
                    .store(false, std::sync::atomic::Ordering::Relaxed);
                drop(client);
                return Err(failure("Application connection closed or expired. No tool was sent. A new explicit call can reconnect."));
            }
            client
        } else {
            tokio::select! {
                biased;
                error=entry.cancelled()=>return Err(error),
                error=crate::tasks::wait_until_halted(app)=>return Err(error),
                client=connect(app,config)=>client?,
            }
        };
        entry.check()?;
        entry
            .online
            .store(true, std::sync::atomic::Ordering::Relaxed);
        Ok(Self {
            client: Some(client),
            entry,
            slot,
        })
    }
    fn service(&self) -> &RunningService<RoleClient, ()> {
        &self.client.as_ref().expect("MCP lease").service
    }
    fn release(mut self, app: &App) {
        if self.entry.check().is_err() {
            return;
        }
        *self.slot = Some((
            self.client.take().expect("MCP lease"),
            tokio::time::Instant::now(),
        ));
        // Idle transports are polled independently of website focus. The watcher
        // owns only a weak entry, so it cannot keep an abandoned pool alive.
        let generation = self
            .entry
            .watch_generation
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            + 1;
        let weak = std::sync::Arc::downgrade(&self.entry);
        let home = app.home.clone();
        let store = app.store.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_millis(250)).await;
                let Some(entry) = weak.upgrade() else { break };
                if entry
                    .watch_generation
                    .load(std::sync::atomic::Ordering::Relaxed)
                    != generation
                {
                    break;
                }
                let Ok(mut slot) = entry.slot.try_lock() else {
                    break;
                };
                let Some((client, last_used)) = slot.as_ref() else {
                    break;
                };
                let current = home.load_config().ok().and_then(|c| {
                    c.workspace_integrations
                        .get(&entry.workspace)
                        .and_then(|s| s.iter().find(|s| s.id == entry.config.id))
                        .cloned()
                });
                let permitted = current.as_ref().and_then(|c| serde_json::to_string(c).ok())
                    == serde_json::to_string(&entry.config).ok()
                    && store
                        .workspaces()
                        .is_ok_and(|w| w.iter().any(|w| w.id == entry.workspace && !w.read_only))
                    && store
                        .active_workspace_id()
                        .is_ok_and(|id| id.as_deref() == Some(&entry.workspace));
                if !permitted
                    || client.service.is_closed()
                    || last_used.elapsed() > Duration::from_secs(300)
                    || !entry.online.load(std::sync::atomic::Ordering::Relaxed)
                {
                    slot.take();
                    entry
                        .online
                        .store(false, std::sync::atomic::Ordering::Relaxed);
                    break;
                }
            }
        });
    }
}
fn current_grant(app: &App, config: &ServerConfig) -> Result<(), ToolFailure> {
    let current = app.home.load_config().map_err(failure)?;
    let current = current
        .workspace_integrations
        .get(&app.workspace.id())
        .and_then(|items| items.iter().find(|s| s.id == config.id));
    if current.map(|s| serde_json::to_string(s).unwrap_or_default())
        != Some(serde_json::to_string(config).map_err(failure)?)
    {
        return Err(ToolFailure::PermissionDenied(
            "integration grant changed or was removed".into(),
        ));
    }
    if app
        .store
        .workspaces()
        .map_err(failure)?
        .iter()
        .find(|w| w.id == app.workspace.id())
        .is_none_or(|w| w.read_only)
    {
        return Err(ToolFailure::PermissionDenied(
            "workspace is locked or no longer granted".into(),
        ));
    }
    Ok(())
}
async fn revoked(app: &App, config: &ServerConfig) -> ToolFailure {
    loop {
        if let Err(error) = current_grant(app, config) {
            return error;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}
async fn connect(app: &App, config: &ServerConfig) -> Result<McpClient, ToolFailure> {
    current_grant(app, config)?;
    config
        .validate()
        .map_err(|e| ToolFailure::PermissionDenied(e.to_string()))?;
    let mut command = tokio::process::Command::new(&config.program);
    command
        .args(&config.args)
        .current_dir(&app.workspace.root)
        .env_clear()
        .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .process_group(0);
    if let Some(home) = std::env::var_os("HOME") {
        command.env("HOME", home);
    }
    let transport = TokioChildProcess::new(command).map_err(failure)?;
    let group = ProcessGroup(transport.id());
    let service = tokio::select! {result=tokio::time::timeout(Duration::from_secs(10), ().serve(transport))=>result
    .map_err(|_| failure("MCP initialization timed out"))?
    .map_err(failure)?,error=revoked(app,config)=>return Err(error)};
    Ok(McpClient { group, service })
}
async fn definitions(
    client: &RunningService<RoleClient, ()>,
) -> Result<Vec<rmcp::model::Tool>, ToolFailure> {
    let reply = tokio::time::timeout(Duration::from_secs(10), client.list_tools(None))
        .await
        .map_err(|_| failure("MCP discovery timed out"))?
        .map_err(failure)?;
    if reply.next_cursor.is_some()
        || reply.tools.len() > 128
        || serde_json::to_vec(&reply.tools).map_err(failure)?.len() > 200_000
    {
        return Err(ToolFailure::Unsupported("server catalog exceeds 128 tools/200 KB or requires pagination; select a narrower server toolset".into()));
    }
    Ok(reply.tools)
}
pub async fn discover(app: &App, id: &str) -> Result<Value, ToolFailure> {
    let config = configured(app, id)?;
    let client = connect(app, config).await?;
    let definitions = tokio::select! {result=definitions(&client.service)=>result?,error=revoked(app,config)=>return Err(error)};
    let tools:Vec<Value>=definitions.into_iter().map(|tool|{let schema=serde_json::to_value(&tool.input_schema).unwrap_or_default();json!({"name":tool.name,"description":tool.description,"schema":schema,"schema_sha256":crate::fsops::sha256_bytes(schema.to_string().as_bytes())})}).collect();
    client.stop().await;
    Ok(
        json!({"server_id":id,"state":"discovered","tools":tools,"authority":"Runs with your OS account permissions; bridge folder restrictions do not sandbox the server."}),
    )
}
pub fn health(app: &App, id: &str) -> Result<Value, ToolFailure> {
    let config = configured(app, id)?;
    let result = current_grant(app, config).and_then(|_| {
        config
            .validate()
            .map_err(|e| ToolFailure::PermissionDenied(e.to_string()))
    });
    Ok(
        json!({"server_id":id,"state":if result.is_err(){"blocked"}else if app.integration_connections.connected(&app.workspace.id(),id)>0{"connected"}else{"configured_unprobed"},"reason":result.err().map(|e|e.to_string()),"granted_tools":config.tools.len(),"timeout_secs":config.timeout_secs,"process_started":false,"connected_sessions":app.integration_connections.connected(&app.workspace.id(),id),"note":"This checks the current grant and executable hash, not server responsiveness. Discovery starts a temporary process. Approved calls reuse a conversation-owned connection for up to five idle minutes; discovery never grants tools. In-flight revocation/lock checks run every 250 ms and cannot undo completed external effects."}),
    )
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Health {
    server_id: String,
}
pub async fn status(ctx: &ToolContext<'_>, args: Value) -> Result<ToolOutcome, ToolFailure> {
    let args: Health = parse_args(args)?;
    let data = health(ctx.app, &args.server_id)?;
    Ok(ToolOutcome::read(
        "Integration readiness (no process started)",
        data.to_string(),
        None,
    )
    .with_data(data))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ListArgs {
    server_id: Option<String>,
    tool: Option<String>,
    #[serde(default)]
    offset: usize,
}
pub async fn list(ctx: &ToolContext<'_>, args: Value) -> Result<ToolOutcome, ToolFailure> {
    let args: ListArgs = parse_args(args)?;
    let servers = ctx
        .app
        .config
        .workspace_integrations
        .get(&ctx.app.workspace.id())
        .map(Vec::as_slice)
        .unwrap_or_default();
    let data = if let Some(tool) = args.tool {
        let id = args.server_id.as_deref().ok_or_else(|| {
            ToolFailure::InvalidArguments(
                "server_id is required when requesting a tool schema".into(),
            )
        })?;
        let server = configured(ctx.app, id)?;
        let grant = server
            .tools
            .iter()
            .find(|grant| grant.name == tool)
            .ok_or_else(|| {
                ToolFailure::PermissionDenied(
                    "tool is not explicitly enabled for this server".into(),
                )
            })?;
        json!({"server_id":id,"tool":grant})
    } else {
        if let Some(id) = &args.server_id {
            configured(ctx.app, id)?;
        }
        let entries: Vec<Value> = servers
            .iter()
            .filter(|s| args.server_id.as_ref().is_none_or(|id| id == &s.id))
            .flat_map(|s| {
                s.tools
                    .iter()
                    .map(|t| json!({"server_id":s.id,"tool":t.name}))
            })
            .collect();
        let next = args.offset.saturating_add(32);
        json!({"servers":servers.iter().map(|s|json!({"id":s.id,"granted_tools":s.tools.len(),"timeout_secs":s.timeout_secs})).collect::<Vec<_>>(),
            "tools":entries.iter().skip(args.offset).take(32).collect::<Vec<_>>(),"total_tools":entries.len(),"next_offset":(next<entries.len()).then_some(next),
            "schema_hint":"Call list_integrations with server_id and tool to read one complete granted schema."})
    };
    Ok(ToolOutcome::read(
        "configured integrations",
        "Granted integrations are in data; no process was started.",
        None,
    )
    .with_data(data))
}

/// Keep the panel's complete configuration response below Native Messaging's
/// frame limit. Model-facing catalogs are separately paged above.
pub fn validate_catalog(servers: &[ServerConfig]) -> Result<()> {
    if servers.len() > 16 || serde_json::to_vec(servers)?.len() > 200_000 {
        bail!("workspace integrations exceed 16 servers or 200 KB of configuration");
    }
    let mut ids = std::collections::HashSet::new();
    if servers.iter().any(|s| !ids.insert(&s.id)) {
        bail!("duplicate integration server ID");
    }
    Ok(())
}
pub fn configured_list(app: &App) -> Value {
    json!({"servers":app.config.workspace_integrations.get(&app.workspace.id()).cloned().unwrap_or_default()})
}
pub fn disconnect(app: &App, workspace: &str, id: &str) -> Result<Value> {
    if workspace != app.workspace.id() {
        bail!("workspace changed; refresh application controls");
    }
    configured(app, id)?;
    app.audit_workspace(
        "integration.disconnect",
        json!({"workspace":workspace,"server":id}),
    )?;
    app.integration_connections.disconnect(workspace, Some(id));
    Ok(
        json!({"disconnected":id,"note":"Connections stopped. Grants are preserved; a new explicit approved call can reconnect. Completed external effects are not undone."}),
    )
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Call {
    server_id: String,
    tool: String,
    arguments: Value,
}
pub async fn call(ctx: &ToolContext<'_>, args: Value) -> Result<ToolOutcome, ToolFailure> {
    let args: Call = parse_args(args)?;
    let config = configured(ctx.app, &args.server_id)?;
    let grant = config
        .tools
        .iter()
        .find(|g| g.name == args.tool)
        .ok_or_else(|| {
            ToolFailure::PermissionDenied("tool is not explicitly enabled for this server".into())
        })?;
    let arguments = args.arguments.as_object().cloned().ok_or_else(|| {
        ToolFailure::InvalidArguments("integration arguments must be an object".into())
    })?;
    if serde_json::to_vec(&args.arguments).map_err(failure)?.len() > 32000 {
        return Err(ToolFailure::InvalidArguments(
            "integration arguments exceed 32,000 bytes".into(),
        ));
    }
    // All external calls ask; upstream readOnly annotations do not confer authority.
    let entry = ctx
        .app
        .integration_connections
        .entry(ctx.app, ctx.session_id, config)?;
    let reason = format!("Run external MCP tool {}/{} with {}. This native program has your OS account permissions and is not confined to the workspace.", args.server_id, args.tool, args.arguments);
    tokio::select! {
        biased;
        error=entry.cancelled()=>return Err(error),
        result=ctx.app.request_approval(ctx.session_id,ctx.agent,"call_integration_tool",&reason,None)=>result?,
    }
    current_grant(ctx.app, config)?;
    let client = ConnectionLease::acquire(ctx.app, entry.clone()).await?;
    let definitions = tokio::select! {
        biased;
        error=entry.cancelled()=>return Err(error),
        error=crate::tasks::wait_until_halted(ctx.app)=>return Err(error),
        error=revoked(ctx.app,config)=>return Err(error),
        result=definitions(client.service())=>result?,
    };
    let definition = definitions
        .iter()
        .find(|t| t.name.as_ref() == args.tool)
        .ok_or_else(|| ToolFailure::NotFound("server no longer advertises this tool".into()))?;
    let schema = serde_json::to_value(&definition.input_schema).map_err(failure)?;
    if crate::fsops::sha256_bytes(schema.to_string().as_bytes()) != grant.schema_sha256 {
        return Err(ToolFailure::PermissionDenied(
            "tool schema changed; rediscover and review its grant".into(),
        ));
    }
    let validator = jsonschema::validator_for(&schema)
        .map_err(|e| ToolFailure::Unsupported(format!("unsupported server schema: {e}")))?;
    validator
        .validate(&args.arguments)
        .map_err(|e| ToolFailure::InvalidArguments(e.to_string()))?;
    current_grant(ctx.app, config)?;
    let request = CallToolRequestParams::new(args.tool.clone()).with_arguments(arguments);
    entry.check()?;
    let result = tokio::select! {
        biased;
        error=entry.cancelled()=>Err(failure(format!("{error} External process stopped; outcome is uncertain and was not retried."))),
        error=crate::tasks::wait_until_halted(ctx.app)=>Err(failure(format!("{error}; external process stopped; outcome is uncertain and was not retried."))),
        error=revoked(ctx.app,config)=>Err(ToolFailure::PermissionDenied(format!("{error}; external process stopped. The operation may already have changed files; outcome is uncertain and was not retried."))),
        result=tokio::time::timeout(Duration::from_secs(config.timeout_secs),client.service().call_tool(request))=>result.map_err(|_|failure("MCP call timed out; outcome may be uncertain. It was not retried.")).and_then(|r|r.map_err(failure)),
    };
    let result = result?;
    current_grant(ctx.app, config)?;
    let serialized = serde_json::to_string(&result).map_err(failure)?;
    if serialized.len() > 50_000 {
        return Err(ToolFailure::Unsupported("MCP result exceeds 50 KB; operation may have completed. Narrow its output before making a new request.".into()));
    }
    client.release(ctx.app);
    let mut outcome = ToolOutcome::write(
        format!("MCP {}/{}", args.server_id, args.tool),
        "External result is in data.mcp_result.",
        None,
    )
    .with_data(json!({"server_id":args.server_id,"tool":args.tool,"mcp_result":result}));
    if result.is_error == Some(true) {
        outcome.failure = Some((
            "integration_tool_error",
            "External MCP tool returned an error".into(),
        ));
    }
    ctx.app
        .audit_path_access(
            ctx.agent,
            ctx.session_id,
            "tool.call_integration_tool",
            None,
            if outcome.failure.is_some() {
                "error"
            } else {
                "allowed"
            },
            &json!({"server":args.server_id,"tool":args.tool,"bytes":serialized.len()}).to_string(),
        )
        .map_err(failure)?;
    Ok(outcome)
}

pub fn save(app: &std::sync::Arc<App>, mut server: ServerConfig) -> Result<std::sync::Arc<App>> {
    if server.executable_sha256.is_empty() {
        server.executable_sha256 = crate::fsops::sha256_file(&server.program)?.0;
    }
    server.validate()?;
    let mut config = app.config.clone();
    let entries = config
        .workspace_integrations
        .entry(app.workspace.id())
        .or_default();
    entries.retain(|entry| entry.id != server.id);
    entries.push(server.clone());
    validate_catalog(entries)?;
    app.audit_workspace(
        "integration.configure",
        json!({"workspace":app.workspace.id(),"server":server}),
    )?;
    app.home.save_config(&config)?;
    app.integration_connections
        .disconnect(&app.workspace.id(), None);
    Ok(std::sync::Arc::new(App {
        home: app.home.clone(),
        config,
        workspace: app.workspace.clone(),
        store: app.store.clone(),
        approvals: app.approvals.clone(),
        browser_files: app.browser_files.clone(),
        integration_connections: app.integration_connections.clone(),
        events: app.events.clone(),
        interactive: app.interactive,
    }))
}
pub fn remove(app: &std::sync::Arc<App>, id: &str) -> Result<std::sync::Arc<App>> {
    let mut config = app.config.clone();
    let entries = config
        .workspace_integrations
        .entry(app.workspace.id())
        .or_default();
    entries.retain(|entry| entry.id != id);
    app.audit_workspace(
        "integration.remove",
        json!({"workspace":app.workspace.id(),"server":id}),
    )?;
    app.home.save_config(&config)?;
    app.integration_connections
        .disconnect(&app.workspace.id(), None);
    Ok(std::sync::Arc::new(App {
        home: app.home.clone(),
        config,
        workspace: app.workspace.clone(),
        store: app.store.clone(),
        approvals: app.approvals.clone(),
        browser_files: app.browser_files.clone(),
        integration_connections: app.integration_connections.clone(),
        events: app.events.clone(),
        interactive: app.interactive,
    }))
}
