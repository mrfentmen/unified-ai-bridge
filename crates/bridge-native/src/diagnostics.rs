use anyhow::Result;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::Path;
use std::process::Command;

use crate::config::{extension_id_from_dir, HomePaths};
use crate::store::Store;
use crate::workspace::Workspace;

#[derive(Debug, Serialize)]
pub struct Check {
    pub name: String,
    pub status: String,
    pub detail: String,
}

pub fn run(paths: &HomePaths, json: bool) -> Result<()> {
    let checks = collect(paths);
    if json {
        println!("{}", serde_json::to_string_pretty(&checks)?);
    } else {
        println!(
            "Unified AI Bridge doctor — host {} / protocol {}",
            env!("CARGO_PKG_VERSION"),
            bridge_core::PROTOCOL_VERSION
        );
        for check in &checks {
            let marker = match check.status.as_str() {
                "ok" => "PASS",
                "fail" => "FAIL",
                _ => "NOTE",
            };
            println!("[{marker}] {}: {}", check.name, check.detail);
        }
        println!("Browser build and live provider execution must be verified in Chrome; doctor does not infer them from files.");
    }
    anyhow::ensure!(
        !checks.iter().any(|check| check.status == "fail"),
        "doctor found failing checks"
    );
    Ok(())
}

pub fn collect(paths: &HomePaths) -> Vec<Check> {
    let mut checks = Vec::new();
    checks.push(check_platform());
    checks.push(Check {
        name: "architecture".into(),
        status: if cfg!(target_arch = "x86_64") { "ok".into() } else { "note".into() },
        detail: format!(
            "{} build; the Intel target is x86_64-apple-darwin. This check does not establish compatibility with a physical Intel Mac or its macOS version.",
            std::env::consts::ARCH
        ),
    });
    checks.push(check_config(paths));
    let config = paths.load_config().ok();
    if let Some(config) = &config {
        match Workspace::load(&config.project_root) {
            Ok(workspace) => {
                checks.push(Check {
                    name: "workspace".into(),
                    status: "ok".into(),
                    detail: workspace.root.to_string_lossy().to_string(),
                });
                let audit = if paths.base.join("audit.jsonl").exists() {
                    paths.base.join("audit.jsonl")
                } else {
                    workspace.audit_file.clone()
                };
                match Store::open_readonly(&paths.database, &audit) {
                    Ok(store) => {
                        match store.verify_audit() {
                            Ok(verification) => checks.push(Check {
                                name: "audit chain".into(),
                                status: if verification.valid {
                                    "ok".into()
                                } else {
                                    "fail".into()
                                },
                                detail: format!(
                                    "{} database entries, {} file entries: {}",
                                    verification.database_entries,
                                    verification.file_entries,
                                    verification.message
                                ),
                            }),
                            Err(err) => checks.push(fail("audit chain", err.to_string())),
                        }
                        let integrity = store.integrity_check();
                        checks.push(match integrity {
                            Ok(value) if value == "ok" => Check {
                                name: "SQLite integrity".into(),
                                status: "ok".into(),
                                detail: format!("{}: {value}", paths.database.display()),
                            },
                            other => fail(
                                "SQLite integrity",
                                format!("{}: {other:?}", paths.database.display()),
                            ),
                        });
                    }
                    Err(err) => checks.push(fail("SQLite", err.to_string())),
                }
            }
            Err(err) => checks.push(fail("workspace", err.to_string())),
        }
    } else {
        checks.push(fail("configuration", "run setup first".into()));
    }
    checks.push(check_native_manifest(paths));
    checks.push(check_installed_binary(paths));
    checks.push(check_extension(paths));
    checks.push(check_chrome());
    checks.push(check_macos_permissions());
    checks.push(Check {
        name: "network requirement".into(),
        status: "ok".into(),
        detail: "No provider API, API key, or billing gateway is used by the bridge.".into(),
    });

    checks.push(Check {
        name: "host log".into(),
        status: "ok".into(),
        detail: paths.logs_dir.join("native-host.log").display().to_string(),
    });
    checks
}

fn check_platform() -> Check {
    let version = Command::new("/usr/bin/sw_vers")
        .arg("-productVersion")
        .output()
        .ok()
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|value| value.trim().to_string())
        .unwrap_or_else(|| "unknown".into());
    Check {
        name: "macOS".into(),
        status: if cfg!(target_os = "macos") {
            "ok".into()
        } else {
            "warn".into()
        },
        detail: format!("{version} ({})", std::env::consts::ARCH),
    }
}

fn check_config(paths: &HomePaths) -> Check {
    match paths.load_config() {
        Ok(config) => Check {
            name: "configuration".into(),
            status: "ok".into(),
            detail: format!(
                "{}; host={}; extension={}",
                paths.config_file.display(),
                config.native_host_name,
                config.extension_id.as_deref().unwrap_or("unset")
            ),
        },
        Err(err) => fail(
            "configuration",
            format!("{}: {err}", paths.config_file.display()),
        ),
    }
}

fn check_native_manifest(paths: &HomePaths) -> Check {
    let manifest = paths.native_manifest_path();
    if !manifest.is_file() {
        return fail(
            "Chrome native host",
            format!("missing {}", manifest.display()),
        );
    }
    match fs::read_to_string(&manifest) {
        Ok(raw) => {
            match serde_json::from_str::<serde_json::Value>(&raw) {
                Ok(value) => {
                    let path = value.get("path").and_then(|v| v.as_str()).unwrap_or("");
                    let id = paths
                        .load_config()
                        .ok()
                        .and_then(|config| config.extension_id);
                    let expected = id
                        .as_ref()
                        .map(|id| serde_json::json!([format!("chrome-extension://{id}/")]));
                    if Path::new(path) == paths.installed_binary()
                        && Path::new(path).is_file()
                        && value["name"] == bridge_core::NATIVE_HOST_NAME
                        && value["type"] == "stdio"
                        && id.is_some()
                        && expected.as_ref() == value.get("allowed_origins")
                    {
                        Check {
                            name: "Chrome native host".into(),
                            status: "ok".into(),
                            detail: format!("{}: {}", manifest.display(), value),
                        }
                    } else {
                        fail(
                        "Chrome native host",
                        format!("{}: name, type, binary path, or exact configured origin mismatch: {}", manifest.display(), value),
                    )
                    }
                }
                Err(err) => fail("Chrome native host", format!("invalid JSON: {err}")),
            }
        }
        Err(err) => fail("Chrome native host", err.to_string()),
    }
}

fn check_extension(paths: &HomePaths) -> Check {
    let mut candidates = vec![
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../bridge-extension/dist/chromium"),
        Path::new("dist/extension").to_path_buf(),
        paths.base.join("extension"),
    ];
    if let Some(dir) = paths
        .load_config()
        .ok()
        .and_then(|config| config.extension_dir)
    {
        candidates.insert(0, dir);
    }
    for candidate in candidates {
        if candidate.join("manifest.json").is_file() {
            match extension_id_from_dir(&candidate) {
                Ok(id) => {
                    let config_id = paths
                        .load_config()
                        .ok()
                        .and_then(|config| config.extension_id);
                    let matches = config_id.as_deref() == Some(id.as_str());
                    let version = fs::read(candidate.join("manifest.json"))
                        .ok()
                        .and_then(|raw| serde_json::from_slice::<serde_json::Value>(&raw).ok())
                        .and_then(|value| value["version"].as_str().map(str::to_owned))
                        .unwrap_or_default();
                    return Check {
                        name: "extension build".into(),
                        status: if matches && !version.is_empty() {
                            "ok".into()
                        } else {
                            "fail".into()
                        },
                        detail: format!(
                            "{} (version {version}, id {id}{})",
                            candidate.display(),
                            if matches {
                                ""
                            } else {
                                ", not the id in configuration"
                            }
                        ),
                    };
                }
                Err(err) => return fail("extension build", err.to_string()),
            }
        }
    }
    Check {
        name: "extension build".into(),
        status: "warn".into(),
        detail: "packaged extension not found; run cargo bridge-deploy".into(),
    }
}

fn check_installed_binary(paths: &HomePaths) -> Check {
    let binary = paths.installed_binary();
    let result = (|| -> Result<String> {
        let output = Command::new(&binary).arg("--version").output()?;
        anyhow::ensure!(output.status.success(), "--version failed");
        let version = String::from_utf8(output.stdout)?;
        anyhow::ensure!(
            version.trim() == format!("unified-ai-bridge-native {}", env!("CARGO_PKG_VERSION")),
            "version mismatch: {}",
            version.trim()
        );
        let sha256 = hex::encode(Sha256::digest(fs::read(&binary)?));
        Ok(format!(
            "{}; {}; sha256={sha256}",
            binary.display(),
            version.trim()
        ))
    })();
    match result {
        Ok(detail) => Check {
            name: "installed binary".into(),
            status: "ok".into(),
            detail,
        },
        Err(err) => fail("installed binary", format!("{}: {err}", binary.display())),
    }
}

fn check_chrome() -> Check {
    let path = Path::new("/Applications/Google Chrome.app/Contents/MacOS/Google Chrome");
    if !path.is_file() {
        return fail(
            "Google Chrome",
            "not installed at the default macOS path".into(),
        );
    }
    let version = Command::new(path)
        .arg("--version")
        .output()
        .ok()
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|value| value.trim().to_string())
        .unwrap_or_else(|| "installed".into());
    Check {
        name: "Google Chrome".into(),
        status: "ok".into(),
        detail: version,
    }
}

fn check_macos_permissions() -> Check {
    let screen = Path::new("/usr/sbin/screencapture").is_file();
    let events = Path::new("/usr/bin/osascript").is_file();
    if screen && events {
        Check {
            name: "desktop commands".into(),
            status: "warn".into(),
            detail: "screencapture and osascript exist; Screen Recording and Accessibility permission are granted per host app and cannot be checked without invoking them".into(),
        }
    } else {
        fail(
            "desktop commands",
            "macOS desktop utilities not found".into(),
        )
    }
}

fn fail(name: &str, detail: String) -> Check {
    Check {
        name: name.into(),
        status: "fail".into(),
        detail,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use bridge_core::NATIVE_HOST_NAME;

    fn paths(root: &Path) -> HomePaths {
        HomePaths {
            base: root.into(),
            config_file: root.join("config.toml"),
            database: root.join("bridge.sqlite3"),
            logs_dir: root.join("logs"),
            default_project: root.join("workspace"),
            native_hosts_dir: root.join("hosts"),
            install_dir: root.join("bin"),
        }
    }

    #[test]
    fn doctor_checks_config_contents_and_exact_origin_without_creating_workspace() {
        let temp = tempfile::tempdir().unwrap();
        let paths = paths(temp.path());
        paths.ensure().unwrap();
        fs::write(&paths.config_file, "invalid = [").unwrap();
        assert_eq!(check_config(&paths).status, "fail");
        let mut config = Config::default();
        config.project_root = paths.default_project.clone();
        config.extension_id = Some("a".repeat(32));
        paths.save_config(&config).unwrap();
        assert_eq!(check_config(&paths).status, "ok");
        fs::write(paths.installed_binary(), "test file, not an executable").unwrap();
        fs::create_dir_all(&paths.native_hosts_dir).unwrap();
        let mut manifest = serde_json::json!({"name":NATIVE_HOST_NAME,"type":"stdio","path":paths.installed_binary(),"allowed_origins":[format!("chrome-extension://{}/", "a".repeat(32))]});
        fs::write(paths.native_manifest_path(), manifest.to_string()).unwrap();
        assert_eq!(check_native_manifest(&paths).status, "ok");
        manifest["allowed_origins"] =
            serde_json::json!([format!("chrome-extension://{}/", "b".repeat(32))]);
        fs::write(paths.native_manifest_path(), manifest.to_string()).unwrap();
        assert_eq!(check_native_manifest(&paths).status, "fail");
        assert_eq!(check_installed_binary(&paths).status, "fail");
        assert!(!paths.default_project.exists());
        assert!(!paths.database.exists());
    }
}
