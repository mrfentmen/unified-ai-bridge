//! Rust-only packaging helpers. No image editor, Node script, or handwritten
//! asset source is needed: icons, deterministic extension IDs, and release
//! manifest patching are generated here.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use base64::Engine;
use png::{BitDepth, ColorType, Encoder};
use sha2::{Digest, Sha256};

mod deploy;

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("deploy") => deploy::run(),
        Some("verify-extension") => {
            deploy::verify_versions(&workspace_root().join("crates/bridge-extension/dist/chromium"))?;
            Ok(())
        }
        Some("icons") => {
            let root = args
                .next()
                .map(PathBuf::from)
                .unwrap_or_else(|| workspace_root().join("crates/bridge-extension/static/icons"));
            generate_icons(&root)?;
            println!("Generated extension icons in {}", root.display());
            Ok(())
        }
        Some("finalize-extension") => {
            let dist = args
                .next()
                .map(PathBuf::from)
                .unwrap_or_else(|| workspace_root().join("crates/bridge-extension/dist/chromium"));
            finalize_extension(&dist)
        }
        Some(other) => anyhow::bail!(
            "unknown bridge-tooling command `{other}` (expected `deploy`, `verify-extension`, `icons` or `finalize-extension`)"
        ),
        None => {
            let root = workspace_root().join("crates/bridge-extension/static/icons");
            generate_icons(&root)?;
            println!("Generated extension icons in {}", root.display());
            Ok(())
        }
    }
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crate is inside the workspace")
        .to_path_buf()
}

fn finalize_extension(dist: &Path) -> Result<()> {
    deploy::verify_versions(dist)?;
    let manifest_path = dist.join("manifest.json");
    if !manifest_path.is_file() {
        bail!(
            "{} is missing; run `cargo oxichrome build` in crates/bridge-extension first",
            manifest_path.display()
        );
    }
    for required in [
        "background.js",
        "popup.html",
        "sidepanel.html",
        "wasm/bridge_extension_bg.wasm",
    ] {
        if !dist.join(required).is_file() {
            bail!(
                "extension bundle is incomplete: {} is missing",
                dist.join(required).display()
            );
        }
    }
    let key_path = workspace_root().join("packaging/extension-public-key.txt");
    let key = fs::read_to_string(&key_path)
        .with_context(|| format!("read {}", key_path.display()))?
        .trim()
        .to_string();
    let extension_id = extension_id_from_key(&key)?;
    let mut manifest: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(&manifest_path).context("read generated manifest")?,
    )
    .context("parse generated manifest")?;
    let object = manifest
        .as_object_mut()
        .context("generated manifest is not an object")?;
    object.insert("key".into(), serde_json::Value::String(key));
    let rendered = serde_json::to_string_pretty(&manifest)? + "\n";
    atomic_write(&manifest_path, rendered.as_bytes())?;
    fs::write(dist.join("extension-id.txt"), format!("{extension_id}\n"))?;
    println!("Finalized {}", dist.display());
    println!("  extension id: {extension_id}");
    println!("  manifest key: packaging/extension-public-key.txt");
    println!("\nInstall with:");
    println!(
        "  unified-ai-bridge-native install --extension-dir {}",
        dist.display()
    );
    Ok(())
}

fn extension_id_from_key(encoded: &str) -> Result<String> {
    let der = base64::engine::general_purpose::STANDARD
        .decode(encoded.trim())
        .context("extension public key is not valid base64")?;
    let digest = Sha256::digest(&der);
    let mut id = String::with_capacity(32);
    for byte in digest.iter().take(16) {
        id.push(char::from(b'a' + (byte >> 4)));
        id.push(char::from(b'a' + (byte & 0x0f)));
    }
    if id.len() != 32 || !id.chars().all(|ch| ('a'..='p').contains(&ch)) {
        bail!("derived an invalid Chrome extension id");
    }
    Ok(id)
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let tmp = path.with_file_name(format!(
        ".{}.tmp-{}",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("manifest"),
        std::process::id()
    ));
    fs::write(&tmp, bytes).with_context(|| format!("write {}", tmp.display()))?;
    fs::rename(&tmp, path).with_context(|| format!("replace {}", path.display()))?;
    Ok(())
}

fn generate_icons(directory: &Path) -> Result<()> {
    fs::create_dir_all(directory)
        .with_context(|| format!("create icon directory {}", directory.display()))?;
    for size in [16u32, 32, 48, 128] {
        let path = directory.join(format!("{size}.png"));
        write_icon(&path, size)?;
    }
    Ok(())
}

fn write_icon(path: &Path, size: u32) -> Result<()> {
    let pixels = render_icon(size, 4);
    let file = fs::File::create(path).with_context(|| format!("create {}", path.display()))?;
    let mut encoder = Encoder::new(file, size, size);
    encoder.set_color(ColorType::Rgba);
    encoder.set_depth(BitDepth::Eight);
    let mut writer = encoder.write_header().context("write PNG header")?;
    writer
        .write_image_data(&pixels)
        .context("write PNG pixels")?;
    Ok(())
}

fn render_icon(size: u32, samples: u32) -> Vec<u8> {
    let mut pixels = Vec::with_capacity((size * size * 4) as usize);
    let scale = size as f64;
    for y in 0..size {
        for x in 0..size {
            let mut rgb = [0.0f64; 3];
            let mut covered = 0u32;
            for sy in 0..samples {
                for sx in 0..samples {
                    let px = (x as f64 + (sx as f64 + 0.5) / samples as f64) / scale;
                    let py = (y as f64 + (sy as f64 + 0.5) / samples as f64) / scale;
                    if let Some(color) = sample(px, py) {
                        rgb[0] += color[0];
                        rgb[1] += color[1];
                        rgb[2] += color[2];
                        covered += 1;
                    }
                }
            }
            let total = (samples * samples).max(1) as f64;
            if covered == 0 {
                pixels.extend_from_slice(&[0, 0, 0, 0]);
            } else {
                let n = covered as f64;
                pixels.push((rgb[0] / n).round().clamp(0.0, 255.0) as u8);
                pixels.push((rgb[1] / n).round().clamp(0.0, 255.0) as u8);
                pixels.push((rgb[2] / n).round().clamp(0.0, 255.0) as u8);
                pixels.push(((n / total) * 255.0).round().clamp(0.0, 255.0) as u8);
            }
        }
    }
    pixels
}

fn sample(x: f64, y: f64) -> Option<[f64; 3]> {
    // Rounded slate tile.
    if !inside_rounded(x, y, 0.03, 0.20) {
        return None;
    }
    let background = [15.0, 23.0, 30.0];
    let border = (13.0, 61.0, 78.0);
    let gold = [214.0, 179.0, 106.0];
    let muted_gold = [150.0, 122.0, 70.0];
    let mut color = background;

    if !inside_rounded(x, y, 0.075, 0.16) {
        color = [border.0, border.1, border.2];
    }

    if (0.18..0.82).contains(&x) && (0.66..0.74).contains(&y) {
        color = gold;
    }
    for cx in [0.26, 0.74] {
        if (cx - 0.045..cx + 0.045).contains(&x) && (0.38..0.70).contains(&y) {
            color = gold;
        }
    }

    let dx = x - 0.5;
    let dy = y - 0.61;
    let radius = (dx * dx + dy * dy).sqrt();
    if (0.19..0.27).contains(&radius) && y <= 0.61 && x > 0.24 && x < 0.76 {
        color = muted_gold;
    }

    for cx in [0.34, 0.66] {
        if (cx - 0.012..cx + 0.012).contains(&x) && (0.48..0.66).contains(&y) {
            color = muted_gold;
        }
    }

    Some(color)
}

fn inside_rounded(x: f64, y: f64, radius: f64, inset: f64) -> bool {
    let min = inset;
    let max = 1.0 - inset;
    if x < min || x > max || y < min || y > max {
        return false;
    }
    let cx = x.clamp(min + radius, max - radius);
    let cy = y.clamp(min + radius, max - radius);
    let dx = x - cx;
    let dy = y - cy;
    dx * dx + dy * dy <= radius * radius
}
