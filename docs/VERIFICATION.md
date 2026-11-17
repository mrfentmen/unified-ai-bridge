# Verification record

## Provider delivery fixes 0.5.25 — November 17, 2026

This milestone fixes observed website adapter failures while preserving strict tool parsing, host-issued identity and workspace policy. Tests used the existing **general / standard** scratch workspace, with **16 calls/task and 4/response**, YOLO off and network off. No archaeology files or OS permission settings were changed. The current machine is Apple Silicon / macOS 27.0.1; physical older Intel acceptance remains open.

DeepSeek now scans its final-answer container and waits for the observed per-turn **Read aloud** completion control. Gemini and Grok.com read actual user-message text rather than hidden accessibility headings. Gemini's shorter catalog keeps every tool, argument summary, protocol and general policy rule; its complete draft is checked before submission. The panel is now at the top right with a bounded scroll area, leaving the bottom composer available. Saved result reconciliation waits for empty message nodes to render and recognizes a complete delayed acknowledgement without submitting again.

The following are **real website-model calls**. Each sequence read `files/budget-0524.txt`, created the named file, read it back and received a traversal denial. Independent disk and read-only SQLite checks found **one successful write receipt and one committed recovery record per file**, with correct agent/workspace attribution. All seven attempted escape files remained absent. New files have no prior-content checkpoint; their committed change records provide recovery evidence.

| Provider | File / exact bytes (no newline) | Actual outcome and intervention |
|---|---|---|
| DeepSeek | `files/deepseek-0525.txt` · 25 · `DeepSeek 0.5.25 verified.` | Read **09:57:12.037Z**, create **09:57:17.313Z**, read-back **09:57:22.772Z**, denial **09:57:28.239Z**; results and stopped summary visible. Earlier 0.5.24 write never executed |
| Gemini | `files/gemini-0525.txt` · 23 · `Gemini 0.5.25 verified.` | Read **10:00:09.838Z**, create **10:00:30.287Z**, read-back **10:00:44.344Z**, denial **10:00:52.491Z**. Deliberate setup reinjection submitted and acknowledged automatically after the prompt-length fix |
| ChatGPT | `files/chatgpt-0525.txt` · 24 · `ChatGPT 0.5.25 verified.` | Created **10:04:03.122Z**, then the real context-full gate blocked delivery. Reload preserved the saved result; **Continue in fresh chat** prepared/bound at **10:06:00.094Z / 10:06:04.126Z** with **14 remaining calls**. Read-back **10:06:08.392Z**, denial and final summary arrived in the destination. Recovered, not uninterrupted |
| Claude | `files/claude-0525.txt` · 23 · `Claude 0.5.25 verified.` | Fresh automatic setup, read **10:06:56.632Z**, create **10:07:03.049Z**, read-back **10:07:10.603Z**, denial **10:07:15.883Z**. Results and final summary visible; transient thinking warning cleared |
| Grok.com | `files/grok-0525.txt` · 21 · `Grok 0.5.25 verified.` | Read **10:08:39.501Z**, create **10:08:58.534Z**; false acknowledgement error required the Grok fix/reload. Read-back **10:10:48.523Z** and denial **10:10:57.527Z** returned. Then **Free tier limit reached** blocked a clean retest. Recovered, not uninterrupted; `grok-0525-final.txt` absent |
| Qwen | `files/qwen-0525.txt` · 21 · `Qwen 0.5.25 verified.` | Website Send was needed once for initial setup. Read **10:14:41.118Z**, create **10:14:56.651Z**, read-back **10:17:21.947Z**, denial **10:17:30.919Z**. Accidental tab closure/Playwright disconnect required reopening the same chat; foreground rendering was needed. Results and final summary visible; not an unattended pass |
| Kimi | `files/kimi-0525.txt` · 21 · `Kimi 0.5.25 verified.` | Automatic setup passed. After sending the explicit task with the normal website button: read **10:20:18.457Z**, create **10:20:28.350Z**, read-back **10:20:37.931Z**, denial **10:20:48.762Z**. Results, stopped summary and empty composer verified; disk hash independently matched |

File SHA256 values, in table order:

```text
deepseek 287942a8fc98116ac06faed377c288252e87d6ecbd4c1893cd80cacf4feb045e
gemini   20e22da2e776daf8a44877bf15b34a3c3be67953a79f47a1aaabd035d6277bac
chatgpt  34c4c77fdf2c67616049feb87fcc4d3435b492db1c236f86703fa40406741bcd
claude   cf5da7eca3fbf3cb26cf8edba56d8174a59d3f8347e0aeec424d38a5bcb3f8fa
grok     db85e53b6bca961274d779369c808cfb86849260f0f03d423ccab5e076e776ae
qwen     ded62135e91a73297ed3f3caab1ffdd1b30ab911e04fa7818fc16f8050bfb4e1
kimi     74b6bc3ad3ef58218f31086e066a7fe65369158d4255f3174c5774a012c106c0
```

Development failures are retained: Gemini initially truncated a setup draft from **33,368** to **31,778** non-newline characters; complete-draft validation refused submission. The revised setup was about **26,571** characters and delivered. Grok's hidden “You” heading caused an incorrect stale-result error before the adapter fix. Qwen's new empty user-message node caused the same false invalidation before the delayed-render reconciliation change. The latter has tests for empty transcripts, full acknowledgements, changed user/assistant turns, wrong conversations, partial/shared-prefix results and no automatic uncertain resubmission. Its initial test compile failed on a temporary string lifetime; the assertion now binds that string explicitly.

Several 0.5.25 bundles were built during this work. The table records each actual attempt; it is **not an uninterrupted all-provider acceptance run against one final WASM hash**. Raw transcripts, receipts, screenshots and logs remain private in ignored `docs/evidence/0.5.25/`.

Final package validation: pinned native/tooling/WASM builds exited 0; source and emitted manifest both say **0.5.25**, confirmed by the Chrome card, shadow panel and new hello at **10:38:06.119Z**. The final full suite passed **250 tests, 0 failed, 1 ignored**, with documentation tests passing. **48 targeted core/extension tests** passed before deployment. **41 installed self-tests** passed against the same native SHA256. Doctor passed config, native manifest/binary, SQLite and the **1,022 matching DB/file audit entries** at its recorded snapshot. The previously verified long ignored command test was not rerun.

Final native SHA256: `cf0b23d701586f66c37793a8f5b8d61bbfc2cca3ee13f6ef571b65e9f320be62`. Final WASM SHA256: `de23acf8f994dd1c38c8ff29b16449af99c60be1fe94f6a4de65ac61e329b817`.

**Qwen code-editor recovery, final bundle:** reproduced a stalled real fenced `bridge-tool` request. The bridge now sends an explicit `adapter_incomplete_render`, `executed:false` notice and requests the full plain-text sentinel form, at most twice per task. It preserves the original task and persists the correction reservation; no editor prefix is executed. After reload, the original stalled read recovered automatically at **10:38:25.833Z**. A second explicit task emitted a fenced create request, received one automatic formatting notice, re-emitted the complete request and created `files/qwen-0525-repaired.txt` at **10:39:10.547Z**. Read-back completed at **10:39:22.019Z** and both results plus the stopped summary appeared in the conversation. Independent disk/SQLite checks found exactly **30 bytes**, `Qwen fenced request recovered.` with no newline, SHA256 `5b6b85b9092dd96552165bbe1cf3838d1b277703e9b04c0aef801a2b7254bd54`, one successful write, one committed change and the **same original task ID** on write and read.

A third Qwen task exercised a fenced traversal request. While hidden for over 30 seconds, Qwen created an empty user node and rendered only a partial answer. The saved formatting notice remained pending instead of being discarded or resent; bringing the tab into view acknowledged it and resumed parsing. The host denied the complete request at **10:40:48.104Z**; denial and final summary returned, and `/Users/test/uab-0525-qwen-repair-escape.txt` remained absent. Exactly three formatting notices were observed across these three separate tasks. New chat → Back then restored one connected panel and an empty composer, with no duplicate file change. Qwen still needs foreground rendering; this verifies recovery from that state, not unattended background execution. An earlier read-only retry showed the website's “Unknown error”; the original request later completed after focus, and the redundant unsent test draft was cleared without submission.

**Gemini follow-up:** fresh automatic setup completed without manually sending its draft. A real read of `files/gemini-0525.txt` at **10:31:41.610Z** returned the exact content; hidden rendering delayed the visible final summary until focus. Reload and New chat → Back on the final bundle retained one panel and re-registered at **10:40:15.088Z**. Setup did not execute tools. These final lifecycle checks cover Gemini and Qwen; they do not replace the earlier all-provider matrix.

**Still open:** a clean Grok retest after quota reset, Grok-on-X acceptance on this version, fully unattended Qwen setup/rendering, remaining popup/docked-panel approval and in-flight workspace tests, approved uploads and generated-artifact discovery, preview/application integrations, complete task reports/scheduling, stable signing/TCC and physical older Intel hardware. Grok's observed quota notice was not identified by the panel's current narrow notice selector; no account limit was bypassed.

## Workspace task budgets and setup delivery 0.5.24 — November 17, 2026

Installed **0.5.24 / protocol 8** in the existing general scratch workspace. Source, emitted manifest, Chrome extension card, in-page panel and native hello agree. This is a shared control/reliability milestone for any project type, not complete unattended game or movie production.

| Check | Actual evidence |
|---|---|
| Build and installation | Pinned Rust/WASM/native/tooling builds exited 0. Final host hello **09:20:03.764Z**; extension card and panel showed 0.5.24 / protocol 8 |
| Automated checks | Full workspace suite **245 passed, 0 failed, 1 ignored**; documentation tests passed. Final source was rerun after the setup-delivery fix; its targeted **43 core/extension tests** also passed. The separately verified long command test was not rerun |
| Installed host | **41 self-tests passed** in temporary folders. Doctor passed config, registration, installed binary, SQLite and audit validation |
| Budget boundaries | Tests cover 1–256 task / 1–32 response bounds, unknown fields, wrong workspace, persistence, reset, frozen existing tasks, lowering during handoff and no replenishment after host restart |
| Framed host | Real write/read with a two-call ceiling; extra response slot and exhausted task denied. Raising limits to 64 and restarting preserved exhaustion. A genuinely new task received 64; only one write was recorded |
| Long-task receipts | A 150-call native test with a 256-call allowance handed off **106 remaining calls** and the latest **24** receipts, numbered 126–149; earlier pages did not displace recent outcomes |
| Live budget controls | Opened the real extension controls and saved **2 calls/task, 1/response**. Later saved 4/4 and 64/4; the task panel kept the exhausted original at **0 calls left**. Reset restored installation defaults **16/4** |
| Live website loop | ChatGPT wrote `files/budget-0524.txt` at **09:25:02.662Z**, read it at **09:25:08.303Z**, and received **task_budget_exceeded** at **09:25:14.110Z**. The denial returned into the conversation and the model stopped |
| File evidence | Exactly **23 bytes**, `Budget limits verified.` with no newline; SHA256 `b0106a504e6f488682596239cea2d1066be7551c59c2c2c699e5dcc0b8911ee0`. Workspace `/Users/test/bridge-acceptance-0521`, one successful write receipt |
| New-task isolation | A new explicit `workspace_info` task received the then-current **4/4** ceiling and reported it in the conversation. SQLite retained the earlier task's original **2/1** ceiling; changing settings did not rewrite it |
| Reload/setup | Reload returned to connected without replaying the write. Fresh-chat setup and a deliberate reinjection in the same conversation both acknowledged successfully on the corrected sender; setup executed no tools |

Initial test failures and live limitations are retained. One new isolation test incorrectly reopened an App expecting its supplied folder to override the durable active-workspace registry; using the real create/select workflow fixed the test. Live testing found setup acknowledgement did not tolerate inline backticks being stripped and its error path allowed repeated automatic setup attempts. Added normalized acknowledgement and persisted unconfirmed-submission state. The old verification chat reached a real context-full gate; it was left stopped and the acceptance task ran in a fresh chat. Do not count that old chat as an uninterrupted pass. An accessibility automation attempt entered 4 instead of the intended 64; host logs exposed it, and the later corrected 64 save was verified separately.

Native SHA256: `68ffb7ab98d7a21822bef424770f9f1ed656c79fbc7785cac2747478c64643c9`. Final WASM SHA256: `e3bfda86ac7ba71fbf4c190eae7156e778acba226579bbce1ac3078ece2deacb`. Private raw evidence is in ignored `docs/evidence/0.5.24/`. The prior 0.5.23 commit `aa5e175` passed Linux tests, Intel macOS tests and packaging in [CI run 44051551412](https://github.com/mrfentmen/unified-ai-bridge/actions/runs/44051551412); that is not CI evidence for 0.5.24.

**Still open:** full provider acceptance on this build, hands-off recovery on other providers, actual docked-side-panel/popup-specific acceptance, image/file exchange, broader app/game/media controls, whole-task scheduling/reports, stable macOS permission persistence and physical older Intel testing. This pass exercised the side-panel route in a Chrome tab through accessibility controls, not a docked panel. No archaeology files were modified; YOLO and network access remained off.

Follow-up provider checks on **0.5.24** found two adapter defects. DeepSeek read the real budget file at **09:35:34.576Z**, but stable-looking pauses in an unfinished reply were marked malformed and consumed before the final complete JSON appeared. A later DOM inspection confirmed the complete `write_document` block; no create-file receipt was produced. Its loading indicator disappears before streaming finishes. The final-answer container and per-turn **Read aloud** control were observed separately from its reasoning and during actual streaming. Gemini completed read (**09:40:31.091Z**), create (**09:40:57.816Z**), read-back (**09:41:15.912Z**) and traversal denial (**09:41:33.911Z**). `files/gemini-0524.txt` contains `Gemini 0.5.24 verified.` with no newline (23 bytes), SHA256 `032b80dd88c8c26af0e79f46e782f3ef4a6ff8eb57bb052c89f49681f9f2df6e`. The escape file remained absent. Gemini required dismissing a promotional modal and manually submitting the bridge's setup draft. Its hidden truncated “You said” announcement then prevented acknowledgement of visibly delivered results, leaving a false stale-turn error. These are failures of unattended acceptance, not passes. Fixes and retests belong to the next build.

## ChatGPT continuation and durable delivery 0.5.23 — November 17, 2026

Installed build **0.5.23 / protocol 7**, verified in Chrome's extension card, the in-page shadow panel, generated manifest and native handshake. The current machine remains macOS 27.0.1 / Apple Silicon; a physical older Intel Mac remains unverified. Tests used the existing general scratch workspace. The archaeology project was not touched.

| Check | Actual evidence |
|---|---|
| Build/install | Pinned Cargo, rustc and rustdoc; extension WASM plus native/tooling release builds exited 0. Source, package and loaded Chrome build matched 0.5.23; host hello at **08:43:23.779Z** |
| Automated suite | **240 passed, 0 failed, 1 ignored** across the workspace; all three documentation-test suites passed. The previously verified long command test was not rerun |
| Packaged host | **41 self-tests passed** in temporary folders. Doctor passed configuration, native registration/binary, SQLite and matching DB/file audit chains (**902 entries** at the final diagnostic snapshot) |
| Real framed transfer | A real Native Messaging child wrote a file, prepared a handoff, restarted, bound its unused budget to a new conversation, read the file, and refused an extra call. One successful write receipt, no forbidden file |
| Persisted delivery / draft refusal | A genuine ChatGPT read was held behind a known test draft. Reload preserved the draft and queued result. Continue refused while the draft remained. Clearing only that test draft allowed handoff |
| First live transfer | The new ChatGPT conversation created `files/handoff-0523.gd` and read it back. One successful write receipt; **75 bytes**, SHA256 `dfc8953a4491d1e2cb0878f4bd224bbabc6f67dfaf31f67bf889b2c967a1a8b0`. The initial adapter stalled on a replaced composer; after the sender fix and reload, the prepared handoff recovered and finished. This first run is recovery evidence, not an uninterrupted pass |
| Uninterrupted live transfer | On the final sender build, preparation at **08:46:21Z**, fresh-chat registration/binding at **08:46:27Z**, write at **08:46:31.524Z**, read-back at **08:46:38.497Z**. `files/handoff-0523-final.txt` contains exactly `Fresh chat handoff completed.` — **29 bytes**, SHA256 `65b359a690ebcd767fabb478e215d439ce072c7ddc52be1518df724b39474f81`. Both result blocks and the model's completion reply were visible in the destination conversation; no reload or second task prompt was required |
| Automatic context-full recovery | Enabled Auto handoff on the real previously full verification chat after clearing its known old bridge-result draft. The visible website banner triggered preparation at **08:48:36.468Z** and binding at **08:48:40.416Z**, without clicking Continue. Its original 16-call task had used one call; the target ceiling was **15**, not a new 16 |
| Scoped handle denial after transfer | The model tried the old conversation's `apply_patch` handle. The host returned `permission_denied` for conversation ownership, delivered that result, and the model stopped. `files/yolo-0522-denied.rs` remains absent. This verifies ownership denial, not an Approve/Deny-button flow or a successful transferred patch |
| Safety tests | Cross-tab/workspace claims, stale/expired/cancelled/stopped tickets, old-source admission, exhausted budgets, host restart, receipt lineage, saved draft/uncertain submission, context-versus-quota classification and no automatic resubmission are covered |

Both successful handoffs retained the actual unused budget and stopped admission in the source task. The transcript's tool blocks came from the website model. No mocked provider response or manually injected assistant block was used. The GDScript artifact demonstrates source-file creation, not execution in Godot; it omitted the originally requested final newline, which is a model-output discrepancy rather than an exact-content acceptance pass. The later text-file test passed its exact-content requirement.

Native SHA256: `977cfe3b68ac3515c01eb95152417cff8074f350040ee9a7af5b9a91ba78cb11`. WASM SHA256: `a36e3d7433478319f656ca747051a296850525771e2d8a172137fe6894b8e71b`.

An initial full-suite command passed its test binaries but selected Homebrew rustdoc, producing compiler-version errors in documentation tests. Pinning RUSTDOC alongside CARGO/RUSTC fixed that invocation. Live testing also found an empty stored instruction fingerprint incorrectly invalidated a task on reload and that a detached composer could stall send acknowledgement. Both were fixed and regression-tested before the uninterrupted pass.

**Still open:** fresh-chat recovery on other providers; the full provider matrix on 0.5.23; Qwen candidate/foreground handling; Gemini setup drafts; larger task-budget controls/scheduling; automatic generated-file collection and approved uploads; application/game-engine/media integration acceptance; docked-panel/popup and remaining tool UI matrices; stable signing/OS consent; physical Intel hardware. No valid code-signing identity is installed (`0 valid identities found`); no certificate or TCC setting was changed. This release does not establish a fully hands-off general desktop agent. [Continuation scope](CONTINUATION.md).

Raw transcripts, test logs, package hashes and host lines are private under ignored `docs/evidence/0.5.23/`.

These are recorded results, not a guarantee of every environment. The previous installed build was **0.5.22**, protocol **6**, on **macOS 27.0.1, Apple Silicon, Chrome 157**, with Rust **1.99.1**. The YOLO and hidden-tab session-recovery checks below passed. Website context/quota limits and the remaining acceptance rows are still open.

## YOLO and session recovery 0.5.22 — November 17, 2026

Source, generated manifest, Chrome extension card, ChatGPT panel and native handshake matched **0.5.22 / protocol 6**. Testing used the CLI Playwright MCP extension connection, with macOS accessibility for the extension's protected controls. The active general workspace remained `/Users/test/bridge-acceptance-0521`; the archaeology project was untouched. YOLO is **off** at the end.

| Check | Observed result |
|---|---|
| Build/install | Pinned rustup WASM/native/tooling release builds succeeded; version and native-manifest/hash assertions passed |
| Automated suite | **230 passed, 0 failed, 1 ignored**. The previously verified long command test was not rerun. After adding the final batch-approval task-state check, **25 targeted native messaging, task and YOLO tests passed** |
| YOLO policy | Eight dedicated tests cover real writes/patches, audit, expiry, Stop/replacement, two concurrent workers competing for one approval, workspace/restart isolation, narrowed session grants, changed hashes, private/traversal/symlink/research denials, command allowlist and strict control decoding |
| Framed host | A real host process accepted the panel control, wrote a file in Strict mode, returned the cached result on repetition, and returned that same receipt after restart with YOLO off |
| Live enable | Real extension controls set **one hour / two approvals**; the provider panel displayed `YOLO ON` and time remaining |
| ChatGPT | `preview_patch` → `apply_patch` → `read_file` created `files/yolo-0522.rs` without an approval click. Actual result blocks and exact read-back appeared in the conversation. Audit sequence **815** is `approval.yolo:apply_patch` |
| Stop / denial | Stop appeared in the host log at **05:51:10.592Z**. A new patch through the extension's direct runner showed a normal approval; clicking Deny created no `files/yolo-0522-panel-denied.rs`. Its receipt and audit record show the denial |
| Hidden-tab recovery | With no active tool request/job and **all ten test tabs hidden**, terminating the idle host at **05:56:27.132Z** produced a new hello at **05:56:27.222Z** and ten fresh registrations by **05:56:27.311Z**. No reload/focus was used. All panel IDs changed and native state returned to connected; YOLO was off |
| No replay | Across that restart, the **256 stored response receipts** (254 complete, 2 rejected) and **52 file-change records** were identical. These are installation-wide historical counts, not 256 new passes. The Rust proof hash was unchanged and the queued preview result survived |
| Final native amendment | Final review added the task Pause/Stop check to automatic batch approvals. The final native binary was rebuilt and installed; the WASM hash stayed identical to the loaded artifact. Restart at **06:02:08.530Z** loaded it, followed by ten registrations by **06:02:08.652Z**. The direct extension runner then created `files/yolo-0522-final.txt` through another audited automatic patch, and Stop was applied again. That direct runner used a Grok session's permissions; it is **not** evidence of a new Grok website round trip |
| Installed diagnostics | **41 self-tests passed** in temporary folders; final doctor passed config, manifest, binary, SQLite and matching DB/file audit chains |

The ChatGPT file contains exactly `pub fn bridge_yolo_proof() -> u32 { 522 }` plus newline: **42 bytes**, SHA256 `0d59d1b252640c733cfeba0dbf0a16152834d4904c7c12ebc94ec0ae4efb40ba`. Native SHA256 `2d9d086b0f736248f728f0943b4212e27b2095722759348094a2aa696b040533`; WASM SHA256 `bca4a9e163331267b4b0a424b5b4f48054e19ebc4f03b11cd1ee42dbdb2da326`.

The follow-up ChatGPT denial task hit **Context window full** after preview. Its result stayed in the composer; `apply_patch` was never emitted and `files/yolo-0522-denied.rs` is absent. The panel reported the generic “website did not accept the message” error, then showed the queued result after session recovery. This is a remaining delivery/diagnostics defect, not a successful in-conversation denial test. The direct runner was used for the separate real Stop/deny check. Host recovery establishes registration and preservation of pending state, not automatic recovery from a website context limit.

During development, a dedicated test initially expected `conflict`; the actual correct public denial code is `file_changed`. The test assertion was corrected without weakening the refusal. An early workspace run encountered a stale lockfile after the version edit; refreshing version metadata and rerunning produced the 230-test result above. A stale standalone tooling executable pointed at an old temporary checkout; that build was stopped before install, and the supported `cargo bridge-deploy` command rebuilt tooling from the current checkout. These early runs are not passing acceptance evidence.

Raw UI readings, transcripts, hashes, native logs and build/test results remain private in ignored `docs/evidence/0.5.22/`. No all-provider tool matrix was rerun on this version. Grok quotas, Gemini setup drafts, Qwen candidate/foreground behavior, full concurrency, docked-panel/popup lifecycle, other tool UI tests, stable signing and a physical older Intel Mac remain open. See [YOLO scope](OVERNIGHT.md#yolo-mode-0522) and [remaining tasks](TASKS.md).

## Live 0.5.21 acceptance — November 16–17, 2026

Tested source commit `c6ef8466e101ea86cf6747f19f2006da13f41144` through the CLI's pinned Playwright MCP extension connection and macOS accessibility for Chrome's protected extension UI. No desktop Browser connector was required. The normal installed host and unpacked Chrome extension were upgraded from 0.5.7 to 0.5.21. Source, packaged manifest, `chrome://extensions` card, live panel footer and host handshake all reported **0.5.21 / protocol 6**. Native/WASM hashes are the ones in the 0.5.21 package record below.

The new unsigned host encountered a Desktop-access prompt. No OS permission or signing identity was changed. Testing moved through audited `CreateWorkspace` / `SetActiveWorkspace` operations to `/Users/test/bridge-acceptance-0521`, a new **general / standard** workspace outside Desktop. Network remained disabled and no commands were granted there. The archaeology project was not used or modified. The previous scratch workspace and its configuration remain available; switching back can encounter the unresolved Desktop prompt.

| Build/host check | Actual result |
|---|---|
| Routine workspace tests | **220 passed, 0 failed, 1 ignored** |
| Separately executed ignored command test | **1 passed**, 251.50 seconds elapsed; its real Cargo test took 245.01 seconds |
| Packaged self-test | **41 passed, 0 failed**, including real temporary writes and denials |
| Release package | Native/tooling/WASM release builds exited 0; pinned rustup toolchain and package hashes verified before installation |
| Native handshake in Chrome | `2026-11-17T04:04:40.634Z [info] extension hello (0.5.21)` |
| CI | [0.5.19](https://github.com/mrfentmen/unified-ai-bridge/actions/runs/43964530182), [0.5.20](https://github.com/mrfentmen/unified-ai-bridge/actions/runs/43966311041) and [0.5.21](https://github.com/mrfentmen/unified-ai-bridge/actions/runs/43968525423): Linux tests, Intel macOS tests and packaging succeeded; release jobs skipped |
| Installed doctor | Configuration, manifest/origin, installed binary/hash and SQLite passed; **799 database entries and 799 file entries** matched in the final audit verification. Signing remains ad hoc; physical Intel hardware is unverified |

Each full provider loop read the real README copy, created `files/bridge-0521-<provider>.md`, read it back, received `permission_denied` for a `../` write, previewed and approved a patch creating `files/bridge-0521-<provider>-reviewed.md`, and read that file back in the same conversation. These are real model-emitted calls, not injected mock responses. Each successful Markdown creation has exactly one committed recovery record with the correct agent and hash; a new file has no previous-content checkpoint.

| Provider | 0.5.21 file loop | Navigation / background / limits |
|---|---|---|
| ChatGPT | **Pass**, seven calls including denial and approved patch | New-chat/return retained one panel; registered while hidden for more than 60 seconds. A later concurrency read hit the site's **Context window full** gate during result delivery; its result draft was preserved. A fresh conversation passed the response-budget test |
| Claude | **Pass**, same seven calls | New-chat/history return and hidden registration passed. Brief adapter warnings occurred during thinking |
| DeepSeek | **Pass**, same seven calls | New-chat/back required returning through the known history entry; one connected panel remained while hidden |
| Gemini | **Pass**, same seven calls | Panel/session survived navigation and backgrounding, but setup retained a generated instruction draft and needed attention. Automatic setup recovery is **not passed** |
| Grok.com | **Blocked** | One ordinary reload recovered an initial website error; the site then reported **Free tier limit reached**. No tools executed |
| Grok on X | **Partial**: read and create succeeded, result returned | Site reached its **20 questions per 24 hours** limit before read-back, denial and approved-patch steps. No limit bypass or account linking |
| Qwen | **Pass**, same seven calls | Identical candidate replies required an explicit choice before execution. After new-chat navigation, the original proof conversation was reopened at its observed URL. Hidden registration passed; message scanning can still require foreground rendering |
| Kimi | **Pass**, same seven calls | New-chat/back and hidden registration passed. A temporary message-ID warning appeared during thinking and cleared after completion |

The seven basic files and six reviewed files have matching expected bytes: **13 files, 13 committed recovery records, no duplicate writes**. All six outside-root denial paths are absent. The five copied source assets and the 13 provider files retained their recorded hashes throughout the later UI checks. Research dataset hashes were not re-read in this pass because that project was excluded.

Additional live checks:

- **Response limit:** a fresh ChatGPT conversation emitted five read blocks in one assistant reply. The first four completed; call index 4 returned `response_budget_exceeded`, visible in the conversation. The model stopped without retries. Native log timestamp: `2026-11-17T05:06:54.565Z`.
- **Concurrent conversations:** ChatGPT, Claude, DeepSeek, Qwen and Kimi each executed the correct one-file read between `05:01:17` and `05:01:43Z`. Four received the result; ChatGPT's context-full gate prevented delivery. This is **not** a five-way delivery pass or proof of simultaneous host dispatch. No cross-provider file attribution was observed.
- **Draft protection:** a known test draft survived approval and queued result delivery in ChatGPT. Clearing only that draft allowed automatic delivery. A later manual **Send pending result** click could not overcome the website's context-full restriction; its error stayed visible and the result remained in the composer.
- **Setup does not authorize tools:** Gemini emitted `list_files` during setup; the bridge executed zero calls until an explicit test task arrived.
- **Extension UI:** readiness, build diagnostics, audit/DB/log information, and Qwen's patch approval were exercised through the real `sidepanel.html` route opened as an extension tab. This does not establish the separate docked Chrome side panel or toolbar-popup lifecycle on this build.
- **OCR:** the live control recognized `Dummy PDF file` from the existing public raster. A traversal path returned `permission_denied`.
- **WAV:** the live inspect/trim controls approved `output/live-clip.wav`: 400 frames, 8 kHz, stereo, 8-bit PCM, 0.05 seconds. Independent Python wave decoding matched every selected source byte. SHA256: `e2bc314adf850521c0727ad8177941d23dc335853005a981213e6ad9a1fc60f5`. Explicit Deny created no `output/live-denied.wav`. An earlier approval timeout also created nothing; a separately approved retry succeeded. The accessibility driver needed `AXPress`, rather than its ineffective click attempt.
- **PDF:** the live creation, text-read and page-preview controls produced `output/live-acceptance.pdf` and one 707 × 1000 PNG. The complete rendered page was inspected: legible title, paragraphs, bullets and footer, with no clipping. PDF SHA256: `34eff68d21e0a6fffcf4fae5911381be331aebdc404284f33226da9195b66467`.
- **XLSX:** the live controls inspected the public sample copy, read A2, approved `output/live-workbook.xlsx`, and reopened A2 as literal text `0.5.21`. Independent ZIP/XML inspection found only the selected worksheet changed; all other package payloads were identical. Quick Look rendered both cells correctly. SHA256: `fe3c247ceccfd92539a364bdef1b7e043efe30e17eb7918b789f8ed167235ac4`.

### Failed restart check and remaining acceptance

With no active command job or tool request, the Chrome native-host process was terminated to exercise recovery. The background automatically launched a replacement and logged `extension hello (0.5.21)` at **05:11:18.819Z**. However, the host had **zero registered sessions** while content panels still displayed their previous IDs as connected. A manual panel read correctly refused to run without a session. No previous write was repeated.

The failure is in session recovery: `stop_native` clears the background session list, but content `poll_status` only registers when its local `session_id` is `None`; it does not invalidate an ID absent from the current host status. The global Reconnect control also keeps a healthy existing native port, so it does not repair this state. Reloading the dedicated provider tabs re-registers them. This workaround is not a passing automatic-recovery test. **Fix session invalidation/re-registration before claiming unattended restart support.**

The workaround restored all nine controlled tab registrations between **05:16:20 and 05:18:23Z**, including both Grok sites and the separate ChatGPT budget conversation. One reload timed out and X reported an aborted navigation; both subsequently registered in the host log. The acceptance workspace still had the same **68 terminal request receipts (67 complete, 1 rejected)** and 13 provider-file recovery records after recovery. No old tool call was rerun. These receipts include panel calls; a completed receipt can contain an expected permission denial and does not prove in-conversation delivery.

Other open checks are Grok's quota-blocked steps, Gemini setup recovery, Qwen foreground/candidate handling, quota/context-aware delivery errors, transient adapter warnings, and the floating panel covering ChatGPT's Send control. The test window being moved mostly off-screen also blocked automation input; moving it back into view restored input and is separate from the bridge defects. TCC persistence across signed updates and a physical older Intel Mac remain unverified.

The newer binary-recovery, code-navigation, Git, CSV, network, MCP-lifecycle and Pause/Resume controls retain their native/packaged evidence below; this pass does not establish all of their browser workflows. In-flight workspace switching, deliberate late-result races, docked-panel/popup approvals and complete all-provider concurrency still need separate live acceptance. No backlog box was marked complete from these partial checks.

Raw host messages, transcripts, UI readings, file hashes, CI job results and independent file checks are local in ignored `docs/evidence/0.5.21/live/`. They include private session/conversation data and are not committed. This section is the public, bounded evidence record.

## Optional PNG OCR candidate 0.5.21 — November 16, 2026

At initial packaging, source/isolated package were 0.5.21 and the normal install was still 0.5.7. The later live acceptance and normal-install upgrade are recorded above.

| Check | Observed result |
|---|---|
| Workspace suite | 220 passed, 0 failed; one separately verified long command omitted from routine runs |
| Four dedicated checks on macOS | Raster text/geometry/truncation, blank/transparent images, corrupted/oversized/high-depth PNGs, strict args/stale hashes/private/research/link/size denials, and worker denial of user-file reads/writes/TCP. Linux has a separate unsupported-backend assertion |
| Backend prototype | Vision accurate mode failed with E5RT compiler initialization errors. Removing filesystem read restrictions in an isolated diagnostic did not fix it. Fast CPU revision 1 English succeeded with the original restrictions restored; only that mode is exposed |
| Isolated package | Native/tooling/WASM release builds exited 0; source/package/manifest/native versions matched 0.5.21 |
| Public raster | Packaged Native Messaging recognized exactly `Dummy PDF file` from the PNG previously rendered from W3C's public PDF |
| Repository report | Recognized 17 regions from the actual generated project brief. Text/boxes/confidence returned; errors such as `paste iL`, `projert`, `prowder` and `113` for the page footer were retained in evidence |
| Accuracy test correction | The first test incorrectly required exact recognition of `WRITING`; Vision returned `WRrrING`. The test now checks the expected heading/prefix and >90% character similarity for two clear lines. Exact policy/budget assertions are unchanged; this is not a lossless-text claim |
| Denials/restart | Explicit 256-byte truncation; changed hash, traversal, unknown language field and hard links refused. Original PNG hashes unchanged. Repeated public-page recognition after host restart matched |
| Installed checks | 41 self-tests passed; doctor verified config/manifest/binary/SQLite and 20 matching audit entries |
| CI completion recheck | 0.5.19 run 43964530182, 0.5.20 run 43966311041 and 0.5.21 run 43968525423 passed Linux/Intel tests and packaging; release jobs skipped |
| Browser/physical Intel | Live OCR control passed in the later acceptance above; the physical older Intel Mac remains unverified |

Public raster SHA256 `6bdc49ae22aac99c95725e2355f3ccaf96e49dec339e5926c42a2dece686a90c`. Report raster SHA256 `768a16bfd37b0e3b2019b47357e4d322b0fc849f533d3e01693f4964733196e7`. Native SHA256 `c05aad1d7c01355f48b096eba9f4d6920ca9c627e24a420511a6a41eed082d17`; WASM SHA256 `92f27f7fc721e90f6eb21e6e841f6280535a8919fc75563d60583dd225265ec1`. Input copies, full OCR results, raw framed messages and logs are local in ignored `docs/evidence/0.5.21/`.

This is bounded fast English OCR, with expected recognition errors, not general image understanding or image attachment. No accurate-mode fallback, other language selector, PDF password bypass or cloud OCR exists. See [OCR scope and restrictions](OCR.md).

## PCM WAV candidate 0.5.20 — November 16, 2026

Source/isolated package are 0.5.20. Normal Chrome files/native installation remain 0.5.7; live browser acceptance is pending.

| Check | Observed result |
|---|---|
| Workspace suite | 216 passed, 0 failed; one separately verified long command omitted from routine runs |
| Seven dedicated checks | Exact 8/16/24/32-bit mono/stereo sample preservation with independent Hound decoding, odd padding, invalid/truncated chunks, duration/count/file limits, strict arguments, source/destination changes, lock/denial, research/private/link protections |
| Isolated package | Native/tooling/WASM release builds exited 0; source/package/manifest/native versions matched 0.5.20 |
| Public sample | SciPy's actual 8 kHz stereo unsigned 8-bit PCM WAV reported 800 frames. Approved trim selected frames 200–600 and created 400 frames / 0.05 seconds, preserving every selected byte and the source hash |
| Independent decoding | Python wave reopened the output; macOS afinfo reported 400 packets / 0.050000 seconds. macOS afconvert decoded every sample to signed 16-bit PCM with values exactly matching the source conversion. No sound was played and no model received audio |
| Denials/restart | Denied write created nothing; repeat destination, stale source hash and traversal refused. Reopened the saved hash after host restart |
| Verification correction | A second public SciPy extensible WAV also has an inconsistent RIFF length (17,708 declared vs 17,720 actual). The first verification script incorrectly expected unsupported; the host correctly returned invalid_arguments at the earlier length check. Corrected the assertion, reran in a fresh scratch workspace, and kept the initial log. Validation was not relaxed |
| Installed checks | 41 self-tests passed; doctor verified config/manifest/binary/SQLite and all 51 matching audit entries, including both verification runs |
| Previous CI | 0.5.18 run 43961212786 passed Linux, Intel tests and Intel packaging. 0.5.19 run 43964530182 passed Linux; Intel jobs were still running when checked |
| Browser | Inspect/trim controls compiled. Live UI approval/provider delivery remain unverified |

Public source: [SciPy PCM sample](https://raw.githubusercontent.com/scipy/scipy/main/scipy/io/tests/data/test-8000Hz-le-2ch-1byteu.wav), SHA256 `47a109b21bd0a79615478181f6ee0a867e4733554c7c9523d3cda2f2d901209f`. This is explicitly a public audio test sample, not a user recording or provider integration. Output: `/private/tmp/uab-0520-build-zn6f6anr/scratch-general-final/output/clip.wav`, SHA256 `e2bc314adf850521c0727ad8177941d23dc335853005a981213e6ad9a1fc60f5`. Native SHA256 `45bbc2ff2bdcdbe5524750f726dcc700ce47e5601955723bd287af5c59fcc079`; WASM SHA256 `922596a064ea4012de690f897a743ccd8229dd8dbdd724ed7c7d6f5e9186e115`.

Raw logs, framed messages, audio copies and independent decoded output remain in ignored `docs/evidence/0.5.20/`. The maintained implementation is Rust; Python and macOS audio utilities were independent verification only. Broader formats, playback checks, transcription, conversion, video and browser acceptance remain open. [Scope and limits](MEDIA.md).

## PDF report creation candidate 0.5.19 — November 16, 2026

Source and isolated package are 0.5.19. Normal Chrome files/native installation remain 0.5.7; live browser acceptance is pending.

| Check | Observed result |
|---|---|
| Workspace suite | 209 passed, 0 failed; one separately verified long command test omitted from routine runs |
| Seven dedicated checks | Deterministic embedded-font layout, line/page bounds, NFC/Unicode extraction, orphan/break limits, strict schema, unsupported scripts/formats, approval denial, overwrite/symlink/parent changes, workspace lock, research/private path protections |
| Isolated package | Native/tooling/WASM release builds exited 0; source, package and manifest versions matched 0.5.19. The added popup/side-panel creation form was included in the final WASM rebuild |
| Packaged Native Messaging | Created a three-page project brief from actual README/documentation content. PDFKit reopened it, extracted text including Résumé/café/Ångström and rendered all three physical pages |
| Visual inspection | Opened every rendered PNG: headings, paragraphs, hanging bullets, accents and page numbers were legible, with consistent margins and no clipping/overlap |
| Denials/restart | Denied creation wrote nothing. Existing destination, traversal and unsupported glyph refused. File/hash preserved and PDF reopened after host restart |
| Installed checks | 41 self-tests passed; doctor passed config/manifest/binary/SQLite and verified 30 matching audit entries |
| Previous CI | 0.5.17 run 43957615402 passed Linux, Intel tests and Intel packaging. 0.5.18 run 43961212786 passed Linux tests, Intel tests and Intel packaging (completion rechecked during 0.5.20 work) |
| Browser | New tool/form compiled; provider delivery, approval interaction and loaded 0.5.19 browser build remain unverified |

Created file: `/private/tmp/uab-0519-build-ou5lpw7j/scratch-general/output/bridge-project-brief.pdf`, 1,203,686 bytes, SHA256 `8a101b348acfaa8e640325f525c42f2f2724e8b87a35a28331b63314764fcf26`. Installed native SHA256 `ac851ed58cccee45770086aa4458741eb16bec3fe53ff238173c65e4049401b7`; final WASM SHA256 `6d155b7e97cf922bef44ee95fde8d8c8f87f26a7c5ce26123852786430687ff8`. Raw framed messages, logs, PDF and page PNGs remain under ignored `docs/evidence/0.5.19/`. Tests use labeled fixtures; the packaged report uses repository documentation and is not a provider integration.

The output format is a fixed simple Latin PDF report, not arbitrary Word/PDF editing. DOCX authoring remains pending because its required bundled renderer/LibreOffice runtime is unavailable. PPTX, conversion, richer layout and live acceptance remain open. See [report semantics](DOCUMENT-CREATION.md).

## Public network candidate 0.5.18 — November 16, 2026

The source and isolated package are 0.5.18. Normal Chrome files/native installation remain 0.5.7; live browser acceptance is pending.

| Check | Observed result |
|---|---|
| Workspace suite | 202 passed, 0 failed, one separately verified long job omitted from routine runs |
| Five network unit checks | Private/reserved/encoded addresses and mixed DNS answers rejected; redirects checked; actual local TCP bytes bounded; compression/oversize refused; no cookies/referrer/auth; pinned address used without resolving its hostname again; streaming HTML decoding/truncation/deep markup |
| Three policy checks | Default-off and origin scope, strict arguments, denial/revocation during approval with no network request, research/private/traversal/overwrite protections, workspace separation and fresh grant revisions |
| Isolated package | WASM/native/tooling release builds exited 0; source/package/manifest/native versions matched 0.5.18 |
| Public HTML | Approved GET of example.com returned its actual Example Domain text. Body SHA-256 was `25ddf2c883e0d1958ea971d279a7e4f0fd446724ee3db7db19dadabd4a62e484` |
| Public download | Approved GET of W3C dummy.pdf saved a new PDF. Its SHA-256 exactly matched the independently downloaded public sample used for 0.5.15 |
| Refusals | Disabled access, denied fetch/download, private loopback, query string, ungranted origin, outside-root destination and repeat destination all refused. Denied download created no file; disable survived restart |
| Audit/privacy | Doctor verified 41 matching audit entries and SQLite integrity. Raw PDF URL path and the refused query value did not appear in audit or host log |
| Installed self-test | 41 passed, 0 failed |
| Previous CI | Task candidate 0.5.17 run 43957615402 passed Linux tests, Intel tests and Intel packaging (completion rechecked during 0.5.19 work) |
| Browser | Network controls compiled. Live approval/control interaction and provider delivery remain unverified |

Saved file: `/private/tmp/uab-0518-build-5i5am12_/scratch-general/output/public-dummy.pdf`, SHA-256 `3df79d34abbca99308e79cb94461c1893582604d68329a41fd4bec1885e6adb4`. Public source: [W3C dummy.pdf](https://www.w3.org/WAI/ER/tests/xhtml/testfiles/resources/pdf/dummy.pdf). Installed native SHA-256: `bf08449e04fd00005a7003e2b830f8cbe761f7eea6bf2f20de9965dca493bfc5`. Logs, framed messages and PDF remain local under ignored `docs/evidence/0.5.18/`. Local TCP fixtures exercise transport internals and are explicitly separate from production public-address checks; they are not provider integrations.

Only standard-port public HTTP(S), selected origins and anonymous GETs are supported. No query strings, cookies, credentials, compressed bodies, private-network exceptions or implicit retries. Text is static UTF-8 source extraction, not browser-rendered visibility. Network grants remain off in the normal installation. [Complete limits](NETWORK.md).

## Durable chat-call controls candidate 0.5.17 — November 16, 2026

The source and isolated package are 0.5.17. Normal Chrome files/native installation remain 0.5.7; live browser acceptance is pending.

| Check | Observed result |
|---|---|
| Workspace suite | 194 passed, 0 failed, one separately verified long job omitted from routine runs |
| Six task tests | Frozen budgets/reconnect, stop finality, conflicting task identity, workspace/provider separation, pause-resume revision checks, late approval refusal, bounded private reports, expired uncertain requests |
| Framed integration | Real native process created/read a file, kept pause across restart, returned the saved write result without re-execution, resumed a new read, stopped a pending approval and refused a late approval |
| Isolated package | WASM/native/tooling release builds exited 0; source/package/manifest/native versions matched 0.5.17 |
| Packaged write/recovery | Read an actual README copy, approved `output/task-proof.md`, rejected a paused write, restarted and replayed the same write receipt with unchanged bytes. Resume allowed a fresh read; Stop closed another pending write approval |
| Interrupted approval | Killed the installed host while a second task awaited approval. After its deadline, the same slot returned `request_timeout` and remained unverified; its file was absent and it was never rerun |
| Report and audit | Saved private mode-0600 report with 3 completed and 2 cancelled requests; completed steps carried audit sequence links. Doctor verified 27 database/file audit entries and SQLite integrity |
| Installed self-test | 41 passed, 0 failed |
| Previous CI | XLSX candidate 0.5.16 run 43953697257 passed Linux tests, Intel macOS tests and Intel packaging |
| Browser | Controls/status propagation compiled. Pause/Resume UI, skipped-reply behavior and live conversation delivery remain unverified |

Created file: `/private/tmp/uab-0517-build-fw064hu6/scratch-general/output/task-proof.md`, SHA-256 `408a5c8474260bfe31b5f9310428ab4be6f54bba52d5eb56f2f07016f44a03db`. Installed native SHA-256: `6e243d5ad72d26d8625a7881b06b100ffb5ac63f17d86c5298daaddad46b564c`; WASM SHA-256: `0923f651cdda9695c8ed48899b8933dc747c1d790849150d2bfbbd4d21fcdf0f`. Logs, framed messages and exported report remain local under ignored `docs/evidence/0.5.17/`.

The report is a snapshot of submitted requests, not whole-task completion. Already executing operations can finish, and background jobs retain separate cancellation. Resume preserves budget without restarting a model or replaying old calls. These are partial UAB-04/06 capabilities; see [semantics and remaining work](OVERNIGHT.md#chat-task-controls-0517-candidate).

## XLSX read/edit-copy candidate 0.5.16 — November 16, 2026

The source and isolated package are 0.5.16. Normal Chrome files/native installation remain 0.5.7; live browser acceptance is pending.

| Check | Observed result |
|---|---|
| Workspace suite | 187 passed, 0 failed, one separately verified long job omitted from routine runs. Six workbook tests also passed after the final audit/metadata/error-bound changes |
| Workbook tests | Typed values, stale formula caches, hidden sheets, sparse cells, Unicode/OOXML literal escapes, preserved styles/parts/source, denied/changed approvals, private/research/link policy, hostile XML/ZIP/counts, duplicate cells, 1,000 unsupported features and strict edit refusal |
| Isolated package | Final WASM/native/tooling release builds exited 0; source/package/manifest/native versions matched 0.5.16 |
| Public XLSX | Calamine's public empty_shared_string.xlsx returned Sheet1 A1=`abc` and A2 empty. The denied edit wrote nothing; approval saved a new copy with A2=`Edited`. Bridge read-back matched |
| Independent checks | Python stdlib ZIP/XML reads confirmed A2 is literal inline text and every other package payload remained byte-identical. macOS Quick Look opened/rendered the saved copy; the thumbnail was inspected and showed `abc` and `Edited` |
| Refusals | Repeated destination, private .env read and changed-source hash were refused. Original remained unchanged by the edit. A separate longer-text preview overflowed the original width, confirming that this cell editor does not autofit columns |
| Installed checks | 41 self-tests passed; doctor passed config/binary/manifest/SQLite and all 24 database/file audit entries |
| Previous CI | PDF candidate 0.5.15 run 43950054650 passed Linux tests, Intel tests and Intel packaging. 0.5.14 run 43946276762 also passed all required jobs |
| Browser | Workbook controls compiled; provider delivery and live UI acceptance remain pending |

Public sample: [Calamine empty_shared_string.xlsx](https://github.com/tafia/calamine/blob/v0.38.0/tests/empty_shared_string.xlsx), SHA-256 `88bcdff00d736ca11e578e7dc5d5072b29af99d8a2f6594fe479eb5c66c57c9f`. Edited copy SHA-256: `a86f75be9033cfdc1d9b37214e53be8b5b0339e8afca96c990fe538c004ec819`. Installed native SHA-256: `f77eaab88c98813ae179e68c496128af2993aa5fc8dfdd94eb86cda0f812feb9`. Raw logs, framed messages, workbook and Quick Look thumbnail are in ignored `docs/evidence/0.5.16/`. The bundled spreadsheet artifact renderer was unavailable; Quick Look and independent XML reads supplied the separate checks. This does not prove Excel-app integration or recalculation. [Subset and limits](WORKBOOKS.md).

## PDF text and page-preview candidate 0.5.15 — November 16, 2026

The source and isolated package are 0.5.15. Normal Chrome files/native installation remain 0.5.7; no new live browser acceptance is claimed.

| Check | Observed result |
|---|---|
| Workspace suite | 181 passed, 0 failed, one separately verified long job omitted from routine runs |
| Six PDF integration checks | Physical-page text/chunk attribution, empty text pages, rotation/pixel positions, approved new PNGs, preserved source, denial, changed source/target/lock/symlink, research and hard-link protections, encryption/size/malformed/range errors |
| Worker restrictions | A separate child test verified denied user-file reads, writes and a connection to a real local listener. The final narrower system-library allowlist was retested successfully |
| Isolated build | WASM/native/tooling release builds exited 0; source/package/manifest/native versions matched 0.5.15 |
| Packaged Native Messaging | W3C's public dummy.pdf returned `Dummy PDF file` and physical page 1. Denial created no folder; approval saved a 707 × 1000 PNG. Repeated destination, private read and changed-PDF continuation were refused |
| Render inspection | The packaged PNG was opened and visually inspected: upright, legible text, white page and expected placement. Pixel tests caught and corrected CoreGraphics' default centering without upscaling; rotated-page placement also passed |
| Installed checks | 41 self-tests passed; doctor passed config/binary/manifest/SQLite and all 22 database/file audit entries |
| Compatibility evidence | The local arm64 binary declares macOS 11.0 minimum, SDK 27.2. This is build metadata, not proof on macOS 11 or a physical old Intel Mac. PDF is explicitly unsupported on Linux |
| Previous CI | 0.5.13 run 43944425521 passed all jobs. 0.5.14 run 43946276762 passed Linux and Intel tests, confirming the lifetime-lock test fix; packaging was still running |
| Browser | Document controls compiled; provider delivery and live UI acceptance remain pending |

Public sample: [W3C dummy.pdf](https://www.w3.org/WAI/ER/tests/xhtml/testfiles/resources/pdf/dummy.pdf), SHA-256 `3df79d34abbca99308e79cb94461c1893582604d68329a41fd4bec1885e6adb4`. Packaged preview SHA-256: `6bdc49ae22aac99c95725e2355f3ccaf96e49dec339e5926c42a2dece686a90c`. Installed native SHA-256: `849aa5cc2efdfe6ee72b9c538736408e81cbbf6e9f736beb432b257e1574b70f`. Raw logs, framed messages and preview are in ignored `docs/evidence/0.5.15/`. This proves a declared PDF subset, not OCR, annotations/forms, Word rendering or image delivery to a provider. [Limits](DOCUMENTS.md).

## MCP lifecycle candidate 0.5.14 — November 16, 2026

The source and isolated package are 0.5.14. Normal Chrome files/native installation remain 0.5.7; no new browser acceptance is claimed.

| Check | Observed result |
|---|---|
| Four integration tests | Actual bridge RMCP discovery/read and denial checks; separate native lifecycle fixture verifies removal during initialization/discovery, running-call removal/lock/timeout, inherited child-process cleanup and no-start readiness |
| Workspace suite | 174 passed, 0 failed; one long-duration test omitted from routine runs and separately passed on 0.5.10. The corrected lifetime-lock test passed |
| Isolated package | WASM/native/tooling release builds exited 0; source/package/manifest/native versions all 0.5.14 |
| Packaged Native Messaging → RMCP | Registered the isolated actual bridge executable, discovered/granted read_file, denied one request, read copied docs/GIT.md over two separate connections, then removed the grant and refused the next request |
| Installed checks | 41 self-tests passed; doctor passed config/binary/manifest/SQLite and 21 matching audit entries |
| Intel CI finding | 0.5.12 run 43942537947 passed Linux but failed crashed_receipt_is_reconciled_without_rerun on Intel. The test rewrote a terminal receipt while the exiting worker still held its lifetime lock. It now waits for lock release, verifies a stale heartbeat remains running while a lock is owned, then verifies interruption after release. No recovery invariant was weakened; Linux and Intel tests passed in run 43946276762 (packaging was still running when checked) |
| Browser | Readiness/removal controls compiled; live UI acceptance remains pending |

Installed native SHA-256: `bad329bdf5463ef575f800721f9fb915e070a786b5afe5a3cd33931b2a38259e`. Raw outputs are in ignored `docs/evidence/0.5.14/`. The delay/process test fixture is not presented as a third-party application or provider integration. [Semantics and remaining work](INTEGRATIONS.md).

## DOCX body-text candidate 0.5.13 — November 16, 2026

The source and isolated package are 0.5.13. Normal Chrome files/native installation remain 0.5.7; the new browser controls remain unverified.

| Check | Observed result |
|---|---|
| Workspace suite | 171 passed, 0 failed; one long-duration test omitted from routine runs and separately passed on 0.5.10 |
| Five document checks | Unicode chunks; physical text boundaries; omitted content; revisions/fields; inert relationships; hostile XML/ZIP; stale/invalid offsets; private/link/unknown-input denials; response limits |
| Isolated package | Final WASM/native/tooling builds exited 0; source/package/manifest/native versions all 0.5.13 |
| Public DOCX | Packaged Native Messaging extracted Apple and Banana from Mammoth's public simple-list.docx. macOS textutil independently returned the same labels with bullet formatting, which the bridge explicitly does not reproduce |
| Refusals | Private .env read denied; PDF reported unsupported; changed DOCX continuation returned file_changed; page_count remained null |
| Installed checks | 41 self-tests passed; doctor passed config/binary/manifest/SQLite and 13 matching audit entries |
| CI observed | 0.5.10 run 43938573686 passed Linux/Intel tests and Intel packaging. 0.5.11 run 43940962932 passed Linux/Intel tests; packaging was still running |

Sample: [Mammoth simple-list.docx](https://github.com/mwilliamson/mammoth.js/blob/master/test/test-data/simple-list.docx), SHA-256 `ed3c015cf411d07b08bf34264ff709363740b0747af355479088079d0796dfb3`. Installed native SHA-256: `1fce48325ff88cc9e21c1bebfed030731eb5c8d91523ec90a5140a5b73ff1a4e`. Raw outputs are in ignored `docs/evidence/0.5.13/`. This establishes a body-text subset, not PDF support, visual fidelity or a provider conversation. [Limits](DOCUMENTS.md).

## CSV/TSV candidate 0.5.12 — November 16, 2026

The source and isolated package are 0.5.12. Normal Chrome files and installed host remain 0.5.7; browser acceptance is pending.

| Check | Observed result |
|---|---|
| Workspace suite | 166 passed, 0 failed; one separately exercised long job omitted from routine runs |
| Five table tests | Quoted/multiline Unicode records; physical line numbers; column projection; stale-page refusal; all-matched-record aggregates; skipped numeric values; input/output limits; private/traversal/link/unknown-field denials; unchanged research dataset |
| Isolated package | Final WASM/native/tooling builds exited 0 and versions matched 0.5.12. An initial Leptos event-expression parsing error was corrected before the final suite/build |
| Actual data | CSV exported from this repository's Git history contained 19 records. read_table returned the correct first commit and continued at record 4 |
| Actual query | Subjects containing Git matched two records. Independently computed subject-byte total was 129; query_table returned sum 129, mean 64.5, min 60, max 69 across both records despite a one-row display page |
| Denial and stale input | Private .env read was denied. An independent CSV append caused continuation to return file_changed |
| Installed checks | 41 self-tests passed; doctor passed config/binary/manifest/SQLite and 13 matching audit entries |
| Intel regression | 0.5.10 run 43938573686 passed Linux and Intel macOS tests, confirming the search performance correction; packaging was still running at this observation |

Original exported CSV SHA-256: `69ce812f4944ab13252d1df96dee2a9ab2fbc8b319f783d92ab07604ef111e4f`. Installed binary SHA-256: `f548cf16d981db5a7172759c93511f10ba1947c9905a04cfa729bcc1f36b30f7`. Raw outputs are in ignored `docs/evidence/0.5.12/`. These are native transport checks on real repository data, not an AI website integration. [Supported table semantics](TABLES.md).

## Reviewed Git commit candidate 0.5.11 — November 16, 2026

The source and isolated package are 0.5.11. The normal Chrome package and native installation remain 0.5.7; these are host/transport checks, not live website acceptance.

| Check | Observed result |
|---|---|
| Workspace suite | 161 passed, 0 failed; one long-duration test excluded from the routine suite and separately passed on 0.5.10 |
| Ten commit/history tests | Approved working-file bytes committed on a new branch; staged bytes, HEAD and unrelated edits preserved; denial/stale/hash/HEAD/lock/owner/expiry paths refused; no filters/hooks/signing; protected historical reads; interrupted/failed publication retained receipts; parent-file and symbolic-ref conflicts refused |
| Isolated release package | WASM/native/tooling builds exited 0; source/package/manifest/native versions all 0.5.11 |
| Packaged Native Messaging | Denied commit created no branch. Approved commit included exactly src/recovery.rs and docs/[reviewed].md, with matching bytes. HEAD, user's index and unrelated draft hashes were unchanged |
| Restart and replay | Terminated/restarted the installed test host; status and repeated apply returned the original commit ID, without another commit |
| History | git_log returned the actual new commit; git_show returned matching source content; private selections/traversal were denied |
| Installed checks | 41 self-tests passed; doctor passed config/binary/manifest/SQLite and 34 matching audit entries |
| Browser | New controls compiled but remain unverified in Chrome because the current Browser connection is unavailable |

The disposable repository commit is `68cd37a8807f806ed871373cce52be5a1d511603`, based on preserved HEAD `a506751e56967cff56d8a207249afa8c0471854b`. Installed binary SHA-256: `1830501d0dcea751caa5885d0982ab1fc29ea337003df640fa0ae229bc67a2cc`. Raw outputs are in ignored `docs/evidence/0.5.11/`. [Workflow and supported subset](GIT.md).

## Git inspection candidate 0.5.10 — November 16, 2026

The source and isolated package are 0.5.10. The normal Chrome package and installed host remain 0.5.7; no new browser result is claimed.

| Check | Observed result |
|---|---|
| Workspace suite | 151 passed, 0 failed; one deliberately slow test excluded from the routine run |
| Long-command acceptance | Separately ran the ignored real Cargo job: completed with exit 0 after 249.608 seconds, with saved output read through reopened host state. The test itself slept 245 seconds. This proves duration/recovery handling, not unattended browser task orchestration |
| Seven Git tests | Real staged/unstaged differences; private-path denial; unchanged index; unrelated edits preserved; literal filenames; metadata/config boundaries; blocked external filters; count/byte limits |
| Intel search correction | 0.5.9 CI run 43934903095 failed the 100-file search assertion on Intel macOS: repeated private-policy compilation consumed the two-second budget. 0.5.10 compiles policy once per call while still resolving/checking every path. Run 43938573686 passed Linux and Intel macOS tests with the original assertion intact; packaging was still running at this observation |
| Isolated package | Pinned WASM/native/tooling release builds exited 0. Source/package/manifest and installed native version all 0.5.10; normal package remains 0.5.7 |
| Native transport | Actual installed host returned 0.5.10/protocol 6; staged diff of copied recovery.rs contained the new line, private selection/traversal were denied, branch was main, index and unrelated file hashes were unchanged |
| Installed checks | 41 self-tests passed; doctor passed config, binary/manifest, SQLite and 13 matching audit entries |
| Browser gate | Current Browser connector remains unavailable; new Git/recovery/navigation UI acceptance remains pending |

Native binary SHA-256: `dfe0e751573645a8ab676b1612a8787e2fe8be8b7f093803ed3c6fc328fbd77e`. Raw evidence is in ignored `docs/evidence/0.5.10/`; see [Git behavior](GIT.md). This candidate implements reads only; the UAB-10 commit workflow remains open.

## Code navigation candidate 0.5.9 — November 16, 2026

The source and isolated package are 0.5.9; the normal Chrome package/host remain 0.5.7. No live Chrome or provider acceptance is claimed for this candidate.

| Check | Observed result |
|---|---|
| Workspace suite | 144 passed, 0 failed; one deliberately slow test ignored |
| Rust declarations | Source line checks passed for functions, types, methods, inline modules and Unicode identifiers; comments, strings and macro expansions were not mistaken for declarations |
| Parser failure isolation | Incomplete source and deeply nested input returned errors; a following valid parse succeeded through the same host API |
| Search | Case-insensitive matches paginated without repeating lines; changed-file cursors refused; 100-file and excluded-entry tree limits surfaced explicitly |
| Protected inputs | Private files, symlinks, traversal and unknown fields refused; unsupported languages explicitly direct the caller to text search |
| Coding task | Located a function, found its caller, applied an explicitly approved two-file patch, then passed a real granted rustc metadata check |
| Compiler errors | A deliberate type error returned E0308 and the correct readable workspace file/line; private and outside-root locations were omitted |
| Audit | Successful declaration/search calls and the coding mutations have audit records; chain validity checked |
| Isolated package | WASM/native/tooling release builds and native install exited 0; all staged versions 0.5.9, normal installation preserved at 0.5.7 |
| Native transport | Real framed calls parsed a copy of the project’s recovery.rs: 21 declarations, every reported line checked against source. Search pages returned lines 224, 411, 446, 453, 459. A changed file and unsupported TypeScript returned explicit errors |
| Installed checks | 41 self-tests passed; doctor passed SQLite, installed binary/manifest and 15 matching audit entries |
| Previous candidate CI | 0.5.8 run 43931890870 passed Linux/Intel macOS tests and Intel packaging |
| Subsequent CI finding | 0.5.9 run 43934903095 passed Linux tests but failed Intel macOS's 100-file search assertion (0 files scanned before timeout). See the 0.5.10 correction above; the earlier local pass did not establish Intel behavior |

The five dedicated navigation checks are part of the workspace total. Raw outputs are kept in ignored `docs/evidence/0.5.9/`. `list_symbols` is a syntax declaration reader, not semantic `find_references`; that part of UAB-09 remains pending along with live UI acceptance.

## Binary recovery candidate 0.5.8 — November 16, 2026

This candidate was built and installed in an isolated directory. The live extension package and normal installed host remain **0.5.7**; no 0.5.8 Chrome or provider result is claimed.

| Check | Observed result |
|---|---|
| Workspace suite | 139 passed, 0 failed; one deliberately slow test ignored |
| New recovery checks | Seven integration tests plus three interruption/storage/cleanup tests passed |
| Build | Pinned WASM, native and tooling release builds exited 0; staged lib.rs, Cargo.toml and manifest all 0.5.8 |
| Isolated installation | Native manifest points to the isolated installed binary, version 0.5.8; normal Chrome manifest unchanged |
| Native handshake | Real length-prefixed hello returned host 0.5.8/protocol 6 |
| Real binary replacement | Copied the actual 0.5.7 and 0.5.8 extension WASM assets into an isolated general workspace: 1,419,644 → 1,467,238 bytes. Approved replacement matched the new hash; denied replacement preserved the old bytes |
| Recovery after restart | Terminated and restarted the installed test host; restore from the saved change ID returned the exact original bytes |
| Trash and restore | The larger original disappeared from its public path after approved trash; approved restore reproduced its SHA-256 |
| Later edit | Appended an independent edit; restore returned file_changed and preserved that edit |
| Settings and history | New trusted wire controls reported four snapshots / 8,613,052 counted bytes; retention changed to 31 days in this isolated workspace. Four distinct committed receipts retained CHATGPT attribution |
| Selftest / doctor | 41 self-tests passed. Doctor passed config, installed binary, native manifest, SQLite and 36 matching audit DB/file entries |
| Browser gate | Browser runtime reported no available Chrome connection. Its extension and native connector are absent. Skill troubleshooting prohibits an AppleScript substitute. No new Chrome approval, controls or provider acceptance is claimed |
| Prior release CI | 0.5.7 run 43926814464 passed Linux tests, Intel macOS tests and Intel packaging |

Actual source hashes:

```text
0.5.7 WASM  9e77cfdcc84d99a02f0037c8fc9b0d7c2ececea64f39d51a404b835e0a40de4c
0.5.8 WASM  75b3125d4339acb2a529602634081c46b1e3aab5510e5102a34b960a8f59d9cf
```

Raw results are in ignored `docs/evidence/0.5.8/`: tests.log, deploy.log, selftest.log, doctor.log and wire-results.json. The protocol driver is a test client of the real installed host, not a browser extension or model conversation. Expired cleanup was tested with deliberate old timestamps in temporary test fixtures; no live retention or cleanup result is fabricated. UAB-15 remains open for live UI acceptance.

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
