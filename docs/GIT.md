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

UAB-10 remains open: log/show and an explicitly approved new-branch/commit workflow are still planned. Inspection grants no push, merge, reset, clean or credential access.
