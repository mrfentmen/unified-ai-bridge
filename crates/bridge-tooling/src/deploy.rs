//! Build and install using one rustup toolchain, then require an explicit browser reload.
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, ensure, Context, Result};
use fs2::FileExt;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::{
    atomic_write, extension_id_from_key, finalize_extension, generate_icons, workspace_root,
};

struct Toolchain {
    cargo: PathBuf,
    rustc: PathBuf,
    path: OsString,
}

impl Toolchain {
    fn discover(root: &Path) -> Result<Self> {
        let locate = |name: &str| -> Result<PathBuf> {
            let output = Command::new("rustup")
                .args(["which", name])
                .current_dir(root)
                .output()
                .context("rustup must be installed and available on PATH")?;
            ensure!(
                output.status.success(),
                "rustup which {name} failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            let path = PathBuf::from(String::from_utf8(output.stdout)?.trim());
            ensure!(
                path.is_absolute() && path.is_file(),
                "rustup returned an invalid {name} path: {}",
                path.display()
            );
            Ok(path)
        };
        let cargo = locate("cargo")?;
        let rustc = locate("rustc")?;
        ensure!(
            cargo.parent() == rustc.parent(),
            "cargo and rustc belong to different toolchains"
        );
        let cargo_home = std::env::var_os("CARGO_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".cargo")
            });
        let mut paths = vec![
            rustc.parent().unwrap().to_path_buf(),
            cargo_home.join("bin"),
        ];
        paths.extend(std::env::split_paths(
            &std::env::var_os("PATH").unwrap_or_default(),
        ));
        let path = std::env::join_paths(paths)?;
        let target = Command::new(&rustc)
            .args([
                "--print",
                "target-libdir",
                "--target",
                "wasm32-unknown-unknown",
            ])
            .output()?;
        ensure!(
            target.status.success(),
            "failed to query wasm target for {}",
            rustc.display()
        );
        let target = PathBuf::from(String::from_utf8(target.stdout)?.trim());
        ensure!(target.is_dir() && fs::read_dir(&target)?.any(|entry| entry.is_ok_and(|e| e.file_name().to_string_lossy().starts_with("libstd-"))),
            "wasm32-unknown-unknown is not installed for {}; install it with rustup before deploying", rustc.display());
        Ok(Self { cargo, rustc, path })
    }

    fn cargo(&self, directory: &Path) -> Command {
        let mut command = Command::new(&self.cargo);
        command
            .current_dir(directory)
            .env("CARGO", &self.cargo)
            .env("RUSTC", &self.rustc)
            .env("RUSTDOC", self.rustc.parent().unwrap().join("rustdoc"))
            .env("PATH", &self.path);
        // Oxichrome runs `cargo` by name; PATH must be pinned as well as CARGO/RUSTC.
        command
    }
}

fn checked(command: &mut Command, description: &str) -> Result<()> {
    let status = command
        .status()
        .with_context(|| format!("start {description}"))?;
    ensure!(
        status.success(),
        "{description} failed ({status}); deployment is incomplete, do not reload Chrome"
    );
    Ok(())
}

pub fn run() -> Result<()> {
    let root = workspace_root();
    let _lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(root.join(".bridge-deploy.lock"))?;
    _lock
        .try_lock_exclusive()
        .context("another bridge deployment is running")?;
    let toolchain = Toolchain::discover(&root)?;
    println!("Pinned CARGO={}", toolchain.cargo.display());
    println!("Pinned RUSTC={}", toolchain.rustc.display());
    let extension = root.join("crates/bridge-extension");
    let dist = extension.join("dist/chromium");
    // Check source agreement before touching the currently generated bundle.
    source_version(&extension)?;
    generate_icons(&extension.join("static/icons"))?;
    if dist.exists() {
        fs::remove_dir_all(&dist).context(
            "clear generated dist/chromium; stale bundles must not survive a failed build",
        )?;
    }
    checked(
        toolchain
            .cargo(&extension)
            .args(["oxichrome", "build", "--release"]),
        "extension build",
    )?;
    finalize_extension(&dist)?;
    let version = verify_versions(&dist)?;

    // Force a known target directory so installation never picks an unrelated cached binary.
    let target = root.join("target/bridge-deploy");
    checked(
        toolchain
            .cargo(&root)
            .env("CARGO_TARGET_DIR", &target)
            .args([
                "build",
                "--locked",
                "--release",
                "-p",
                "bridge-native",
                "-p",
                "bridge-tooling",
            ]),
        "native and tooling release build",
    )?;
    let binary = target.join("release/unified-ai-bridge-native");
    checked(
        Command::new(&binary)
            .arg("install")
            .arg("--extension-dir")
            .arg(&dist),
        "native host install",
    )?;
    verify_install(&binary, &dist)?;

    let receipt = json!({
        "extension_version": version,
        "cargo": toolchain.cargo,
        "rustc": toolchain.rustc,
        "native_sha256": digest(&binary)?,
        "wasm_sha256": digest(&dist.join("wasm/bridge_extension_bg.wasm"))?,
        "browser_reload_verified": false,
    });
    atomic_write(
        &dist.join("build-receipt.json"),
        (serde_json::to_string_pretty(&receipt)? + "\n").as_bytes(),
    )?;
    println!("\nBUILD AND INSTALL PASS: extension {version}. Chrome reload is still REQUIRED.");
    println!("Open chrome://extensions, find Unified AI Bridge (gdoojijdmbacpolhfbgebeciagllgcop), and click Reload.");
    println!("Reload each provider tab. Confirm chrome://extensions and the panel footer both show {version}.");
    println!("Confirm the NEW host log line says `extension hello ({version})`. A build receipt is not proof of a loaded build.");
    Ok(())
}

fn digest(path: &Path) -> Result<String> {
    Ok(format!("{:x}", Sha256::digest(fs::read(path)?)))
}

fn attribute_version(source: &str) -> Result<String> {
    let file = syn::parse_file(source).context("parse extension src/lib.rs")?;
    for item in file.items {
        let syn::Item::Struct(item) = item else {
            continue;
        };
        for attr in item.attrs {
            if !attr.path().is_ident("extension") {
                continue;
            }
            let syn::Meta::List(list) = attr.meta else {
                continue;
            };
            let tokens: Vec<_> = list.tokens.into_iter().collect();
            for window in tokens.windows(3) {
                if window[0].to_string() == "version" && window[1].to_string() == "=" {
                    return Ok(syn::parse_str::<syn::LitStr>(&window[2].to_string())?.value());
                }
            }
        }
    }
    bail!("extension attribute has no literal version in src/lib.rs")
}

fn check_versions(source: &str, package: &str, manifest: &Value) -> Result<String> {
    let declared = attribute_version(source)?;
    let package: toml::Value = toml::from_str(package)?;
    let package = package["package"]["version"]
        .as_str()
        .context("extension Cargo.toml version must be literal")?;
    let emitted = manifest["version"]
        .as_str()
        .context("generated manifest has no version")?;
    ensure!(declared == package && declared == emitted,
        "STALE EXTENSION BUILD: lib.rs={declared}, Cargo.toml={package}, dist/chromium/manifest.json={emitted}. Run cargo bridge-deploy; do not reload this bundle.");
    Ok(declared)
}

fn source_version(extension: &Path) -> Result<String> {
    let source = fs::read_to_string(extension.join("src/lib.rs"))?;
    let package = fs::read_to_string(extension.join("Cargo.toml"))?;
    let version = attribute_version(&source)?;
    check_versions(&source, &package, &json!({"version": version}))
}

pub fn verify_versions(dist: &Path) -> Result<String> {
    let extension = workspace_root().join("crates/bridge-extension");
    let version = check_versions(
        &fs::read_to_string(extension.join("src/lib.rs"))?,
        &fs::read_to_string(extension.join("Cargo.toml"))?,
        &serde_json::from_slice(&fs::read(dist.join("manifest.json"))?)?,
    )?;
    println!("PASS version: lib.rs={version}, Cargo.toml={version}, dist/chromium/manifest.json={version}");
    Ok(version)
}

fn validate_manifest(manifest: &Value, installed: &Path, id: &str, host: &str) -> Result<()> {
    ensure!(
        manifest["name"] == host && manifest["type"] == "stdio",
        "native manifest name/type mismatch"
    );
    ensure!(
        manifest["path"].as_str().map(Path::new) == Some(installed),
        "native manifest points at an unexpected executable"
    );
    ensure!(
        manifest["allowed_origins"] == json!([format!("chrome-extension://{id}/")]),
        "native manifest must allow only the packaged extension origin"
    );
    Ok(())
}

fn verify_install(binary: &Path, dist: &Path) -> Result<()> {
    let home = PathBuf::from(std::env::var_os("HOME").context("HOME missing")?);
    let base = std::env::var_os("UAB_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join("Library/Application Support/unified-ai-bridge"));
    let hosts = std::env::var_os("UAB_NATIVE_HOSTS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            home.join("Library/Application Support/Google/Chrome/NativeMessagingHosts")
        });
    let config: toml::Value = toml::from_str(&fs::read_to_string(base.join("config.toml"))?)?;
    let host = bridge_core::NATIVE_HOST_NAME;
    ensure!(
        config["native_host_name"].as_str() == Some(host),
        "configured native host name mismatch"
    );
    let extension: Value = serde_json::from_slice(&fs::read(dist.join("manifest.json"))?)?;
    let id = extension_id_from_key(extension["key"].as_str().context("manifest key missing")?)?;
    ensure!(
        config["extension_id"].as_str() == Some(&id),
        "config extension id differs from packaged extension key"
    );
    ensure!(
        fs::read_to_string(dist.join("extension-id.txt"))?.trim() == id,
        "packaged extension id file differs from key"
    );
    let manifest_path = hosts.join(format!("{host}.json"));
    let manifest: Value = serde_json::from_slice(&fs::read(&manifest_path)?)?;
    let installed = base.join("bin/unified-ai-bridge-native");
    validate_manifest(&manifest, &installed, &id, host)?;
    ensure!(
        digest(binary)? == digest(&installed)?,
        "installed native host differs from the release build"
    );
    checked(
        Command::new(&installed).arg("--version"),
        "installed native host version check",
    )?;
    println!("PASS installed binary SHA-256: {}", digest(&installed)?);
    println!(
        "PASS native manifest: {} -> {} (only {id})",
        manifest_path.display(),
        installed.display()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    const SOURCE: &str = "#[extension(name = \"Bridge\", version = \"0.2.1\", permissions = [Storage])] pub struct Bridge;";
    const PACKAGE: &str = "[package]\nversion = '0.2.1'";

    #[test]
    fn versions_must_match_source_package_and_bundle() {
        assert_eq!(
            check_versions(SOURCE, PACKAGE, &json!({"version":"0.2.1"})).unwrap(),
            "0.2.1"
        );
        assert!(check_versions(SOURCE, PACKAGE, &json!({"version":"0.2.0"}))
            .unwrap_err()
            .to_string()
            .contains("STALE EXTENSION BUILD"));
        assert!(check_versions(
            SOURCE,
            "[package]\nversion = '0.2.0'",
            &json!({"version":"0.2.1"})
        )
        .is_err());
        assert!(check_versions(SOURCE, PACKAGE, &json!({})).is_err());
    }

    #[test]
    fn manifest_requires_exact_installed_path_and_only_expected_origin() {
        let path = Path::new("/test/bridge");
        let mut manifest = json!({"name":"host", "type":"stdio", "path":"/test/bridge", "allowed_origins":["chrome-extension://abc/"]});
        validate_manifest(&manifest, path, "abc", "host").unwrap();
        manifest["path"] = json!("/stale/bridge");
        assert!(validate_manifest(&manifest, path, "abc", "host").is_err());
        manifest["path"] = json!("/test/bridge");
        manifest["allowed_origins"] =
            json!(["chrome-extension://abc/", "chrome-extension://other/"]);
        assert!(validate_manifest(&manifest, path, "abc", "host").is_err());
    }
}
