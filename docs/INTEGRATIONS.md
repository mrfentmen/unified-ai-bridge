# Native MCP integrations

The bridge can connect to an explicitly registered native MCP executable in a selected workspace. Registering a server enables no tools. **Start and discover tools** launches it, reads its catalog and closes it; review each schema before granting a tool. Every external tool call requires approval, even if the server labels it read-only.

The program runs with your OS account permissions. The bridge's path policy is not an OS sandbox for that program. Register only an executable and fixed arguments you intend to run. The executable's hash and each granted input schema are pinned; a changed binary or schema requires review again. The bridge does not install packages, grant tools automatically or accept model-created server registrations.

## Current lifecycle controls

**Check connection and grant** and the model-facing `integration_status` tool inspect current configuration, workspace policy and the executable hash without starting a server. `connected` includes the number of live session connections; `configured_unprobed` means there is no retained connection. Neither state proves an application has an open project. `blocked` includes the reason.

**Remove grant / stop integration** remains available during discovery. Removal changes saved configuration; initialization, discovery and active calls check for grant changes or workspace locks every 250 ms. A changed or removed grant stops the operation's process group. These checks also catch changes after approval and immediately before dispatch.

Approved calls reuse a process owned by one workspace, conversation session and server. Another conversation receives a separate process. Connections close after five idle minutes, session closure, workspace switching or locking, grant changes, timeout or failed validation. Every call still checks the executable and advertised schema and requires approval. Discovery always uses a separate temporary process.

**Disconnect (keep grants)** closes idle connections and cancels waiting approvals, initialization, discovery within a call and active calls. A later explicit approved call can reconnect. Removal also revokes the saved configuration. Shutdown kills the server process group, including descendants that remain in that group. A program can deliberately detach processes or cause external effects; this is not an OS confinement mechanism and cannot undo completed actions.

If a call has started when it is stopped, the result says its effects may be uncertain and it was not retried. A timeout or result-size failure also does not prove that the external operation failed. Inspect the actual application/files and audit outcome before proposing a fresh call. The outer bridge request journal handles replay protection; this release does not add resumable MCP task semantics.

## Limits

- Configuration: at most 16 servers / 200 KB per workspace, 128 tool grants per server, 32 KB per schema.
- Discovery: 10-second initialization and discovery deadlines, at most 128 tools / 200 KB, no paginated server catalogs in this subset.
- Calls: 32,000-byte argument objects, user-configured timeout from 1–120 seconds, 50,000-byte returned result. The RMCP stdio transport separately caps incoming lines at 16 MiB.
- Shutdown: process-group stop plus bounded service cleanup; no automatic operation retry or grant enlargement.
- Pool: at most 32 retained or active conversation/server connections per native host, one call at a time on each connection, five-minute idle expiry.

Eight integration tests passed in the 0.5.29 full workspace run. They include a real bridge RMCP subprocess reading a temporary file, plus a labeled Rust transport fixture proving same-session PID reuse, separate-session isolation, cancellation, child cleanup and expiry. Expiry advances Tokio's test clock; the killed processes are real. The fixture is not a supported third-party application.

The installed Roblox Studio native MCP proxy also completed two approved `list_roblox_studios` calls through an isolated bridge host and reported one retained connection, then zero after disconnect. It returned `studios: []`, so this proves real proxy transport, not an open-place workflow. Roblox open-place acceptance remains open.

Only installed native executables are supported. Node, Python, shell and package-manager launchers are excluded; many third-party MCP servers therefore cannot be registered. Progress notifications, MCP task resumption and rich external image-result delivery remain separate work. Ordinary local coding and document tasks use the bridge's filesystem tools and need no application integration.

The installed native `rust-analyzer-mcp` 0.4.0 completed two separately approved `rust_analyzer_symbols` calls against an isolated real Cargo project. Both returned `bridge_marker` from `src/lib.rs`; connection status stayed at one and fell to zero after explicit disconnect. Its first call had failed because the server could not find its already installed rust-analyzer dependency. Linking that binary at `~/.cargo/bin/rust-analyzer`, a location the server explicitly searches, resolved discovery without changing the bridge’s sanitized PATH. This proves the symbol-inspection workflow, not every language-server operation or browser-to-application flow.
