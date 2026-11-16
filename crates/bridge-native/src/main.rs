use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::Arc;

use bridge_core::provider::Agent;
use bridge_native::app::App;
use bridge_native::config::HomePaths;
use bridge_native::{diagnostics, mcp, native_host, selftest, setup};

#[derive(Parser, Debug)]
#[command(
    name = "unified-ai-bridge-native",
    version,
    about = "Unified AI Bridge native backend"
)]
struct Cli {
    /// Override application state directory (useful for isolated installations).
    #[arg(long, global = true)]
    home: Option<PathBuf>,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand, Debug)]
enum Command {
    #[command(hide = true)]
    RustSymbolWorker,
    #[command(hide = true)]
    DocxWorker,
    #[command(hide = true)]
    PdfWorker,
    #[command(hide = true)]
    OcrWorker,
    #[command(hide = true)]
    XlsxWorker,
    #[command(hide = true)]
    JobWorker { job_id: String },
    /// Run the Chrome native messaging host (default when launched by Chrome).
    Native,
    /// Initialize the workspace and save configuration.
    Setup {
        /// Archaeology project directory. Created if missing; existing files are never overwritten.
        #[arg(long)]
        project: Option<PathBuf>,
        /// Built unpacked extension directory containing manifest.json and extension-id.txt.
        #[arg(long)]
        extension_dir: Option<PathBuf>,
        /// Explicit Chrome extension id (32 chars a-p).
        #[arg(long)]
        extension_id: Option<String>,
        /// Also install the native host manifest and binary.
        #[arg(long)]
        install_host: bool,
    },
    /// Install/update the native host registration from a built extension directory.
    Install {
        #[arg(long)]
        extension_dir: Option<PathBuf>,
        #[arg(long)]
        extension_id: Option<String>,
    },
    /// Remove the Chrome native host manifest (leaves data untouched).
    Uninstall,
    /// Show configuration, sessions, and audit status.
    Status,
    /// Check the installation, architecture, Chrome, and permissions.
    Diagnose {
        #[arg(long)]
        json: bool,
    },
    /// Read-only installation checks, database integrity, and audit verification.
    Doctor {
        #[arg(long)]
        json: bool,
    },
    /// Run the RMCP stdio server with a fixed local identity.
    Mcp {
        #[arg(long, default_value = "chatgpt")]
        agent: String,
        #[arg(long)]
        project: Option<PathBuf>,
    },
    /// Verify real backend behavior in a temporary workspace.
    Selftest,
    /// Open the macOS folder picker and print its validated workspace path.
    PickWorkspace,
}

/// Chrome launches a native messaging host with the calling extension's
/// origin as the first argument (`chrome-extension://<id>/`). It is not a
/// user-supplied command, so strip it before clap parses the CLI. The host
/// manifest's `allowed_origins` remains the authorization decision.
fn host_arguments() -> Vec<OsString> {
    let mut args: Vec<OsString> = std::env::args_os().collect();
    if args
        .get(1)
        .is_some_and(|arg| arg.to_string_lossy().starts_with("chrome-extension://"))
    {
        args.remove(1);
    }
    args
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse_from(host_arguments());
    if let Some(home) = cli.home {
        anyhow::ensure!(home.is_absolute(), "--home must be absolute");
        std::env::set_var("UAB_HOME", home);
    }
    match cli.command.unwrap_or(Command::Native) {
        Command::JobWorker { job_id } => {
            bridge_native::jobs::worker(HomePaths::discover()?, &job_id).await
        }
        Command::RustSymbolWorker => bridge_native::tools::code_navigation::rust_symbol_worker(),
        Command::DocxWorker => bridge_native::tools::documents::worker(),
        Command::OcrWorker => bridge_native::tools::ocr::worker(),
        Command::PdfWorker => bridge_native::tools::pdf::worker(),
        Command::XlsxWorker => bridge_native::tools::workbooks::worker(),
        Command::Native => native_host::run().await,
        Command::Setup {
            project,
            extension_dir,
            extension_id,
            install_host,
        } => setup::run_setup(
            HomePaths::discover()?,
            setup::SetupOptions {
                project,
                extension_dir,
                extension_id,
                install_host,
            },
        ),
        Command::Install {
            extension_dir,
            extension_id,
        } => {
            let paths = HomePaths::discover()?;
            if let Some(dir) = extension_dir {
                setup::install_from_extension_dir(paths, &dir)
            } else if let Some(id) = extension_id {
                setup::install_with_id(paths, &id)
            } else {
                anyhow::bail!("install requires --extension-dir or --extension-id")
            }
        }
        Command::Uninstall => setup::uninstall(&HomePaths::discover()?),
        Command::Status => setup::print_status(&HomePaths::discover()?),
        Command::Diagnose { json } | Command::Doctor { json } => {
            diagnostics::run(&HomePaths::discover()?, json)
        }
        Command::Mcp { agent, project } => {
            let paths = HomePaths::discover()?;
            let config = paths.load_config()?;
            let project = project.unwrap_or_else(|| config.project_root.clone());
            let workspace = bridge_native::workspace::Workspace::load(&project)?;
            let mut config = config;
            config.project_root = workspace.root.clone();
            let app: Arc<App> = App::open_with_config(paths, config, workspace, false)?;
            let agent = agent
                .parse::<Agent>()
                .map_err(|err| anyhow::anyhow!(err.to_string()))?;
            mcp::run(app, agent).await
        }
        Command::Selftest => selftest::run().await,
        Command::PickWorkspace => {
            let selected = bridge_native::workspace::pick_folder().await?;
            println!("{}", selected.display());
            Ok(())
        }
    }
    .context("Unified AI Bridge command failed")
}
