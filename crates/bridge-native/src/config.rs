use anyhow::{bail, Context, Result};
use base64::Engine;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};

use bridge_core::NATIVE_HOST_NAME;

pub const CONFIG_VERSION: u32 = 1;
pub const APP_DIR_NAME: &str = "unified-ai-bridge";
pub const HOST_EXECUTABLE_NAME: &str = "unified-ai-bridge-native";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub format_version: u32,
    /// User-selected archaeology project root. The bridge creates its fixed
    /// workspace structure here and never assumes a location.
    pub project_root: PathBuf,
    pub native_host_name: String,
    pub extension_id: Option<String>,
    pub extension_dir: Option<PathBuf>,
    pub approval_timeout_secs: u64,
    pub max_calls_per_response: u32,
    pub max_calls_per_task: u32,
    pub tool_timeout_secs: u64,
    pub desktop_tools_enabled: bool,
    /// Scripts the user has explicitly approved by name. No arbitrary shell.
    pub approved_scripts: Vec<ApprovedScript>,
    /// Explicit command grants keyed by canonical workspace ID. Models cannot
    /// add grants or supply executable paths.
    pub workspace_commands: std::collections::BTreeMap<String, Vec<AllowedCommand>>,
    /// User-configured native MCP executables. No package installers or interpreters.
    pub workspace_integrations:
        std::collections::BTreeMap<String, Vec<crate::integrations::ServerConfig>>,
    /// User-granted public origins. Disabled unless explicitly configured.
    pub workspace_network: std::collections::BTreeMap<String, crate::tools::network::Policy>,
    pub private_file_patterns: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            format_version: CONFIG_VERSION,
            project_root: PathBuf::new(),
            native_host_name: NATIVE_HOST_NAME.to_string(),
            extension_id: None,
            extension_dir: None,
            approval_timeout_secs: 180,
            max_calls_per_response: 4,
            max_calls_per_task: 16,
            tool_timeout_secs: 240,
            desktop_tools_enabled: true,
            approved_scripts: Vec::new(),
            workspace_commands: Default::default(),
            workspace_integrations: Default::default(),
            workspace_network: Default::default(),
            private_file_patterns: [
                ".env",
                ".env.*",
                ".ssh",
                ".aws",
                "*.pem",
                "*.key",
                "credentials.json",
            ]
            .into_iter()
            .map(str::to_owned)
            .collect(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AllowedCommand {
    pub command_id: String,
    pub program: PathBuf,
    #[serde(default)]
    pub fixed_args: Vec<String>,
    /// Finite, explicitly approved argument suffixes, never arbitrary strings.
    #[serde(default)]
    pub allowed_args: Vec<Vec<String>>,
    pub cwd: PathBuf,
    pub timeout_secs: u64,
    pub max_output_bytes: usize,
    pub writes: bool,
    #[serde(default)]
    pub always_allow: bool,
}

impl AllowedCommand {
    pub fn validate(&self) -> Result<()> {
        if self.command_id.is_empty()
            || self.command_id.len() > 80
            || !self
                .command_id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b))
        {
            bail!("command_id must contain 1–80 letters, digits, hyphens or underscores");
        }
        if !self.program.is_absolute() {
            bail!("command program must be an absolute path");
        }
        let program = self
            .program
            .canonicalize()
            .context("resolve approved command program")?;
        let name = program
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_ascii_lowercase();
        if [
            "sh",
            "bash",
            "zsh",
            "dash",
            "fish",
            "csh",
            "tcsh",
            "ksh",
            "env",
            "osascript",
            "node",
            "nodejs",
            "ruby",
            "perl",
            "php",
        ]
        .contains(&name.as_str())
            || name.starts_with("python")
        {
            bail!("shells and general-purpose script interpreters cannot be command grants");
        }
        if !program.is_file() {
            bail!("command program is not a file");
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if fs::metadata(&program)?.permissions().mode() & 0o111 == 0 {
                bail!("command program is not executable");
            }
        }
        if self.cwd.is_absolute()
            || self
                .cwd
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            bail!("command cwd must be workspace-relative without traversal");
        }
        if !(1..=3600).contains(&self.timeout_secs)
            || !(128..=50000).contains(&self.max_output_bytes)
        {
            bail!("command timeout must be 1–3600 seconds; output budget 128–50000 bytes");
        }
        if self.fixed_args.len() > 64
            || self.allowed_args.len() > 64
            || self.allowed_args.iter().any(|args| args.len() > 32)
            || self
                .fixed_args
                .iter()
                .chain(self.allowed_args.iter().flatten())
                .any(|arg| arg.len() > 4096 || arg.contains('\0'))
        {
            bail!("command argument template is too large or contains NUL");
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApprovedScript {
    pub name: String,
    /// Absolute program path or a PATH-resolved program name. The bridge never
    /// invokes a shell and never appends model-supplied arguments.
    pub program: PathBuf,
    pub args: Vec<String>,
    /// Workspace-relative working directory.
    pub cwd: Option<PathBuf>,
    pub timeout_secs: u64,
}

#[derive(Debug, Clone)]
pub struct HomePaths {
    pub base: PathBuf,
    pub config_file: PathBuf,
    pub database: PathBuf,
    pub logs_dir: PathBuf,
    pub default_project: PathBuf,
    pub native_hosts_dir: PathBuf,
    pub install_dir: PathBuf,
}

impl HomePaths {
    pub fn discover() -> Result<Self> {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .context("HOME is not set to an absolute path")?;
        let base = std::env::var_os("UAB_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                home.join("Library")
                    .join("Application Support")
                    .join(APP_DIR_NAME)
            });
        let native_hosts_dir = std::env::var_os("UAB_NATIVE_HOSTS_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                home.join("Library")
                    .join("Application Support")
                    .join("Google")
                    .join("Chrome")
                    .join("NativeMessagingHosts")
            });
        Ok(Self {
            config_file: base.join("config.toml"),
            database: base.join("bridge.sqlite3"),
            logs_dir: base.join("logs"),
            default_project: base.join("Archaeology"),
            native_hosts_dir,
            install_dir: base.join("bin"),
            base,
        })
    }

    pub fn ensure(&self) -> Result<()> {
        fs::create_dir_all(&self.base)
            .with_context(|| format!("create application directory {}", self.base.display()))?;
        fs::create_dir_all(&self.logs_dir)
            .with_context(|| format!("create log directory {}", self.logs_dir.display()))?;
        fs::create_dir_all(&self.install_dir)
            .with_context(|| format!("create install directory {}", self.install_dir.display()))?;
        Ok(())
    }

    pub fn load_config(&self) -> Result<Config> {
        let raw = fs::read_to_string(&self.config_file).with_context(|| {
            format!(
                "read {}; run `unified-ai-bridge-native setup --project <path>` first",
                self.config_file.display()
            )
        })?;
        let config: Config = toml::from_str(&raw)
            .with_context(|| format!("parse configuration {}", self.config_file.display()))?;
        validate_config(&config)?;
        Ok(config)
    }

    pub fn save_config(&self, config: &Config) -> Result<()> {
        validate_config(config)?;
        self.ensure()?;
        let rendered = toml::to_string_pretty(config).context("serialize configuration")?;
        atomic_write(&self.config_file, rendered.as_bytes())?;
        Ok(())
    }

    pub fn native_manifest_path(&self) -> PathBuf {
        self.native_hosts_dir
            .join(format!("{NATIVE_HOST_NAME}.json"))
    }

    pub fn installed_binary(&self) -> PathBuf {
        self.install_dir.join(HOST_EXECUTABLE_NAME)
    }
}

pub fn validate_config(config: &Config) -> Result<()> {
    for (workspace, policy) in &config.workspace_network {
        if workspace.len()!=24||!workspace.bytes().all(|b|b.is_ascii_hexdigit()){bail!("invalid network workspace ID");}
        policy.validate()?;
    }
    for (workspace, servers) in &config.workspace_integrations {
        if workspace.len() != 24 || !workspace.bytes().all(|b| b.is_ascii_hexdigit()) {
            bail!("invalid integration workspace ID");
        }
        crate::integrations::validate_catalog(servers)?;
    }
    if config.private_file_patterns.len() > 100 {
        bail!("at most 100 private-file patterns are supported");
    }
    for pattern in &config.private_file_patterns {
        if pattern.len() > 512 {
            bail!("private-file pattern too long");
        }
        globset::GlobBuilder::new(pattern)
            .case_insensitive(true)
            .literal_separator(true)
            .build()
            .context("invalid private-file pattern")?;
    }
    if config.format_version != CONFIG_VERSION {
        bail!(
            "unsupported configuration version {} (expected {})",
            config.format_version,
            CONFIG_VERSION
        );
    }
    if config.project_root.as_os_str().is_empty() || !config.project_root.is_absolute() {
        bail!("project_root must be an absolute path");
    }
    if config.native_host_name != NATIVE_HOST_NAME {
        bail!(
            "native_host_name must be {NATIVE_HOST_NAME}; another name requires rebuilding Chrome's host manifest"
        );
    }
    if !(10..=600).contains(&config.approval_timeout_secs) {
        bail!("approval_timeout_secs must be between 10 and 600");
    }
    if !(1..=32).contains(&config.max_calls_per_response)
        || !(1..=256).contains(&config.max_calls_per_task)
    {
        bail!("call budgets must be 1..=32 per response and 1..=256 per task");
    }
    if !(1..=3600).contains(&config.tool_timeout_secs) {
        bail!("tool_timeout_secs must be between 1 and 3600");
    }
    if let Some(id) = &config.extension_id {
        validate_extension_id(id)?;
    }
    for script in &config.approved_scripts {
        if script.name.trim().is_empty() {
            bail!("approved script has an empty name");
        }
        if script.name.len() > 80 {
            bail!("approved script name is too long: {}", script.name);
        }
        if script.timeout_secs == 0 || script.timeout_secs > 3600 {
            bail!("approved script {} has an invalid timeout", script.name);
        }
        if let Some(cwd) = &script.cwd {
            if cwd.is_absolute()
                || cwd
                    .components()
                    .any(|c| matches!(c, std::path::Component::ParentDir))
            {
                bail!(
                    "approved script {} cwd must be a relative path without ..",
                    script.name
                );
            }
        }
    }
    for (workspace, commands) in &config.workspace_commands {
        if workspace.len() != 24 || !workspace.bytes().all(|b| b.is_ascii_hexdigit()) {
            bail!("invalid command workspace ID");
        }
        let mut ids = std::collections::HashSet::new();
        for command in commands {
            command.validate()?;
            if !ids.insert(&command.command_id) {
                bail!("duplicate command {}", command.command_id);
            }
        }
    }
    Ok(())
}

pub fn is_extension_id(value: &str) -> bool {
    value.len() == 32 && value.chars().all(|ch| ('a'..='p').contains(&ch))
}

pub fn validate_extension_id(value: &str) -> Result<()> {
    if is_extension_id(value) {
        Ok(())
    } else {
        bail!(
            "invalid Chrome extension id `{value}` (expected 32 characters in a–p; \
             build with `bridge-tooling build-extension` to create a deterministic id)"
        )
    }
}

/// The unpacked extension has a deterministic id if its manifest carries the
/// public `key`. Prefer the id file written by our packaging tool, and derive
/// it from the manifest key as a second source of truth.
pub fn extension_id_from_dir(extension_dir: &Path) -> Result<String> {
    let id_file = extension_dir.join("extension-id.txt");
    if let Ok(raw) = fs::read_to_string(&id_file) {
        let id = raw.trim().to_string();
        validate_extension_id(&id)?;
        return Ok(id);
    }
    let manifest_path = extension_dir.join("manifest.json");
    let raw = fs::read_to_string(&manifest_path)
        .with_context(|| format!("read {}", manifest_path.display()))?;
    let manifest: serde_json::Value =
        serde_json::from_str(&raw).with_context(|| format!("parse {}", manifest_path.display()))?;
    let key = manifest
        .get("key")
        .and_then(|value| value.as_str())
        .context(
        "extension manifest has no key; run bridge-tooling build-extension or pass --extension-id",
    )?;
    extension_id_from_public_key(key)
}

pub fn extension_id_from_public_key(encoded: &str) -> Result<String> {
    let der = base64::engine::general_purpose::STANDARD
        .decode(encoded.trim())
        .context("extension public key is not valid base64")?;
    let digest = Sha256::digest(&der);
    let mut id = String::with_capacity(32);
    for byte in digest.iter().take(16) {
        id.push(char::from(b'a' + (byte >> 4)));
        id.push(char::from(b'a' + (byte & 0x0f)));
    }
    validate_extension_id(&id)?;
    Ok(id)
}

#[derive(Serialize)]
struct NativeHostManifest<'a> {
    name: &'a str,
    description: &'a str,
    path: String,
    #[serde(rename = "type")]
    host_type: &'a str,
    allowed_origins: Vec<String>,
}

pub fn write_native_host_manifest(
    paths: &HomePaths,
    extension_id: &str,
    host_path: &Path,
) -> Result<PathBuf> {
    validate_extension_id(extension_id)?;
    if !host_path.is_absolute() {
        bail!("native host path must be absolute");
    }
    fs::create_dir_all(&paths.native_hosts_dir).with_context(|| {
        format!(
            "create Chrome NativeMessagingHosts directory {}",
            paths.native_hosts_dir.display()
        )
    })?;
    let manifest = NativeHostManifest {
        name: NATIVE_HOST_NAME,
        description: "Unified AI Bridge local Rust backend",
        path: host_path.to_string_lossy().to_string(),
        host_type: "stdio",
        allowed_origins: vec![format!("chrome-extension://{extension_id}/")],
    };
    let target = paths.native_manifest_path();
    let raw = serde_json::to_vec_pretty(&manifest).context("serialize native host manifest")?;
    atomic_write(&target, &raw)?;
    Ok(target)
}

/// Persist the workspace root without creating it if it does not exist.
pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("create parent directory {}", parent.display()))?;
    }
    let tmp = path.with_file_name(format!(
        ".{}.tmp-{}",
        path.file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("config"),
        std::process::id()
    ));
    {
        use std::io::Write;
        let mut file = fs::File::create(&tmp)
            .with_context(|| format!("create temporary file {}", tmp.display()))?;
        file.write_all(bytes)?;
        file.sync_all()?;
    }
    fs::rename(&tmp, path)
        .with_context(|| format!("rename {} to {}", tmp.display(), path.display()))?;
    if let Some(parent) = path.parent() {
        let _ = fs::File::open(parent).and_then(|f| f.sync_all());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extension_id_derivation_is_deterministic() {
        let key = fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packaging/extension-public-key.txt"),
        )
        .unwrap();
        let id = extension_id_from_public_key(&key).unwrap();
        assert_eq!(id, extension_id_from_public_key(&key).unwrap());
        assert!(is_extension_id(&id));
    }

    #[test]
    fn rejects_traversal_in_script_cwd() {
        let mut config = Config::default();
        config.project_root = PathBuf::from("/tmp/project");
        config.approved_scripts.push(ApprovedScript {
            name: "x".into(),
            program: PathBuf::from("/bin/true"),
            args: vec![],
            cwd: Some(PathBuf::from("../elsewhere")),
            timeout_secs: 10,
        });
        assert!(validate_config(&config).is_err());
    }
}
