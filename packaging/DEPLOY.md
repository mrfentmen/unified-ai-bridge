# Build, install, and load

From an already configured checkout, run `cargo +stable bridge-deploy`.

## First installation

Install Rust through rustup, the Xcode Command Line Tools, and Google Chrome.
Use a current stable toolchain; the recorded 0.4.7 build used Rust 1.99.1.

```sh
rustup toolchain install stable
rustup target add --toolchain stable wasm32-unknown-unknown
export PATH="$HOME/.cargo/bin:$PATH"
cargo +stable install --locked \
  --git https://github.com/0xsouravm/oxichrome \
  --rev f36ed22c007541e47308904ee925e21aa544e431 \
  cargo-oxichrome

cargo +stable run --locked -p bridge-native -- setup \
  --project "$HOME/Bridge Workspaces/bootstrap"
cargo +stable bridge-deploy
```

The current setup command creates a research-profile skeleton. Use a new scratch
folder for this bootstrap step. After loading the extension, open **Workspace
controls**, select your intended folder and the **general** or **code** profile,
and click **Create / use folder**. Existing files are not moved or overwritten.
Do not rerun setup on an established installation just to change workspaces;
use the workspace controls.

Open `chrome://extensions`, enable Developer mode, and load
`crates/bridge-extension/dist/chromium` as an unpacked extension. Fully restart
Chrome once after the first native-host registration, preserving your tabs.
Then reopen a supported website, sign in, and check the bridge panel.

## Subsequent deployments

The Rust tooling resolves cargo and rustc through rustup, checks that that
toolchain owns `wasm32-unknown-unknown`, and pins CARGO, RUSTC **and PATH** for
Oxichrome. Install cargo-oxichrome in your Cargo bin directory first. Oxichrome
uses a matching wasm-bindgen-cli or installs the version matching Cargo.lock
into its own cache. Missing tools or targets fail the command.

If Homebrew's Cargo is taking precedence, explicitly select rustup's binaries:

```sh
export CARGO="$(rustup which --toolchain stable cargo)"
export RUSTC="$(rustup which --toolchain stable rustc)"
export RUSTDOC="$(rustup which --toolchain stable rustdoc)"
export PATH="$(dirname "$CARGO"):$HOME/.cargo/bin:$PATH"
"$CARGO" bridge-deploy
```

Deployment clears the generated Chromium directory, rebuilds it, and refuses
any mismatch between the version in the extension attribute, extension
Cargo.toml, and generated manifest. It builds native/tooling in release mode,
installs the native host, verifies its hash against the new binary, and checks
the native manifest's exact path, host name, and sole allowed extension origin.
It requires an existing configuration and does not initialize a workspace.

Chrome still needs a reload. Open `chrome://extensions`, find **Unified AI
Bridge**, and click **Reload**. Reload each provider tab. Confirm all three:

1. `chrome://extensions` displays the generated manifest version.
2. Each provider panel footer displays that same build.
3. A new `extension hello (<version>)` line appears in
   `~/Library/Application Support/unified-ai-bridge/logs/native-host.log`.

If Reload is unavailable, fully quit and reopen Chrome, then reopen the desired
provider tabs. Preserve unrelated tabs before restarting. The receipt in
`dist/chromium/build-receipt.json` records artifact hashes and explicitly does
not certify that Chrome loaded them. Never diagnose live behavior until the
loaded version checks above pass.

`cargo +stable run --locked -p bridge-tooling -- verify-extension` repeats the version
check without rebuilding. The actual native host name is
`com.unified_ai_bridge.host`; its registration file is
`~/Library/Application Support/Google/Chrome/NativeMessagingHosts/com.unified_ai_bridge.host.json`.

## Installed files and diagnostics

The default application directory is
`~/Library/Application Support/unified-ai-bridge/`. It contains `config.toml`,
`bridge.sqlite3`, the audit history, operational logs, and the installed binary.
These files belong to the local installation and must not be committed.

```sh
"$HOME/Library/Application Support/unified-ai-bridge/bin/unified-ai-bridge-native" doctor
"$HOME/Library/Application Support/unified-ai-bridge/bin/unified-ai-bridge-native" selftest
```

macOS may ask again for folder access when an unsigned binary is rebuilt.
Use the appropriate Files & Folders permission, or a deliberate Full Disk Access
grant for the host and Chrome, if needed. Screen capture and accessibility
tools have separate OS grants. Do not reset the keychain to address these prompts.

The public key in `packaging/extension-public-key.txt` is intentionally public.
It provides a deterministic unpacked extension ID so the native manifest can
restrict access to that origin. It is not a signing credential.
