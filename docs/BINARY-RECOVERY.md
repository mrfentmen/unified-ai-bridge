# Recover larger file changes

The 0.5.8 candidate extends the existing `list_changes` / `restore_change` journal to files up to **64 MiB**. The native operations and packaged transport have been exercised with real files. The new Chrome controls have compiled but have **not yet been verified live**; the working browser installation remains on 0.5.7.

`replace_file` replaces one existing regular file with another workspace file. It requires the SHA-256 of both files and always requests a fresh approval. `copy_file` keeps its existing create-only, 1 MiB contract. `delete_file` uses the larger recovery path when its target exceeds 1 MiB. Binary restores always require approval as well.

```json
{
  "tool": "replace_file",
  "arguments": {
    "source": "files/revised.pdf",
    "destination": "files/report.pdf",
    "source_sha256": "SHA256 returned for revised.pdf",
    "expected_sha256": "SHA256 returned for report.pdf"
  }
}
```

Use `calculate_sha256` for those hashes. The source must remain unchanged while approval is pending. Traversal, protected/private paths, hard links, workspace locks and research-agent restrictions remain enforced. No command is executed. New file inodes use private 0600 permissions; executable bits and extended attributes are not copied.

## Panel controls

Under **Files, preview and restore → Replace or trash a binary file**, enter both paths and use the hash buttons. **Request binary replacement** and **Request binary trash** open the existing approval flow. **Recovery history → Request restore** uses the common journal.

**Binary recovery storage** shows storage use and settings for the active workspace. Only trusted extension controls can alter these settings; a model cannot create a larger budget or erase its recovery history.

| Setting | Default | Allowed range |
|---|---:|---:|
| Storage budget | 512 MiB per workspace | 64–4096 MiB |
| Private snapshot count | 1,000 | 1–2,000 |
| Retention age for cleanup eligibility | 30 days | 1–365 days |
| Free disk reserve | 64 MiB | 64–4096 MiB |

Storage accounting includes the private binary snapshots and workspace trash, including older text-trash entries. Existing SQLite text backups and checkpoint files are not migrated or managed by these controls. Quotas are checked again after approval under a cross-process storage lock; exceeding them refuses the new operation. Lowering a budget does not delete existing data. Disk checks are conservative checks at the time of the operation, not a reservation against unrelated programs filling the disk.

Retention **does not automatically delete anything**. Inspect storage, review the expired entries, then explicitly choose **Delete expired backup** and confirm. Cleanup removes that private snapshot and retained original, preserves the current file and audit records, and marks the receipt `backup_removed`. It checks hashes again and refuses uncertain, changed or corrupt backups. Cleanup is irreversible for that recovery entry. Changes younger than the configured retention age cannot be cleaned through this control.

## What happens during replacement

The host copies the expected original bytes into a private 0600 snapshot under its application home. The snapshot is independent of the original inode, and neither its contents nor host path is returned to the model.

After persisting intent, the host moves the original inode into `.bridge/trash/<change-id>` using held directory descriptors and a no-replace rename. It verifies the moved bytes, then publishes the prepared replacement only if the destination is still absent. This is a **journalled detach/publish operation**, with a brief interval when the destination is missing. It is not a filesystem-wide transaction or an atomic compare-and-replace against other editors.

An independent editor creating a new file during that interval is preserved. A failure leaves the original and snapshot available and records `needs_reconciliation`; a process crash may leave `prepared` or `original_detached`. Do not retry those mutations or clean their backups automatically. Inspect the receipt and files before manual reconciliation. The current UI exposes their state through Recovery history; automatic reconciliation is not implemented.

A normal restore checks the original post-change hash before approval and again afterward. It refuses later edits. The restore itself creates a new recovery receipt. Corrupt snapshots fail SHA-256 verification. An interrupted cleanup is marked `cleanup_pending` and also requires inspection; it is not silently retried.

One binary recovery operation may hold the processing slot at a time. Additional calls return a busy error rather than queue indefinitely. Snapshot and source reads are capped at 64 MiB; private storage enumeration is bounded. Unknown entries or abandoned cleanup directories stop automatic storage operations for inspection. As with all local tools, these checks are not a sandbox against a hostile process running as the same OS user.

## Recorded verification

Ten dedicated tests cover larger replace/trash/restore, approvals, stale hashes, corrupt snapshots, cleanup/corruption during approval, workspace locks, protected paths, hard links, storage limits, expired cleanup, independent editors and interruption after detach. The full suite passed **139 tests**, with one deliberately slow test ignored.

An isolated installed 0.5.8 host also replaced two actual extension WASM builds through Native Messaging, restarted, restored the original, trashed/restored it, and rejected a later edit. The two files were 1,419,644 and 1,467,238 bytes. Four committed receipts and 36 valid audit entries were checked. This establishes native transport and disk behavior, not a provider conversation or Chrome control test. See [verification](VERIFICATION.md) for the remaining browser gate.
