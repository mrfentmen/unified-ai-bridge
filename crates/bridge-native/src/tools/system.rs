use super::{parse_args, ToolContext, ToolOutcome};
use crate::failure::ToolFailure;
use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Empty {}

pub fn information(app: &crate::app::App) -> Value {
    #[cfg(unix)]
    let free_bytes = {
        use std::os::unix::ffi::OsStrExt;
        let path = std::ffi::CString::new(app.workspace.root.as_os_str().as_bytes()).ok();
        path.and_then(|path| {
            let mut stat = std::mem::MaybeUninit::<libc::statvfs>::uninit();
            // SAFETY: path and the output pointer are valid for statvfs.
            if unsafe { libc::statvfs(path.as_ptr(), stat.as_mut_ptr()) } == 0 {
                let stat = unsafe { stat.assume_init() };
                Some((stat.f_bavail as u64).saturating_mul(stat.f_frsize as u64))
            } else {
                None
            }
        })
    };
    #[cfg(not(unix))]
    let free_bytes: Option<u64> = None;
    let os_version = if cfg!(target_os = "macos") {
        std::process::Command::new("/usr/bin/sw_vers")
            .arg("-productVersion")
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
    } else {
        None
    };
    json!({"os":std::env::consts::OS,"os_version":os_version,"architecture":std::env::consts::ARCH,
        "host_version":env!("CARGO_PKG_VERSION"),"protocol":bridge_core::PROTOCOL_VERSION,"workspace_free_bytes":free_bytes,
        "os_permissions":{"files_and_folders":"unknown until exercised in this launch context","accessibility":"unknown until exercised","screen_recording":"unknown until exercised"}})
}

pub async fn system_info(ctx: &ToolContext<'_>, args: Value) -> Result<ToolOutcome, ToolFailure> {
    let _: Empty = parse_args(args)?;
    Ok(ToolOutcome::read(
        "Host system information",
        "OS permission state is not inferred from configuration.",
        None,
    )
    .with_data(information(ctx.app)))
}

pub async fn workspace_info(
    ctx: &ToolContext<'_>,
    args: Value,
) -> Result<ToolOutcome, ToolFailure> {
    let _: Empty = parse_args(args)?;
    let root = ctx.resolve_existing(".")?;
    ctx.check_read("workspace_info", &root)?;
    let policy = ctx
        .app
        .store
        .workspaces()
        .map_err(|e| ToolFailure::Internal(e.to_string()))?
        .into_iter()
        .find(|w| w.id == ctx.app.workspace.id())
        .ok_or_else(|| ToolFailure::PermissionDenied("workspace is no longer granted".into()))?;
    let commands:Vec<_>=ctx.app.config.workspace_commands.get(&policy.id).into_iter().flatten().map(|c|json!({"command_id":c.command_id,"timeout_secs":c.timeout_secs,"max_output_bytes":c.max_output_bytes,"writes":c.writes,"always_allow":c.always_allow})).collect();
    Ok(ToolOutcome::read("Current workspace capabilities", "Use only advertised tools and user-granted command IDs.",Some(".".into())).with_data(json!({
        "root":policy.root,"profile":policy.profile,"read_only":policy.read_only,"permission_mode":policy.permission_mode,
        "tools":ctx.app.workspace.profile.tools().map(|t|t.name).collect::<Vec<_>>(),"commands":commands,
        "network":super::network::policy(ctx.app),
        "limits":{"text_bytes":super::MAX_READ_BYTES,"artifact_bytes":super::artifacts::MAX_BYTES,"approval_seconds":ctx.app.config.approval_timeout_secs,"tool_seconds":ctx.app.config.tool_timeout_secs,"calls_per_response":ctx.app.config.max_calls_per_response,"calls_per_task":ctx.app.config.max_calls_per_task}})))
}

pub fn preflight(app: &crate::app::App) -> anyhow::Result<Value> {
    let policy = app
        .store
        .workspaces()?
        .into_iter()
        .find(|w| w.id == app.workspace.id())
        .ok_or_else(|| anyhow::anyhow!("active workspace is no longer granted"))?;
    let commands=app.config.workspace_commands.get(&policy.id).into_iter().flatten().map(|g|json!({
        "command_id":g.command_id,"valid":g.validate().is_ok(),"program_exists":g.program.is_file(),
        "cwd_exists":app.workspace.root.join(&g.cwd).is_dir(),"timeout_secs":g.timeout_secs,"always_allow":g.always_allow})).collect::<Vec<_>>();
    Ok(
        json!({"system":information(app),"workspace":policy,"commands":commands,"network":super::network::policy(app),
        "checks":{"workspace_exists":app.workspace.root.is_dir(),"workspace_unlocked":!policy.read_only,
            "workspace_readable":std::fs::read_dir(&app.workspace.root).is_ok(),"audit_valid":app.store.verify_audit()?.valid},
        "limits":{"tool_seconds":app.config.tool_timeout_secs,"calls_per_response":app.config.max_calls_per_response,"calls_per_task":app.config.max_calls_per_task},
        "browser_readiness":"The panel checks the loaded extension and registered sessions separately. Registration does not establish composer readiness or account availability."}),
    )
}
