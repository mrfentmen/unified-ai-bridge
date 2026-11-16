# Bring a downloaded chat file into your workspace

The bridge can copy a file you downloaded from a chat into the active workspace. It preserves the file's bytes, whether it is an image, PDF, document, spreadsheet, audio clip, video, ZIP, or source file. This does not make the bridge an editor for every format.

1. Use the website's normal **Download** control. A thumbnail or screenshot is not the original file.
2. Open the bridge popup or side panel and expand **Browser files → workspace**.
3. Click **Refresh browser downloads**. Select the completed file and the conversation you want to share it with. Only safe, existing, non-incognito downloads up to 64 MiB are listed, from the 50 most recent completed downloads.
4. Click **Share selected file with this session**. This grants access to that file for 24 hours; it does not paste anything into the conversation. The model can now call `list_artifacts` to get the file's opaque ID, size, type and hash.
5. Enter a new destination inside an existing workspace directory, such as `files/chart.png`, and click **Request import**. Review and approve the request. The model can also request this with `import_artifact`.
6. Check the result's destination and SHA-256. The original download stays in place. Use **Revoke sharing** to withdraw an unused grant.

The selected conversation's filesystem permissions apply. A share does not cross into another conversation or workspace. In the research profile, use the selected agent's own folder, or an otherwise permitted destination. Read-only mode blocks imports. A model cannot register a file, browse Downloads, or provide its own source path or URL.

## What the result means

The host reads the selected file, checks its size and hash, requests approval, then rechecks the source, destination and current policy before copying. It refuses symlinks, hard-linked sources, private-file exclusions, overwrites and changed downloads. Source paths and download URLs stay out of model results and audit entries. Provenance contains only the download's hostname, not a signed URL or its query parameters.

Imports are recorded in SQLite and the append-only audit chain. An already imported ID cannot create a second copy. An `importing` receipt left by an interruption means the outcome needs inspection; it will not be retried automatically. Check the recorded destination and hash before downloading and sharing a new copy. Expired or revoked grants require a new download. There is a limit of 100 shares per session.

Files are streamed locally; binary payloads are not pasted into chat or carried in giant Native Messaging messages. Existing text edit and recovery limits remain 1 MiB. Imports do not provide large-binary undo, unpack archives, run programs, render untrusted content, or upload files back into chats.

## Limits of this first step

This is a user-selected browser-download importer. It does not automatically discover a provider's generated files or press its download buttons. Support for a file's bytes is separate from support for inspecting or editing that format. Provider file-generation capabilities and account limits still apply.

Chrome's Downloads permission is required. After upgrading, reload the extension and check that the card, panel, and native host show the same build. macOS may separately request permission for the host to read Downloads or write to the workspace.
