# Bounded work while you are away

The bridge can run an approved command as a background job, save its output, and recover its receipt when Chrome reconnects. It does not yet schedule a whole sequence of AI tasks, resume uncertain edits, or work through a login or account limit on your behalf.

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
