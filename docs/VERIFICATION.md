# Verification record

These are recorded results, not a guarantee of every environment. The latest local checks use extension/native host **0.5.7**, protocol **6**, on **macOS 27.0.1, Apple Silicon, Chrome 157**, with Rust **1.99.1**. Provider observations below name the build and distinguish foreground intervention from background registration.

## Adapter and concurrent conversation checks 0.5.7 — November 16, 2026

| Check | Observed result |
|---|---|
| Workspace suite | 129 passed, 0 failed; one deliberately slow command test ignored in the routine run |
| Installed selftest | 41 passed, 0 failed; temporary files removed afterward |
| Deterministic deployment | Native/tooling and WASM builds exited 0; source, package, generated manifest, Chrome card, seven panels and host hello all 0.5.7/protocol 6 |
| Hidden-tab registration | All seven dedicated verification tabs registered after reload without being brought forward, with fresh host log entries between 18:03:05.706 and 18:03:05.743 UTC |
| Provider file loops | ChatGPT, Claude, DeepSeek, Gemini, Grok on X, Qwen and Kimi each read their earlier real output, created a new file and read it back |
| Provider approvals | Each then previewed a new one-file patch, requested apply, received an explicit in-page approval, created its reviewed file and read it back into the same conversation |
| Concurrent routing | Seven tasks were submitted over eight seconds. Five patch requests overlapped while waiting for approval. All 14 outputs had exact expected bytes, one committed change each, and the correct agent in recovery/audit records; no cross-talk or duplicate writes observed |
| Claude fix | Reads only the transcript roots inside the assistant wrapper, excluding its truncated hidden announcement. Fenced JSON completed the full loop. New chat/sidebar return retained one connected panel; the unrelated saved draft remained intact |
| Busy composer | Before approving ChatGPT's patch, a known test draft was entered. The completed result waited, the draft remained byte-for-byte unchanged, and the panel offered Send pending result. Clearing only that test draft and clicking the button delivered the result; the model read the new file and finished |
| Limitations observed | ChatGPT initially emitted only an opening marker; it executed nothing and required a new task with fenced JSON. Qwen paused on hidden message text and two candidate answers; foregrounding and selecting one identical read-only candidate allowed completion. These are not unattended-pass claims |
| Audit/DB | Post-loop doctor passed configuration, manifest, binary, SQLite and the hash-chained audit |
| External checks | Grok.com remains quota-blocked; persistent macOS consent and a physical older Intel Mac remain unverified. 0.5.6 CI passed Linux/Intel macOS tests and Intel packaging; 0.5.7 CI is recorded separately after push |

Each provider created `files/bridge-057-<provider>.md` containing `Bridge 0.5.7 <provider> verified.`, then an approved patch created `files/bridge-057-<provider>-reviewed.md` containing `Approved batch for <provider> on 0.5.7.`. Provider slugs are `chatgpt`, `claude`, `deepseek`, `gemini`, `grok-x`, `qwen`, and `kimi`. None has a trailing newline. All **14 files** were checked against those exact bytes and their returned hashes. The final reviewed-file result was inspected in each real conversation, including DeepSeek's virtualized transcript after foregrounding.

Observed log excerpts (UTC, private identifiers omitted):

```text
18:03:05.663 extension hello (0.5.7)
18:05:58.302 tool apply_patch for CLAUDE ... (ok) ... duration=49187ms
18:06:21.721 tool apply_patch for DEEPSEEK ... (ok) ... duration=70557ms
18:06:21.880 tool apply_patch for GEMINI ... (ok) ... duration=63707ms
18:06:22.044 tool apply_patch for GROK ... (ok) ... duration=64870ms
18:06:22.340 tool apply_patch for KIMI ... (ok) ... duration=43173ms
```

The durations include time waiting for approval. Raw host logs, exact receipts and provider transcript readings are retained in ignored `docs/evidence/0.5.7/`. This strengthens UAB-05 and UAB-08 evidence, but does not complete the unattended scheduler, upload/download discovery or remaining format tools in [TASKS.md](TASKS.md).

## Provider conversations 0.5.6 — November 16, 2026

The seven provider identities each completed real reads, file creation, read-back and a traversal denial in dedicated conversations against the general scratch workspace. Each successful write has exactly one committed recovery receipt and correct agent attribution. Each denied destination remains absent. The raw conversations and receipts are local under `docs/evidence/0.5.6/`.

| Provider surface | Model → host → conversation result | New file and bytes | Navigation/background observation |
|---|---|---|---|
| ChatGPT | Read, create, read-back, traversal denial passed | `files/bridge-056-chatgpt.md` · 42 | New chat and return through the sidebar; one panel, registered/connected while hidden >30 seconds |
| Claude | Read passed; plain-text write exposed a hidden-announcement/typographic-quote defect. A new task using fenced JSON completed create/read-back/denial | `files/bridge-056-claude.md` · 40 | Registered/connected while hidden >30 seconds; navigation retest pending with the adapter fix |
| DeepSeek | All four passed | `files/bridge-056-deepseek.md` · 42 | New chat and sidebar return; one panel, registered/connected while hidden >30 seconds |
| Gemini | All four passed | `files/bridge-056-gemini.md` · 40 | Panel survived New chat; reopened original conversation and stayed connected hidden >30 seconds |
| Grok on X | All four passed | `files/bridge-056-grok-x.md` · 43 | Panel survived New Chat; reopened original conversation and stayed connected hidden >30 seconds |
| Grok.com | Setup connected, but website reported **Free tier limit reached** after a normal reload cleared its initial error screen | No create claimed | Full loop blocked by account quota |
| Qwen | All four passed using plain sentinel lines | `files/bridge-056-qwen.md` · 38 | Panel survived New Chat; reopened original conversation and stayed connected hidden >30 seconds |
| Kimi | All four passed in a fresh chat; the earlier website capacity refusal did not recur | `files/bridge-056-kimi.md` · 38 | New chat and sidebar return; one panel, registered/connected while hidden >30 seconds |

The expected file content was `Bridge 0.5.6 <provider> round trip verified.` (ChatGPT included one trailing newline; Grok used `Grok on X`). Disk bytes and returned hashes matched. The files are ordinary Markdown outputs, not mock provider responses. Native logs distinguish these calls as `tool ... for <AGENT>`, unlike the manual controls' `panel tool ...` lines.

Claude's saved draft was temporarily preserved in a private local copy, then restored with an exact text/hash comparison. It was never sent. Its failure exposed a real adapter issue: the outer assistant wrapper includes a hidden, truncated announcement, while visible prose changes ASCII quotes to typography. The planned fix reads only transcript content and requests fenced JSON; it does not loosen parsing or rewrite quotation marks.

These observations establish registration while hidden, not guaranteed model generation in every background tab. Concurrent conversation routing, per-provider approval delivery and the latest-build matrix remain separate checks. The post-loop doctor verified SQLite and the append-only audit chain.

## Image file controls 0.5.6 — November 16, 2026

| Check | Observed result |
|---|---|
| Workspace suite | 127 passed, 0 failed; one deliberately slow command test ignored in the routine run |
| Image tests | Six integration tests passed: exact crop/rotation pixels, PNG/JPEG/WebP reopening, alpha preservation, EXIF normalization, strict nested arguments, corrupt/oversized inputs, animation/ICC/high-bit-depth refusals, stale hashes, protected paths, symlinks, lock and approval denials |
| Installed selftest | 41 passed, 0 failed in a temporary workspace |
| Build/install | Native/tooling and WASM release exit 0; source and generated manifest both 0.5.6; installed binary SHA-256 and exact native messaging origin verified |
| Loaded build | Chrome extension details, Kimi shadow panel and fresh host hello agree on 0.5.6/protocol 6 |
| Live resize | Extension controls inspected an existing project icon copied into the scratch workspace, then approved a PNG resize from 128×128 to 64×64. The output was reopened and visually inspected |
| Live denial | Denying a separate transform through extension controls returned an approval denial and left its destination absent |
| Draft status | Kimi preserved its draft and correctly reported “Session ready · chat setup needs attention” alongside native bridge connected |
| Doctor | Configuration, manifest, installed binary, SQLite integrity and audit chain passed |
| Limitations | Still-image subset only; no binary replacement/undo, visual understanding or provider attachment claim. macOS requested Desktop access again after rebuilding; persistent consent remains unresolved |

The source `files/bridge-056-source.png` was **1,482 bytes**, SHA-256 `292b200b9b667bbaefb4a55fddc30dfd2cc27d3093fd00eaf954db393a425850`, unchanged after the operation. The output `files/bridge-056-resized.png` was **1,567 bytes**, SHA-256 `870d85321ed8fd647f2288bb768daf7a247b2da5a9d367b01eb6494eb5a9b1bb`, matching the reviewed output hash. This input is the project's existing icon, not a provider-generated image.

Observed host log excerpts (UTC, private identifiers omitted):

```text
17:38:10.904 extension hello (0.5.6)
17:38:10.930 registered Kimi ... extension=Some("0.5.6")
17:39:02.754 panel tool image_info ok ... path=Some("files/bridge-056-source.png")
17:40:03.729 panel tool transform_image ok ... path=Some("files/bridge-056-resized.png")
17:41:12.866 panel tool transform_image error ... path=Some("files/bridge-056-denied.png")
```

The denial was separately recorded as `approval denied: the user denied permission for transform_image`. Raw receipts, build/test logs and browser readings are retained in ignored `docs/evidence/0.5.6/`. These are panel/host/filesystem checks. See [image controls and limitations](IMAGES.md).

## ZIP archive controls 0.5.5 — November 16, 2026

| Check | Observed result |
|---|---|
| Workspace suite | 121 passed, 0 failed; one existing slow command test excluded from routine runs |
| Archive tests | Seven integration tests plus two staging/limit unit tests passed: real source/binary round trip; approval denial; changed sources; locked workspace; path traversal, symlinks and protected areas; case/Unicode collisions; CRC failure after a staged first file; output cleanup; entry/count/depth/expansion limits; unsupported encrypted/ZIP64 inputs |
| Installed selftest | 41 passed, 0 failed in a temporary workspace |
| Build/install | Native/tooling and WASM release exit 0; source, package and generated manifest all 0.5.5, protocol 6; installed binary hash and native manifest verified |
| Loaded build | Chrome extension card, Kimi shadow panel and fresh host handshake agree on 0.5.5 |
| Live creation | Extension controls approved a ZIP containing existing Rust source, Markdown and the earlier Kimi-generated SVG; output was a real 734-byte ZIP |
| Live extraction | Inspection returned three entries and the archive hash. Approval created a new folder; all extracted bytes and hashes matched sources and ZIP entries. No extracted file had executable bits |
| Live denial | Denying a second extraction through extension controls returned an approval denial and left its destination absent |
| Final doctor | Configuration, native manifest, installed binary, SQLite integrity and audit chain passed |
| Limitations | ZIP32/Stored/Deflate subset only; no automatic nested extraction or large-binary undo. Hard process kills can leave a named staging directory for inspection. No new provider model-loop or permanent macOS consent claim |

The live ZIP was `files/bridge-055-project.zip`, SHA-256 `72c818897d99a2fd3decb82c793f6f1a710f2e4b819e4fd5bff361306bc03f50`. Its three sources totaled **397 bytes**: `files/general-proof.rs` (33), `files/bridge-050-kimi.svg` (311), and `files/bridge-053-panel.md` (53). All three were recovered byte-for-byte under `files/bridge-055-unpacked/`. Independent ZIP reading also verified their CRCs.

Observed host log excerpts (UTC, private identifiers omitted):

```text
17:18:15.735 extension hello (0.5.5)
17:18:15.747 registered Kimi ... extension=Some("0.5.5")
17:19:44.128 panel tool create_archive ok ... provider=kimi ... workspace=".../bridge-general-scratch"
17:19:57.661 panel tool list_archive ok ... path=Some("files/bridge-055-project.zip")
17:20:47.782 panel tool extract_archive ok ... provider=kimi ... duration_ms=27531
```

Raw evidence is retained in ignored `docs/evidence/0.5.5/`. See [archive controls and limits](ARCHIVES.md). These are panel/host/filesystem checks; current-build conversation delivery and the full provider matrix remain separate acceptance work.

## Multi-file patch controls 0.5.4 — November 16, 2026

| Check | Observed result |
|---|---|
| Workspace suite | 112 passed, 0 failed; the existing slow command acceptance test remains excluded from routine runs (separately passed on 0.5.3) |
| Patch tests | Eight passed: real multi-file apply/restore and idempotent receipts; approval denial; stale files; conversation ownership; traversal/symlinks; locked/expired previews; research protections; malformed/oversized/binary input; mid-batch path-lock failure and recovery; persisted interruption reconciliation |
| Installed selftest | 41 passed, 0 failed in a temporary workspace |
| Build/install | Native/tooling and WASM release exit 0; source, package and generated manifest all 0.5.4, protocol 6; installed binary and exact manifest origin verified |
| Loaded build | Chrome extension details read 0.5.4, Kimi shadow panel read build/host 0.5.4, fresh host hello and registration recorded 0.5.4 |
| Live preview/apply | Panel preview listed an existing scratch file and one new file. Approval applied both; exact expected bytes and two committed per-file receipts were read from disk/SQLite |
| Live restore | A second approval restored the existing file to its original SHA-256 and moved the new file to recoverable trash with matching bytes; restore receipt completed |
| Live denial | Denying a separate patch through the extension controls returned an approval denial, persisted `denied`, and left its destination absent |
| Doctor after live work | Configuration, manifest, binary, SQLite integrity and hash-chained audit passed |
| Provider loop / macOS consent | No new model-driven loop claimed. Kimi preserved its existing draft. Desktop access prompted again after installing the rebuilt host; stable consent remains unresolved |

The applied files were `files/bridge-053-panel.md` (48 bytes) and `files/bridge-054-batch.md` (41 bytes). Restore recovered the first file's exact original SHA-256, `ef33be9af987af8a3d7487595b3360c08d1221c7f7b70ba2552bb05170c57de9`; the second was absent from its original path and present in trash. These are real **panel → host → filesystem** checks, not a provider conversation integration test.

Observed host log excerpts (UTC, private identifiers omitted):

```text
16:56:30.299 extension hello (0.5.4)
16:56:30.315 registered Kimi ... extension=Some("0.5.4")
16:57:36.703 panel tool preview_patch ok ... provider=kimi ... workspace=".../bridge-general-scratch"
16:58:32.923 panel tool apply_patch ok ... provider=kimi ... duration_ms=27314
16:59:18.005 panel tool restore_patch ok ... provider=kimi ... duration_ms=27055
```

Raw local receipts and logs are retained in ignored `docs/evidence/0.5.4/`. GitHub [run 43910319128](https://github.com/mrfentmen/unified-ai-bridge/actions/runs/43910319128) passed Linux tests, Intel macOS tests and Intel extension packaging for the preceding 0.5.3 commit; it does not establish CI acceptance of this newer patch milestone or physical older-Mac compatibility.

## Bounded jobs and file grants 0.5.3 — November 16, 2026

| Check | Observed result |
|---|---|
| Workspace tests | 104 passed, 0 failed; one intentionally slow acceptance test excluded from routine runs |
| Real long command | Separately ran a real Cargo test in a temporary workspace: 249.6 seconds, exit 0, persisted result/output; exceeds the normal 240-second foreground limit |
| Native restart | A test killed the real Native Messaging host, started a replacement, and recovered the same completed worker receipt without creating a second job |
| Installed selftest | 41 passed, 0 failed, including 35 simultaneous writes across seven agent identities in a temporary workspace |
| Packaging | WASM and native/tooling release builds passed; source/package/generated manifest 0.5.3; exact installed binary and extension origin verified |
| Signing | No valid signing certificate is configured. Desktop access prompted again after rebuilt installs. Persistent permission consent is **not solved** |
| Loaded browser | Chrome extension details and the Kimi shadow panel show 0.5.3; host log has a fresh 0.5.3 handshake and Kimi registration |
| Readiness | Live panel reported actual root, disk space, policy, command grants, versions and audit state; unknown OS permissions remain explicitly unknown |
| Session display | Historical SQLite sessions no longer appear as connected; the live readiness screen showed one current registered session |
| Live command control | Panel approval started a real `/bin/sleep` job. A bounded caffeinate assertion identified its worker PID; Stop recorded `cancelled`, the command PID disappeared, and that assertion disappeared |
| Local report | Panel saved a JSON job receipt under application support `reports/jobs/`; the receipt reported cancellation, not success |
| Live scoped file grant | Panel created `files/bridge-053-panel.md`, then read back its exact content. A write to `notes/bridge-053-denied.md` returned a denial and created no file; revoking the grant also denied a new file inside `files/` |
| File grant tests | Allow case, stale content, wrong path/tool/session/workspace, research protection, symlink, budgets, expiry, revocation, and concurrent budget reservation checks pass |
| Kimi in-conversation loop | **Blocked on this build**: website refused the verification message with “Too many people are chatting with Kimi.” Panel/host checks are not counted as a provider conversation loop |
| Other providers / Intel | Not rerun in this milestone; physical Intel hardware remains unverified |

The first selftest exposed grant-lock contention on ordinary concurrent writes. It was corrected with bounded serialization and a concurrent budget test before the final installed selftest passed. The initial job dropdown was replaced with explicit job buttons and a visible selected ID after live selection ambiguity. Raw local logs and receipts are retained under ignored `docs/evidence/0.5.3/`.

Observed host log excerpts (UTC):

```text
16:35:14.722 extension hello (0.5.3)
16:35:14.741 registered Kimi tab 163427767 ... extension=Some("0.5.3")
16:35:48.106 panel tool run_command ok ... provider=kimi ... workspace=".../bridge-general-scratch"
16:36:23.709 panel job control KeepAwake { job_id: "f7c412d88f4f233b2a8995044605798d", seconds: 3600 }
16:36:49.796 panel job control Cancel { job_id: "f7c412d88f4f233b2a8995044605798d" }
16:37:02.122 panel job control ExportReport { job_id: "f7c412d88f4f233b2a8995044605798d" }
```

This implements command jobs and scoped file operations, not a complete overnight task scheduler or a claim of unattended access to locked/login-limited websites. See [current controls and limits](OVERNIGHT.md).


Raw transcripts, session IDs, conversation URLs, machine paths, audit exports, and build logs are retained locally and excluded from the public repository. This document summarizes those observations without publishing the chats.

## Browser-file import 0.5.0 — November 16, 2026

| Check | Recorded result |
| --- | --- |
| Workspace tests | 91 passed, 0 failed: 32 core, 2 extension metadata, 18 native unit, 6 artifact integration, 13 filesystem, 1 MCP subprocess, 8 Native Messaging, 9 workspace/command, 2 tooling |
| Release packaging | WASM and native/tooling exit 0; source/package/generated manifest all 0.5.0; installed binary hash and exact native manifest verified |
| Installed self-test | 41 passed, 0 failed, using a temporary workspace |
| Loaded build | Chrome details, Kimi shadow panel, and fresh host hello all 0.5.0; protocol 5 |
| Real browser file | Kimi generated `bridge-050-test.svg`; its visible Download control saved a 311-byte SVG in Chrome |
| Explicit sharing | Extension controls selected that Chrome download and Kimi session; the host registered an opaque file ID and recorded the grant |
| Denied import | Manual import request denied from the in-page panel; destination remained absent |
| Model-driven import | Kimi called `list_artifacts`, then `import_artifact`; approval in the extension controls created `files/bridge-050-kimi.svg` in the general scratch workspace |
| In-conversation verification | The real import result returned to Kimi; its following `read_file` returned the SVG contents and matching hash |
| Source preservation | Download and workspace copy both remained present, byte-identical |
| Final selector/revocation check | Rebuilt and reloaded after the UI fix; selected file displayed correctly, then changed to `revoked` after Revoke sharing |
| Doctor after import/revocation | Configuration, manifest, binary, SQLite and audit passed; 362 matching database/audit-file entries |

Both files were 311 bytes, with SHA-256:

```text
11d19bfa6721f67f8a00173ead6549853e158549f04601c01ed48a8eaf0e8728
```

Observed host lines, with private identifiers omitted:

```text
14:54:22.565Z extension hello (0.5.0)
15:03:54.153Z artifact control result=ok
15:05:01.508Z panel tool import_artifact error (approval denied)
15:05:10.019Z tool list_artifacts for KIMI finished in 14ms (ok)
15:05:34.175Z tool import_artifact for KIMI finished in 12188ms (ok)
15:05:45.004Z tool read_file for KIMI finished in 1ms (ok)
```

The artifact regression suite exercises binary bytes above the existing text limit, explicit approval and denial, changed sources/inodes, source and destination symlinks, hard links, private-file exclusions, traversal, collisions, workspace locking during approval, revoked and expired grants, cross-session/workspace isolation, interrupted receipts after restart, research protections and duplicate import refusal. A Native Messaging test also imports real binary bytes through the compiled host's framed protocol.

This proves the specified Kimi download/import flow. It does not establish automatic provider-artifact discovery, uploads, large-binary recovery, archive extraction, media understanding, other providers' download flows, or physical Intel Mac compatibility. Kimi briefly reported its existing message-ID selector warning while the final answer was streaming; the import and read results still reached the conversation. The final UI-only follow-up was rebuilt, reloaded and checked for correct selection/revocation display; the native binary hash remained unchanged.

## Patch 0.4.7 — November 16, 2026

[Tagged release CI](https://github.com/mrfentmen/unified-ai-bridge/actions/runs/43880560247) passed on commit `6ef4e3b234726a3b17b12c8ceb01849526f99eb7`. Ubuntu 24.04 and the macOS 15 Intel runner each passed all **81 workspace tests and 41 self-tests**. The macOS packaging job built the WASM extension and release host, confirmed version 0.4.7, installed into isolated directories, and verified the binary hash and exact native manifest origin/path. The extension artifact uploaded successfully. The gated publication job then published [v0.4.7](https://github.com/mrfentmen/unified-ai-bridge/releases/tag/v0.4.7).

Hosted Intel build/test evidence does not establish compatibility with a physical 2015 MacBook Pro or older macOS versions.

| Check | Recorded result |
| --- | --- |
| Workspace tests | 81 passed, 0 failed: 32 core, 17 native unit, 13 filesystem, 1 real MCP subprocess, 7 Native Messaging, 9 workspace/command, 2 tooling |
| Native/tooling and WASM release builds | Exit 0; source/package/generated manifest all 0.4.7 |
| Installed-host self-test | 41 passed, 0 failed, in a temporary workspace |
| Loaded extension | Chrome details, Kimi shadow panel and fresh host hello all 0.4.7; protocol 4 |
| Doctor after UI tests | Configuration, binary, manifest, database and audit passed; 335 matching DB/audit-file entries at capture |
| Original move reproduction | Newly created destination preserved, source preserved, terminal `file_changed` result |
| Original Strict research-folder reproduction | Approval requested before creation; denying left the folder absent |

The eight added regression tests cover real approval waits, changed source contents, destination creation, symlink swaps, updated policy, all three profiles, immutable research data, cross-agent denials, concurrent no-replace moves and dangling destination symlinks. They do not establish an OS sandbox against arbitrary local processes.

Actual installed-host output from the isolated reproduction included:

```text
hostVersion: 0.4.7
researchStrictCreateFolder: approvalRequested=true, folderExists=false
moveDestinationCreatedDuringApproval: code=file_changed
destinationContent: independent new destination content\n
sourceStillExists=true, overwroteNewDestination=false
```

### Live approval and recovery controls

These were manual tool-runner calls bound to Kimi's live session, not model-generated calls. The side-panel page's Preview/Apply controls created a disposable file; the in-page panel, extension controls and actual toolbar popup handled approvals through Chrome Native Messaging.

| Action | Observed result |
| --- | --- |
| Preview → Apply | Actual 54-byte file created; committed change record and host attribution |
| Delete → in-page Deny | `approval_denied`; file hash unchanged |
| Delete → extension controls Approve once | File absent from original path; trash copy hash matched |
| Recovery history → Request restore → toolbar popup Deny | `approval_denied`; original path remained absent |
| Request restore → in-page Approve once | File restored with exact original bytes; committed recovery record |

The actual saved path was `iles/ui-047-recovery.md` in the general scratch workspace (the automation dropped the initial `f` while typing the path). All later checks used that actual path. The content was `Real UI approval and recovery proof for version 0.4.7.` without a newline; its SHA-256 before deletion, in trash and after restore was:

```text
b40f335ac56e24b64c120d1297b4a53a5037510c9448a15d3dd500f83b0e78b7
```

Host observations, with private identifiers omitted:

```text
13:40:08.342Z extension hello (0.4.7)
13:48:21.566Z panel tool delete_file error agent=KIMI (approval denied)
13:49:01.709Z panel tool delete_file ok agent=KIMI
13:52:15.840Z panel tool restore_change error agent=KIMI (approval denied)
13:53:00.761Z panel tool restore_change ok agent=KIMI
```

These observations cover the listed actions, not every approval button in every layout or model-driven delivery following an approval. All-provider concurrency and interruption recovery still need live acceptance.

## Earlier build and host checks — 0.4.6

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

For approval testing, use a disposable file and its fresh hash. Request deletion, deny it once, and confirm the file remains. Request it again, approve it, confirm it moves to workspace trash, then exercise denied and approved restore. Record which surface handled each decision. The 0.4.7 manual run above does not replace model-driven result-delivery tests.

## Still unverified

- Physical Intel Mac compatibility and the minimum macOS version.
- Current-build acceptance of every older provider, all-provider concurrency, and interruption recovery across all sites.
- Every live approval/recovery control and third-party MCP server.
- Fully unfocused Qwen tool execution, long virtualized transcripts, and alternate provider modes/layouts.
- A permanent solution to macOS folder-access prompts after unsigned host rebuilds.
