use anyhow::{bail, Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

use crate::config::{
    self, extension_id_from_dir, is_extension_id, write_native_host_manifest, HomePaths,
};
use crate::workspace::Workspace;

pub struct SetupOptions {
    pub project: Option<PathBuf>,
    pub extension_dir: Option<PathBuf>,
    pub extension_id: Option<String>,
    pub install_host: bool,
}

pub fn run_setup(paths: HomePaths, options: SetupOptions) -> Result<()> {
    paths.ensure()?;
    let project = options
        .project
        .unwrap_or_else(|| paths.default_project.clone());
    let (workspace, report) = Workspace::initialize(&project)?;
    let mut config = if paths.config_file.exists() {
        paths.load_config()?
    } else {
        config::Config::default()
    };
    config.project_root = workspace.root.clone();

    let extension_id = match (options.extension_id, options.extension_dir.as_deref()) {
        (Some(id), _) => {
            config::validate_extension_id(&id)?;
            Some(id)
        }
        (None, Some(dir)) => Some(extension_id_from_dir(dir)?),
        (None, None) => config.extension_id.clone(),
    };
    config.extension_id = extension_id.clone();
    paths.save_config(&config)?;

    println!("Unified AI Bridge configured.");
    println!("  workspace: {}", workspace.root.display());
    println!("  database:  {}", paths.database.display());
    println!("  config:    {}", paths.config_file.display());
    if report.root_was_nonempty {
        println!(
            "  note: the selected directory already had files; {} existing file(s) were left untouched",
            report.existing_files_skipped
        );
    }
    if report.created_files > 0 {
        println!(
            "  created {} initial workspace file(s); they are templates, not research evidence",
            report.created_files
        );
    }

    if options.install_host {
        let Some(id) = extension_id else {
            println!("\nNative host not registered yet: build the extension, then run");
            println!("  unified-ai-bridge-native install --extension-dir <dist/extension>");
            return Ok(());
        };
        install_native_host(&paths, &config, &id)?;
    }
    Ok(())
}

pub fn install_from_extension_dir(paths: HomePaths, extension_dir: &Path) -> Result<()> {
    let config = paths.load_config().with_context(|| {
        "configuration missing; run `unified-ai-bridge-native setup --project <path>` first"
    })?;
    let extension_id = extension_id_from_dir(extension_dir)?;
    let mut config = config;
    config.extension_id = Some(extension_id.clone());
    config.extension_dir = Some(extension_dir.canonicalize()?);
    paths.save_config(&config)?;
    install_native_host(&paths, &config, &extension_id)
}

pub fn install_with_id(paths: HomePaths, extension_id: &str) -> Result<()> {
    let mut config = paths.load_config()?;
    config::validate_extension_id(extension_id)?;
    config.extension_id = Some(extension_id.to_string());
    paths.save_config(&config)?;
    install_native_host(&paths, &config, extension_id)
}

fn install_native_host(
    paths: &HomePaths,
    _config: &config::Config,
    extension_id: &str,
) -> Result<()> {
    if !is_extension_id(extension_id) {
        bail!("invalid extension id");
    }
    paths.ensure()?;
    let current = std::env::current_exe().context("locate the running native binary")?;
    let installed = paths.installed_binary();
    if current != installed {
        if let Some(parent) = installed.parent() {
            fs::create_dir_all(parent)?;
        }
        let temp = installed.with_extension("installing");
        fs::copy(&current, &temp)
            .with_context(|| format!("copy {} to {}", current.display(), temp.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&temp, fs::Permissions::from_mode(0o755))?;
        }
        fs::rename(&temp, &installed)
            .with_context(|| format!("install native host at {}", installed.display()))?;
    }
    let manifest = write_native_host_manifest(paths, extension_id, &installed)?;
    println!("Native host registered.");
    println!("  executable: {}", installed.display());
    println!("  manifest:   {}", manifest.display());
    println!("  extension:  {extension_id}");
    println!(
        "\nReload the unpacked extension in chrome://extensions, then reload the AI website tabs."
    );
    println!("Chrome must be fully restarted once if it was already running when the host manifest was added.");
    Ok(())
}

pub fn uninstall(paths: &HomePaths) -> Result<()> {
    let manifest = paths.native_manifest_path();
    if manifest.exists() {
        fs::remove_file(&manifest)
            .with_context(|| format!("remove native host manifest {}", manifest.display()))?;
        println!("Removed {}", manifest.display());
    } else {
        println!("No native host manifest at {}", manifest.display());
    }
    println!("Workspace, configuration, audit log, and database were not removed.");
    Ok(())
}

pub fn print_status(paths: &HomePaths) -> Result<()> {
    let config = paths.load_config()?;
    let workspace = Workspace::load(&config.project_root)?;
    let audit = if paths.base.join("audit.jsonl").exists() {
        paths.base.join("audit.jsonl")
    } else {
        workspace.audit_file.clone()
    };
    let store = crate::store::Store::open_readonly(&paths.database, &audit)?;
    println!("Unified AI Bridge status");
    println!("  workspace: {}", workspace.root.display());
    println!("  native host: {}", env!("CARGO_PKG_VERSION"));
    println!(
        "  extension id: {}",
        config.extension_id.as_deref().unwrap_or("not registered")
    );
    println!(
        "  native manifest: {}",
        paths.native_manifest_path().display()
    );
    println!(
        "  native manifest present: {}",
        paths.native_manifest_path().is_file()
    );
    println!("  installed binary: {}", paths.installed_binary().display());
    let sessions = store.list_sessions(20)?;
    println!("  sessions: {}", sessions.len());
    for session in sessions {
        println!(
            "    {} {} tab={} conversation={}",
            session.agent,
            session.provider,
            session.tab_id,
            session.conversation.as_deref().unwrap_or("(new chat)")
        );
    }
    let audit = store.verify_audit()?;
    println!(
        "  audit: {} ({} entries, file {})",
        if audit.valid { "valid" } else { "INVALID" },
        audit.database_entries,
        audit.file_entries
    );
    let recent = store.recent_activity(10)?;
    if !recent.is_empty() {
        println!("  recent activity:");
        for activity in recent {
            println!(
                "    {} {} {} — {}",
                activity.agent, activity.tool, activity.decision, activity.summary
            );
        }
    }
    Ok(())
}
