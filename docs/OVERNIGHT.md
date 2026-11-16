# Bounded work while you are away

The bridge can run an approved command as a background job, save its output, and recover its receipt when Chrome reconnects. Candidate 0.5.17 adds durable chat-task admission controls and local task reports. It does not yet schedule a whole sequence of AI tasks, restart a model after a pause, resume uncertain edits, or work through a login or account limit on your behalf.

## Chat task controls (0.5.17 candidate)

The popup and side panel have **Chat task controls and receipts**. Refresh to see up to 100 recent tasks in the selected workspace. Each record belongs to the host-validated provider, conversation, workspace and observed user message. Registration or receiving a tool result does not create a task. Records appear when a task first requests a tool; they do not enumerate planned steps the model has not submitted.

**Pause local calls** blocks new calls and calls waiting for an execution slot. It also closes pending approvals, including when Pause is quickly followed by Resume. A call already executing outside an approval wait may finish. **Resume local calls** permits new response slots with the same remaining budget. It does not send a message, wake a model or replay a rejected block. The content script skips replies observed while paused. Continuing an idle conversation may require a new user message, which is a new task with its own budget. **Stop this task** permanently closes admission for that task; a new explicit user task can still start.

These controls do not undo files, terminate background command jobs, stop website generation, or disable the user's direct tool runner/MCP connection. Use **Stop job** separately for a running command. Stop is not an emergency stop for every process on the computer. The host checks task state independently of the panel, including after reconnect.

Per-response and task call limits are saved when the task is first observed. Raising configuration limits does not expand an existing task. A stricter current configuration also applies. Calls reserved before Pause keep their consumed budget; denied admissions do not consume another reservation. A cached result can be recovered while paused or stopped without executing again. A different tool payload or task identity cannot reuse that response slot. Failed, timed-out and uncertain mutations never rerun automatically.

**Status / first receipts** and **Next receipts** show 50 bounded request summaries per page: tool, requested path, actual calling session, timestamps, duration, outcome, recovery/job IDs when present and the audit sequence for newly finished calls. “Running” means admission is open, not that the model is working. `observed_age_seconds` is wall-clock age since first observation, including idle time. A completed `run_command` request can mean it only queued a job; inspect the linked job receipt.

**Save local task report** creates a private JSON snapshot under the application home's `reports/tasks/`. It distinguishes completed, failed, denied, cancelled, in-progress and unverified requests. Reports include at most 512 summaries and declare whether more exist; no raw argument bodies or file contents are exported. Requested paths are not a complete list of changed files: multi-file tools and external programs have their own receipts. An expired request without a terminal receipt is unverified, even if its file happens to exist. Reports do not infer whole-task completion or certify that the user's objective was achieved.

The packaged host restart/write/approval checks passed; live browser acceptance of these new controls remains pending. Whole-task plans, explicit start-time budget selection, scheduling, automatic continuation and comprehensive completion reports remain open in UAB-04/06.

## Before starting

Open the extension popup or side panel and expand **Readiness check**. Confirm that the panel, background worker and host versions match, the workspace is correct and unlocked, and the relevant commands are granted. **Check host and workspace** reads actual disk space, workspace policy and audit status. macOS file, Accessibility and screen-recording grants remain “unknown until exercised”; configuration alone cannot prove access. A registered browser session also does not prove that its composer is usable or that the account has quota.

## File-write grants

In **Scoped file-write grants**, choose a registered session, an existing directory, the allowed operations, duration, and call/byte/path budgets. Available operations are write, edit, append, copy, and create-folder. The grant belongs to that session and workspace; a new session after reconnect requires its own grant.

A grant can replace repeated write approvals, including in Strict mode, within those limits. It cannot override research datasets, another agent's private directory, a workspace lock, exclusions, symlink checks, or stale-file hashes. Expiry, revocation and exhaustion stop granted writes. Revocation applies before the next commit; an already committed write is not undone. A very short concurrent commit can make the grant control return busy; refresh and retry rather than assuming revocation succeeded.

Bytes count the full new file contents on each write, including append operations. Destination paths are counted once; directory creation counts its requested path. Failed or interrupted writes may consume reservations. Deletes, restores, imports, commands, integrations and desktop actions retain their separate controls. A file grant does not increase the host's per-response or per-task call limits.

## Command jobs

After granting an exact command in **Allowed commands**, use `run_command` with `background: true`. Its result is a job ID, not proof that the command succeeded. Read `job_status`, then page through `read_job_output` using `next_cursor`. Poll sparingly. `inspect_processes` shows only this conversation's bridge-owned jobs and omits arguments and environments.

The command keeps its original workspace. A Rust worker persists its receipt and capped output in the host database, independently of the Native Messaging connection. It rechecks command grants and workspace locks while running. At most four jobs can be active per workspace. Timeouts, nonzero exits, cancellation and interruption have distinct terminal receipts. Interrupted jobs never automatically rerun.

The panel's **Command jobs** section can inspect any job in the selected workspace, stop it, or save a private JSON receipt under the bridge application home's `reports/jobs/` directory. Stop terminates the command process group but does not reverse file changes. Command programs and project build scripts run with your OS account permissions; a working directory is not an OS sandbox.

**Keep awake for job** prevents idle system sleep for up to one hour, bounded by the granted command timeout. It is released when the job ends, when you click Release, or at its deadline. It does not unlock the screen, suppress permission dialogs, keep an AI account logged in, or prevent lid-close sleep. macOS only.

## Updating and macOS permissions

`cargo bridge-deploy` refuses to rebuild/install while live command workers hold the deployment lock. Wait for them or stop them first. After installing, reload the extension and provider pages and verify the loaded build.

Set `UAB_SIGNING_IDENTITY` to an existing signing identity to sign the native host during deployment. The deployer uses a fixed bundle identifier and verifies the signature. It never creates certificates or changes keychain trust. This development machine had no valid signing identity at implementation time, so persistent macOS consent across updates remains unverified. An ad-hoc signature is not a solution to repeated access prompts.

Grant Files & Folders access only for the folders you intend to use. Accessibility, Screen Recording and Automation are needed only for the corresponding desktop/browser features. A one-time Full Disk Access grant is an optional broader alternative, not proof that an unsigned replacement binary will retain consent. Keep the installed host at its stable application-support path. Do not reset the keychain.
