# Review changes across files

A patch groups related text-file changes under one review. For example, a chat can update a function and its callers together, then run a command you have granted. The bridge saves a recovery receipt for each file; it does not promise an atomic transaction across the filesystem.

In the popup or side panel, open **Files, preview and restore → Changes across files**. Select the connected session whose permissions should apply. Enter operations such as:

```json
{
  "operations": [
    {"path": "files/new.rs", "content": "pub fn example() {}\n"},
    {"path": "notes/change.md", "content": "Added example.\n"}
  ]
}
```

For an existing file, also supply `expected_sha256` from a fresh `read_file` or `inspect_file` result. A missing hash means the destination must be new. **Preview batch** returns a saved patch ID, affected paths and the differences. It changes no workspace files. **Request batch apply** opens an approval containing all differences; a scoped write grant never replaces this approval.

The same flow is available to chats through `preview_patch`, `apply_patch`, `patch_status` and `restore_patch`. Patch IDs belong to one workspace and conversation. Another conversation cannot apply, inspect or restore them. Preview IDs expire after 30 minutes. Creating a fresh preview is required after expiry or rejection.

## Limits and recovery

- A preview accepts 1–20 UTF-8 file creations or replacements, with at most 512,000 bytes of new content. Each existing file must fit the 1 MiB text recovery limit.
- All changed lines must fit the 50,000-byte review. A larger difference is rejected; split it into smaller batches.
- Duplicate destinations, parent/child destinations, symlinks, protected paths and non-text files are rejected. Research dataset and agent boundaries still apply.
- The bridge checks hashes and current permissions again after approval and before each file commit. A workspace lock or a later user edit stops the operation.
- If a later file fails, earlier successful files remain changed. The result identifies committed, failed, pending and uncertain steps. A failed batch is never automatically rerun.
- **Batch status** retrieves saved receipts after reconnect. An interrupted operation is marked explicitly; inspect uncertain files and their individual recovery receipts before taking another action.
- **Request batch restore** requires a new approval and restores committed steps in reverse order. Files created by the patch move to recoverable trash. Later user changes prevent restoration rather than being overwritten.
- Restoring an interrupted or uncertain batch requires individual `restore_change` receipts. Keep the patch ID; the current panel does not yet provide a searchable list of patches.

Panel calls leave their results in the panel. They do not paste messages into a chat. Successful panel tests therefore establish the local controls, not a provider's ability to follow the protocol.
