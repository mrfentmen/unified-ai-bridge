# Bring a downloaded chat file into your workspace

The bridge can copy a file you downloaded from a chat into the active workspace. It preserves the file's bytes, whether it is an image, PDF, document, spreadsheet, audio clip, video, ZIP, or source file. This does not make the bridge an editor for every format.

1. Open the bridge popup or side panel and expand **Browser files → workspace**. Choose the connected conversation.
2. Click **Discover generated files in conversation**, choose a file and click **Request generated-file download**. Review the approval. You can also use the website's normal **Download** control. A thumbnail or screenshot is not the original file.
3. Click **Refresh browser downloads**. Select the completed file and the conversation you want to share it with. Only safe, existing, non-incognito downloads up to 64 MiB are listed, from the 50 most recent completed downloads.
4. Click **Share selected file with this session**. This grants access to that file for 24 hours; it does not paste anything into the conversation. The model can now call `list_artifacts` to get the file's opaque ID, size, type and hash.
5. Enter a new destination inside an existing workspace directory, such as `files/chart.png`, and click **Request import**. Review and approve the request. The model can also request this with `import_artifact`.
6. Check the result's destination and SHA-256. The original download stays in place. Use **Revoke sharing** to withdraw an unused grant.

When a supported explicit download link is available, the bridge uses Chrome's returned download ID and shares that exact completed file automatically after the download approval; skip steps 3–4. When the website only exposes a button, the result says `manual_share_required`; complete steps 3–4. Chrome does not give extensions a download's originating tab ID, and an origin-only referrer cannot identify a conversation. The bridge never guesses from a matching filename or the newest download.

The selected conversation's filesystem permissions apply. A share does not cross into another conversation or workspace. In the research profile, use the selected agent's own folder, or an otherwise permitted destination. Read-only mode blocks downloads and imports. A model cannot register a file, browse Downloads, or provide its own source path or URL.

## Ask the model to download a generated file

`list_generated_files {}` returns names, source response IDs and short-lived handles from rendered assistant file controls. `request_download {"handle":"…"}` asks for approval and returns either a shared artifact or `manual_share_required`. Follow that result before calling `list_artifacts` and `import_artifact`. The normal per-task and per-response budgets apply to these tools too.

Handles expire after five minutes and are bound to the session, workspace, conversation and task. The panel binds its handles to the current user message instead. Navigating, starting a different task, replacing the file control or disconnecting invalidates a handle. Download claims are persisted before the browser action, across host and extension restarts. An interrupted or timed-out attempt is not retried automatically, even after rediscovery; inspect Chrome's downloads and explicitly share the file if it finished. Paused tasks, locked workspaces and changed sessions fail closed.

## What the result means

The host reads the selected file, checks its size and hash, requests approval, then rechecks the source, destination and current policy before copying. It refuses symlinks, hard-linked sources, private-file exclusions, overwrites and changed downloads. Source paths and download URLs stay out of model results and audit entries. Provenance contains only the download's hostname, not a signed URL or its query parameters.

Imports are recorded in SQLite and the append-only audit chain. An already imported ID cannot create a second copy. An `importing` receipt left by an interruption means the outcome needs inspection; it will not be retried automatically. Check the recorded destination and hash before downloading and sharing a new copy. Expired or revoked grants require a new download. There is a limit of 100 shares per session.

Files are streamed locally; binary payloads are not pasted into chat or carried in giant Native Messaging messages. Existing text edit and recovery limits remain 1 MiB. Imports do not provide large-binary undo, unpack archives, run programs, render untrusted content, or upload files back into chats.

## Discovery limits

Current live evidence covers ChatGPT card discovery, stale-handle refusal, approval denial and an approved website-button download. The real downloaded SVG also passed an installed-native-host import/hash check. The complete browser selection/import round trip was interrupted by changed browser state. Direct-link automation has automated transport/policy coverage but still needs a real-provider acceptance run. See [verification](VERIFICATION.md#generated-file-controls-0526--november-17-2026).

Discovery recognizes ChatGPT's rendered file cards and explicit `a[download][href]` links inside supported providers' assistant replies. It scans at most 30 rendered replies and returns at most 40 files. Hidden/virtualized files, image-only controls, ambiguous same-named cards and provider-specific export menus may require manual download. Other providers' generated-file adapters are not verified by their ordinary read/write tool tests.

Direct links are restricted to HTTPS provider origins, ChatGPT's file origin, or their blob URLs. Source URLs stay inside the extension; only selected-file metadata goes to the host. Direct downloads are observed for at most 40 seconds; an interrupted, oversized, unsafe or still-pending file is not shared. Chrome can still leave a partial/completed file in Downloads. The 64 MiB limit is enforced again by the native importer; the bridge does not promise a byte-exact network transfer cutoff.

Support for a file's bytes is separate from inspecting/editing its format. Provider file-generation capabilities, quotas, foreground rendering requirements and normal macOS permissions still apply. Uploading local files/images back to a chat remains separate work.

Chrome's Downloads permission is required. After upgrading, reload the extension and check that the card, panel, and native host show the same build. macOS may separately request permission for the host to read Downloads or write to the workspace.
