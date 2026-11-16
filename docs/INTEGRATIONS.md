# Native MCP integrations

The bridge can connect to an explicitly registered native MCP executable in a selected workspace. Registering a server enables no tools. **Start and discover tools** launches it, reads its catalog and closes it; review each schema before granting a tool. Every external tool call requires approval, even if the server labels it read-only.

The program runs with your OS account permissions. The bridge's path policy is not an OS sandbox for that program. Register only an executable and fixed arguments you intend to run. The executable's hash and each granted input schema are pinned; a changed binary or schema requires review again. The bridge does not install packages, grant tools automatically or accept model-created server registrations.

## Current lifecycle controls

The 0.5.14 candidate adds **Check grant and executable** and the model-facing `integration_status` tool. It checks current configuration, workspace policy and the executable hash without starting the server. `configured_unprobed` deliberately does not claim a live connection. `blocked` includes the reason. Use explicit discovery to exercise server responsiveness.

**Remove grant / stop integration** remains available during discovery. Removal changes saved configuration; initialization, discovery and active calls check for grant changes or workspace locks every 250 ms. A changed or removed grant stops the operation's process group. These checks also catch changes after approval and immediately before dispatch.

Each connection belongs to one operation; it is not a persistent application session. Completion, timeout, validation failure and cancellation clean up the server process group, including descendants that remain in that group. A program can deliberately detach processes or cause external effects; this is not an OS confinement mechanism and cannot undo completed actions.

If a call has started when it is stopped, the result says its effects may be uncertain and it was not retried. A timeout or result-size failure also does not prove that the external operation failed. Inspect the actual application/files and audit outcome before proposing a fresh call. The outer bridge request journal handles replay protection; this candidate does not add resumable MCP task semantics.

## Limits

- Configuration: at most 16 servers / 200 KB per workspace, 128 tool grants per server, 32 KB per schema.
- Discovery: 10-second initialization and discovery deadlines, at most 128 tools / 200 KB, no paginated server catalogs in this subset.
- Calls: 32,000-byte argument objects, user-configured timeout from 1–120 seconds, 50,000-byte returned result. The RMCP stdio transport separately caps incoming lines at 16 MiB.
- Shutdown: process-group stop plus bounded service cleanup; no automatic operation retry or grant enlargement.

The existing integration test starts the real bridge RMCP server, discovers tools, grants a schema, and verifies a real file read with denied/ungranted/locked cases. Three additional lifecycle tests use a clearly labeled Rust RMCP test fixture to delay initialization/discovery/calls and spawn a real child process. They verify revocation, locking, timeout, no-start readiness and process cleanup. The fixture is not an AI provider or a supported third-party application integration.

UAB-24 remains open for persistent application-server workflows, progress, per-call cancellation controls, richer bounded result delivery, image handoff and live browser acceptance. There is no claim that an arbitrary external server is compatible simply because discovery succeeds.
