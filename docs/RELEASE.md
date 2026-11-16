This patch fixes two permission and file-preservation bugs:

- A file created at a move's destination while approval was pending could be overwritten. Moves now recheck paths, source contents and current policy after approval, and use an atomic no-replace rename.
- Creating folders in a research workspace could skip Strict-mode approval. Strict now applies consistently across profiles while dataset and cross-agent restrictions remain hard denials.

Related fixes protect create-only writes and trash moves from destination replacement. Folder creation rechecks policy and paths after approval; screenshots are captured privately, then published only after revalidation with no-overwrite semantics.

Regression tests use real temporary files, including concurrent destination creation, symlink swaps, source edits and policy changes. CI runs workspace tests and self-tests on Linux and macOS, plus the pinned Oxichrome build and isolated native-host installation. Tagged releases publish only after these checks pass.

Build from source following [the installation guide](https://github.com/mrfentmen/unified-ai-bridge/blob/main/packaging/DEPLOY.md). Reload the extension and provider tabs after deployment, then check that the Chrome card, panel and host hello all report 0.4.7. Existing provider verification and limitations are recorded in [the verification guide](https://github.com/mrfentmen/unified-ai-bridge/blob/main/docs/VERIFICATION.md); this patch does not claim new provider acceptance or physical Intel Mac testing.
