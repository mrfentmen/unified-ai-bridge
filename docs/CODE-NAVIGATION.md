# Find code and follow compiler errors

The 0.5.9 candidate adds a Rust declaration reader and strengthens existing text search. These tools read source files; they do not rename symbols automatically, start a language server, run project macros, or claim that a literal match refers to the same symbol.

In **Files, preview and restore → Code navigation**, enter a folder and query to search, or a `.rs` file to list Rust declarations. Result buttons read a bounded excerpt around the reported line. Continuation buttons request the next page. Compiler locations from manual tool results appear in the same workbench.

These controls have built successfully but are not yet verified in a live Chrome session. The normal browser package remains 0.5.7; 0.5.9 is an isolated candidate.

Intel CI later exposed excessive repeated policy compilation during search in 0.5.9. The 0.5.10 candidate reuses the compiled policy within a single call while recomputing path resolution for every file. It never caches policy across calls or workspace switches. The local five-check navigation suite passes, and run 43938573686 passed Linux and Intel macOS tests with the original 100-file assertion intact.

## Literal search

`search_files {query, path?, max_results?, cursor?}` returns structured matches with `path`, `line`, `excerpt` and the file's `sha256`. Matching is case-insensitive literal text. Use the hash when preparing a patch; inspect each caller before changing it.

Pass a returned `next_cursor` unchanged as `cursor`. Continuation within a file verifies its hash and refuses a changed file. The cursor is a location in a live tree, not a snapshot of the entire repository: newly added or moved earlier files require a fresh search. If `tree_truncated` is true, narrow the folder rather than assuming another page covers omitted tree entries.

Each call is limited to 100 readable files, 8 MiB of file data, 5,000 tree entries, depth 12, and a two-second budget checked between filesystem and line operations. Individual files are limited to 1 MiB. Excluded entries count toward the traversal budget. Unreadable, binary, oversized and protected files are skipped and reported; symlinks are not followed. `.git`, `.bridge`, build output and dependency directories are excluded.

Results contain at most 100 matches and about 48 KiB of structured match data. Excerpts are limited to 300 characters. `truncated`, `skipped_files`, `tree_truncated`, and the returned limits matter: an empty bounded page is not proof that a symbol is absent from the project. Filesystem calls themselves are not preempted by the two-second check.

## Rust syntax declarations

`list_symbols {path, max_results?, offset?, expected_sha256?}` supports Rust `.rs` files up to 1 MiB. It reports functions, types, modules, constants, statics, enum variants, traits, methods and named macros with their source line and column. It visits inline modules and associated declarations; function-local declarations are not indexed. At most 1,000 declarations are retained, with up to 100 and roughly 24 KiB returned per page.

Use `next_offset` with the returned SHA-256 as `expected_sha256`. The tool refuses stale continuation. Unsupported languages return an explicit error directing the caller to `search_files`; no invented semantic results are returned.

Parsing uses pinned `syn` 2.0.119 with real source spans. It is syntax-only:

- Comments and strings are not mistaken for declarations.
- Macro definitions can be listed, but macros are neither executed nor expanded.
- Conditional declarations are included without evaluating `cfg`.
- External module files, types, imports and references are not resolved.
- Invalid or incomplete Rust reports a parse error and suggests text search.

The same Rust native executable runs an internal parser worker with bounded stdin/stdout, two processing slots, a two-second CPU limit and a three-second wall timeout. Core dumps are disabled. A parser crash or timeout returns an error while the host remains available. The worker reads only supplied source bytes; it does not load the bridge configuration or open project files. This isolation is for parser reliability, not an OS sandbox against a compromised native executable.

## Compiler diagnostics

`run_command` results and `inspect_test_results` now include structured locations from Cargo JSON compiler messages and rustc JSON diagnostics. For a user-approved Rust check, use Cargo's `--message-format=json` or rustc's `--error-format=json` in the fixed command grant.

Only locations that resolve to readable files inside the workspace become links. Missing files, traversal, external paths, private files and invalid coordinates are omitted. Messages, spans, result count and JSON output are bounded. Plain text and other compiler formats remain available as the original command/log output; they are not misrepresented as parsed diagnostics. The bridge still does not evaluate compiler fix suggestions automatically.

A real test task locates a function, finds its caller, updates both through an approved `preview_patch` / `apply_patch`, then passes a granted rustc metadata check. A deliberate type error returns `E0308` at the actual source location. This verifies the local coding workflow, not an AI-provider conversation.

UAB-09 remains open for live browser acceptance and semantic references through a separately verified parser or configured native language server. There is no `find_references` tool in this candidate.
