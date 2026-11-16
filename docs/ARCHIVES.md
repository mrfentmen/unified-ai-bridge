# Work with ZIP files

The bridge can inspect a ZIP, extract it into a new folder, or create a ZIP from files you select. It never runs archive contents or replaces an existing destination. Creation and extraction each require approval, even with a scoped file-write grant.

Open **Files, preview and restore → ZIP archives** in the popup or side panel:

1. Enter a workspace ZIP path and choose **Inspect ZIP**. The result lists entries and the archive SHA-256.
2. Enter a new destination folder under an existing parent, then choose **Request ZIP extraction**. Review the entry list and approve. The folder appears only after all entries pass validation and are written successfully.
3. To create an archive, enter a JSON array of regular file paths, for example `["files/main.rs", "notes/readme.md"]`, and a new ZIP destination. Choose **Request ZIP creation** and review the exact source paths and hashes.

Chats use the same tools: `list_archive {path}`, `extract_archive {path, destination, expected_sha256}`, and `create_archive {paths, destination}`. The extraction hash comes from `list_archive`; changes during approval stop the operation. Creation records and rechecks source hashes before building the ZIP. File names inside a created archive retain their workspace-relative paths.

## Supported subset and limits

- ZIP32 with Stored or Deflate compression, one unambiguous end record, and no executable prefix. ZIP64, encrypted, split, ambiguous and unsupported-compression archives are rejected explicitly.
- 1–256 entries, at most 32 MiB compressed, 16 MiB per file and 64 MiB expanded. Processing after approval has a 15-second budget.
- Paths up to 12 components and 240 bytes; all names together must fit 8192 bytes. Oversized header fields and review text are rejected.
- Creation takes an explicit list of regular files. It does not recursively sweep a directory. Select the intended files using the file tree or glob tools first.
- Traversal, absolute/drive paths, symlinks, special files, duplicate names, case/Unicode collisions and file/directory conflicts are rejected. Workspace policy and private-file exclusions also apply. Git internals and bridge metadata are excluded.
- Extraction validates actual expanded sizes and CRCs. Metadata inspection alone does not prove that entry data is intact.
- Files receive no executable permission bits. Nested archives stay ordinary files; they are not recursively unpacked. No executable, macro, script or installer is opened.

## Failure and recovery

Writes stage under a private temporary name in the destination's parent. Publication refuses any existing destination, including a dangling symlink. On failure, cleanup removes only entries created by that operation. If another local writer interferes with cleanup, the error identifies the staging name and records that cleanup is required; it never recursively deletes unknown files.

Audit records distinguish preparation, completion, failed cleanup, and publication with an uncertain final receipt. A hard process kill may leave a staging directory and a preparation record. Inspect these before retrying. The bridge never resumes extraction automatically.

Sources remain unchanged. Archive operations do not yet integrate with large-binary undo or delete an extracted directory as one recoverable change. These limits are separate from text-file patch recovery. Ordinary browser-download imports still require selecting and sharing the completed download before the ZIP is available inside the workspace.
