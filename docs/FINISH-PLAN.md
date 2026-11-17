# Unified AI Bridge finish plan

November 17, 2026 · Version 0.5.30 · Protocol 10

The bridge now creates and reads back real code and documents, imports an explicitly selected chat download, and reuses approved native MCP connections. The focused implementation is ready for review, but fully automatic uploads and final acceptance of every browser control remain open. Checked items have recorded evidence; unchecked items are remaining work, not promises of completion.

## 1 Reliable chat execution

- [x] Complete explicit browser selection, sharing, approval, import and in-conversation hash verification of the real 284-byte bridge-roundtrip-0526.svg. Download began on 0.5.26; import finished on 0.5.30 in a new test conversation. This was manual continuation, not one unattended run.
- [x] Preserve valid generated-file handles when panel and model discovery use different scopes. Ownership, expiry and replay checks remain enforced.
- [ ] Prove panel and model handle coexistence in one live generated-file task. The source fix is reviewed, but this exact browser interaction remains unverified.
- [x] Verify ChatGPT setup, saved-result recovery, reload, SPA handoff and background result delivery. The 0.5.30 panel recognizes the observed Chat memory full notice. Recovery needed explicit inspection and clearing of the bridge-owned restored draft; no write repeated.
- [ ] Repeat affected-provider acceptance without development interventions. Other providers retain the individually qualified 0.5.25 evidence; they were not freshly certified in this pass.

## 2 Files and images in both directions

- [x] Implement approved local-file attachment for ChatGPT: exact hash, size, type, conversation, turn and repeat-attempt checks. Native allow, denial, changed-file, lock, session and interruption tests pass.
- [ ] Complete automatic image upload and result submission on the corrected adapter. A real PNG reached ChatGPT on 0.5.27; after a completion-selector timeout, manual Send let the model correctly quote Dummy PDF file. The account then exhausted its attachment quota. The final automatic path remains unverified.
- [x] Report unsupported providers, unsafe or changed files, occupied composers and uncertain transfers explicitly. Local write grants and YOLO do not authorize uploads. Supported text/source and PNG/JPEG/WebP files are limited to 512 KiB.

## 3 Reusable application connections

- [x] Reuse reviewed MCP processes within one workspace and conversation, with separate processes for other sessions. Expose actual connection counts.
- [x] Stop connections on grant revocation, lock, session closure, explicit disconnect and five-minute idle expiry. Eight tests cover cancellation, real process cleanup, reuse, isolation and reconnect behavior.
- [x] Inspect a real isolated Cargo project through installed native rust-analyzer-mcp. Two approved calls returned bridge_marker from src/lib.rs; connection count was one, then zero after disconnect. This verifies that tool, not every application operation.
- [x] Document the native-only boundary and sanitized environment. Node/Python MCP launchers remain unsupported. Roblox proxy calls returned no connected Studio instances; an open-place workflow remains unverified.

## 4 Simple everyday controls

- [x] Put workspace, connected chats, approvals, file exchange and Stop first. Group advanced controls without removing existing tools.
- [x] Verify in-page denial and docked-side-panel approval with the real SVG import. Stop disconnected the owned test chat; reconnect restored its session. The import result and hash returned while the tab was hidden.
- [x] Open the actual toolbar popup and verify its workspace, connected session, everyday controls and Refresh status. Popup Deny blocked a test PDF; the later handoff paused at the website’s account-change notice.

## 5 Release evidence

- [x] Pass the complete workspace suite: 268 passed, 0 failed, 1 intentionally ignored long command test on 0.5.29. The final 0.5.30 notice/send changes passed all 45 core and 6 extension tests. Installed 0.5.30 selftest passed 41 checks. Doctor passed configuration, native manifest, binary, SQLite and audit checks.
- [x] Build and install with the pinned deployment command; source, generated manifest, Chrome card, live panel and host hello all report 0.5.30 / protocol 10.
- [x] Verify actual code/document writes and read-backs, selected-file import and a native application inspection. Preserve failures and manual recovery steps in VERIFICATION.md.
- [x] Publish reviewed source to GitHub main as 1b59b1c and inspect CI run 44111377281. The run was queued at publication; this is not a CI pass.

## Remaining external gates

The corrected attachment workflow needs the provider quota to reset. Stable macOS permission persistence needs a user-owned signing identity and OS grants. Older Intel hardware and its supported macOS version still require physical acceptance. CI compilation cannot establish either permission persistence or hardware compatibility.

The archaeology project was not touched. Acceptance used the existing general scratch workspace with network and YOLO off, 16 calls per task and 4 per response. Unrelated drafts were preserved. No new providers or broad format expansion were added.
