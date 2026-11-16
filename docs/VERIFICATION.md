# Verification record

These are recorded results from local development, not a hosted CI badge or a guarantee of every environment. The most recent live provider run used extension/native host **0.4.6**, protocol **4**, on **macOS 27.0.1, Apple Silicon, Chrome 157**, with Rust **1.99.1**.

Raw transcripts, session IDs, conversation URLs, machine paths, audit exports, and build logs are retained locally and excluded from the public repository. This document summarizes those observations without publishing the chats.

## Build and host checks — November 16, 2026

| Check | Recorded result |
| --- | --- |
| Workspace tests | 73 passed, 0 failed: 32 core, 15 native unit, 7 filesystem, 1 real MCP subprocess, 7 Native Messaging, 9 workspace/command, 2 tooling |
| Native/tooling release build | Exit 0 |
| WASM/Oxichrome release build | Exit 0; source, package and generated manifest all 0.4.6 |
| Loaded extension | Chrome's extension details, both new-provider shadow panels, and a fresh host hello agreed on 0.4.6 |
| Installed-host self-test | 41 passed, 0 failed; seven provider identities and 35 concurrent writes in a temporary workspace |
| Direct framed handshake | `hello_ack`, host 0.4.6, protocol 4, active general workspace |
| Doctor | Configuration, binary, exact native extension origin, SQLite integrity and audit passed; 301 matching DB/audit-file entries at capture |

The installed-host self-test ended with:

```text
41 passed, 0 failed. All files were created under a temporary directory and removed afterward.
```

The deployment output included:

```text
PASS version: lib.rs=0.4.6, Cargo.toml=0.4.6, dist/chromium/manifest.json=0.4.6
BUILD AND INSTALL PASS: extension 0.4.6. Chrome reload is still REQUIRED.
```

Chrome was subsequently reloaded and checked separately. A successful build receipt alone does not establish the loaded version.

## Qwen and Kimi — live 0.4.6 acceptance

Both ran against a general scratch workspace. The provider model emitted the calls, the native host performed them, and the extension returned actual results into the conversation.

| Check | Kimi | Qwen |
| --- | --- | --- |
| Read an existing source file | Passed | Passed |
| Create a new Markdown file | Passed | Passed |
| Read back exact requested content | Passed | Passed |
| Outside-root write | `permission_denied`; no file created | `permission_denied`; no file created |
| Results delivered to the same conversation | Five result blocks, automatically | Four result blocks, automatically after a candidate selection |
| New chat → sidebar return | One connected panel | One connected panel |
| Remove bridge panel element | Reattached once, connected | Reattached once, connected |
| Hidden-tab connection between probes | 38.369 seconds, connected | 45.066 seconds, connected |

Each proof file contained one sentence plus a newline, exactly **44 bytes**:

| File | SHA-256 |
| --- | --- |
| `files/kimi-046-proof.md` | `1059f1eb76be2ad48704772e1e2d7d39cd63ff5377e9b8c438b41658c1b16331` |
| `files/qwen-046-proof.md` | `581d176dee929faaf257f2ff79724015b30bfb8ef9c4b9d366aacd03092192c4` |

Host log writes were recorded at **12:32:45.350 UTC** for KIMI and **12:34:47.361 UTC** for QWEN, with the correct workspace attribution and committed recovery records.

Kimi also completed two independent reads with identical arguments and output. Its persistent outer message IDs avoided the collision found with text-derived identities. The editor regression test checks that asynchronous insertion can normalize line breaks without accepting changed, partial, or empty result text.

Qwen presented two candidate answers during the denial test. The bridge paused without executing either; selecting one resumed the test. Its code-editor rejection and empty-user-text guard remain deliberate limitations, not successful background execution claims.

## Earlier provider evidence

ChatGPT, Claude, DeepSeek, and Gemini completed read/create/result loops on 0.4.0. ChatGPT, DeepSeek, and Gemini also passed navigation checks; background connection was observed for all four. Claude's file was real and its hash matched, but the model omitted a requested period.

Grok on `grok.com` connected and returned a real file read. An account limit prevented create-file acceptance. Grok on X connected on 0.4.2, but no full file loop was verified. These earlier results do not establish current-build acceptance for those providers.

## Repeat the browser test

1. Build/install, reload the extension and provider page, and compare the Chrome card, shadow panel, and fresh host hello. Record the actual build and workspace.
2. Use a dedicated conversation and a fresh file path. Ask for one tool at a time: read an existing file, create a Markdown file with a specified sentence and newline, then read it back. Wait for a real result block between calls.
3. On Qwen, request a compact plain-text `[[BRIDGE_TOOL]]` JSON `[[/BRIDGE_TOOL]]` line without Markdown fences. Select one answer if the site presents alternatives.
4. Request a `create_only` write to `../bridge-denied-test.md`. Expect a visible denial, no file, and an audit event. Do not retry around the guard.
5. Compare file bytes and hash with the returned result. Check the recovery record and host attribution. For Kimi, repeat a read with identical arguments to exercise distinct message IDs.
6. Navigate New chat → select the original chat from the sidebar. Expect one connected panel and no repeated writes. Hide the tab for more than 30 seconds and check registration separately from tool execution.

For approval testing, use a disposable file and its fresh hash. Request deletion, deny it once, and confirm the file remains. Request it again, approve it, confirm it moves to workspace trash, then exercise approved restore. This remains required live acceptance for the current UI; native tests alone do not complete it.

## Still unverified

- Physical Intel Mac compatibility and the minimum macOS version.
- Current-build acceptance of every older provider, all-provider concurrency, and interruption recovery across all sites.
- Every live approval/recovery control and third-party MCP server.
- Fully unfocused Qwen tool execution, long virtualized transcripts, and alternate provider modes/layouts.
- A permanent solution to macOS folder-access prompts after unsigned host rebuilds.
