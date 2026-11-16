# Inspect local Git changes

The 0.5.10 candidate strengthens `git_status`, `git_diff`, and `inspect_repository`. In **Files, preview and restore → Inspect local Git changes**, select a repository root, inspect its status, and choose staged or unstaged differences. An optional JSON array selects exact workspace-relative filenames. These controls are built; live browser acceptance is pending.

The default repository is the workspace root for general/code profiles and the agent's directory for research. Select an ordinary repository with its own `.git` directory. Parent discovery, bare repositories, linked worktrees, shared/alternate object stores, configuration includes and partial clones are refused. This initial subset favors explicit boundaries over supporting every Git layout.

Private-file rules apply to paths before status is returned or a diff is requested. A private file explicitly selected for review is denied. Status counts omitted protected paths without revealing their names. A clean-looking visible selection is not proof that no private changes exist. Filenames are literal; brackets and wildcard characters cannot broaden a selection.

Inspection clears inherited Git configuration/environment, disables global/system config, optional index locks, hooks, filesystem monitors, external diff/text conversion and configured clean/smudge/process filters. It does not refresh the user's index. Filtered-file comparisons can differ from a normal Git invocation. This is bounded local inspection, not an OS sandbox against a concurrent hostile process replacing repository metadata.

Limits are explicit:

- Metadata traversal: 10,000 entries and a checked two-second budget; symlinks and non-regular metadata are refused.
- Git subprocesses: 15 seconds each, bounded stdout/stderr, process-group cleanup on failure.
- Status: at most 2,000 changed paths scanned, 100 displayed, and 24,000 bytes of entry metadata. Truncation is reported.
- Diff: at most 100 selected changed paths, 16,000 bytes of path metadata and 48,000 bytes of decoded diff. Oversized reviews return an error; choose fewer files.
- Repository overview: at most 200 directory entries examined, 100 visible entries and 24,000 bytes of path metadata; README text is bounded and read without following symlinks.

Seven tests cover real staged/unstaged changes, private files, unrelated edits, unchanged index bytes, literal filenames, malicious filters, metadata boundaries and output limits. The native transport check uses a disposable repository with copied project source. It is not a provider-conversation test.

## Reviewed commits (0.5.11 candidate)

Select 1–20 existing working files, a new branch name under `bridge/`, and a commit message. **Preview new-branch commit** shows the full bounded diff, base commit, filenames, hashes, modes, message and fixed author `Unified AI Bridge <bridge@localhost>`. **Request approval for saved commit** submits that saved review; changing the form does not change a saved review. The approval itself displays the exact saved contents. Reviews expire after 30 minutes.

The bridge commits the approved **working-file bytes**, even if the staging area contains a different version. It builds a private temporary index from the original HEAD and publishes only the selected paths on a new branch. The current branch, user's index and working files remain unchanged. No checkout or push occurs. Existing unrelated committed files carry forward from the base commit; unrelated uncommitted edits stay in place.

Files, modes, policy, expiry and HEAD are checked again after approval. A final Git reference transaction verifies HEAD and creates the previously absent branch together. Existing refs, including dangling symbolic refs, cannot be overwritten. Before publishing, the host checks that the prepared commit changed only selected paths. A new child file cannot silently remove an unselected committed parent file.

The initial subset accepts UTF-8 files without NUL bytes, with a combined baseline/current budget of 512,000 bytes, a 44,000-byte diff and a 50,000-byte full review. The baseline must have an existing commit and at most 5,000 files. Deletions, binary edits, submodules, symlinks, alternate stores, linked worktrees and first commits are outside this subset. Research commits require a repository inside the agent's own directory. Content filters, hooks and signing are disabled; bytes are stored exactly as reviewed. This may differ from a project's normal filtered or signed commit workflow.

`preview_git_commit` returns a conversation/workspace-owned `review_id`. `apply_git_commit` always requests approval. `git_commit_status` reads its durable receipt after reconnect. Completed requests return the same receipt; preparing/publishing requests are never rerun automatically. If the exact intended branch/commit exists after interruption, status can record it as committed. Otherwise it reports `needs_reconciliation`. Object preparation can leave unreachable Git objects after failure; the bridge does not delete them or run garbage collection.

`git_log` returns bounded commit IDs, timestamps and subjects. Continue with the returned `base_commit` and `next_offset` to keep the history anchor stable. `git_show` reads one permitted historical UTF-8 file at a full commit ID, capped at 48,000 bytes. Current private-path rules apply to history. It does not accept shell options, arbitrary revision expressions or remote fetching.

Ten new tests cover approved commits in a dirty repository, denials, file/HEAD/policy changes, hooks/filters/signing, private history, ownership, expiry, interrupted receipts, failed publication and parent/ref conflicts. UAB-10 remains open for live browser acceptance. There is no push, merge, reset, clean or credential workflow.
