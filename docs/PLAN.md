# What comes next

The bridge should make everyday work in local folders dependable before taking on more kinds of applications. The existing host, protocol, and extension stay in place; this is an incremental project.

## Already implemented

- Rust/WASM adapters for ChatGPT, Claude, DeepSeek, Gemini, Grok, Qwen, and Kimi, including a separate Grok-on-X path.
- Host-issued identities, bounded tool requests, workspace permissions, approval prompts, and a hash-chained audit log.
- Research, general, and code profiles; workspace switching with updated conversation context and in-flight workspace binding.
- File previews, hash-checked edits, append/copy/glob/tree/batch-read/diff tools, recoverable deletion, and restore history.
- Fixed command grants, bounded output and timeouts, plus reviewed native stdio MCP server discovery and calls.
- A popup, side panel, in-page controls, activity filters, tool runner, and installation diagnostics.
- Strict-mode approval across profiles, no-overwrite file moves, approval-wait revalidation, and regression tests for concurrent file/policy changes.
- GitHub workflows for tests, isolated extension/native packaging, and publication of tagged releases after checks pass.

Implementation is not the same as acceptance. [Verification](VERIFICATION.md) records which behavior has actually been exercised.

## First: finish the reliability gate

Complete Grok's real create-file loop when account access permits. Repeat the live matrix on the current build for the older providers, then exercise simultaneous calls, interrupted host connections, approval controls, stale results, and draft preservation. Add reproducible adapter regressions for website changes.

Qwen's background rendering and candidate selection need clear, predictable handling. Retire content-script timers cleanly after extension reloads. Keep build/package/loaded-version checks explicit.

## Then: improve ordinary file and coding work

Add multi-file patch preview and recovery without promising filesystem-wide atomicity. Improve large-file streaming and private-file exclusion controls. Add command progress, cancellation, and bounded polling while preserving explicit executable/argument grants.

Expand the 0.4.7 manual preview/apply/delete/restore acceptance to every approval surface and model-driven result delivery. Add session-scoped permission grants only with explicit revocation and expiry semantics.

## Then: expand integrations carefully

Verify real third-party native MCP servers, tool collisions, disconnections, schema changes, and oversized results. Add persistent integration health and image delivery separately from simply saving screenshot files.

Desktop input needs separate observation/input permissions and a Stop control. Browser actions should target a selected tab and visible page state. PDF/Office text extraction comes before format-specific editing. Policy-gated web fetching remains planned and should start disabled.

The default runtime remains Rust. No arbitrary shell, provider APIs, hidden endpoints, copied browser profiles, or automatic server installs by a model. External processes retain OS-level permissions; the bridge must make that boundary clear.

## Distribution and platform work

Test a physical Intel Mac and establish an actual minimum macOS version. Document Chrome's OS support separately from Rust's build targets. Improve initial setup so a new user can choose a general workspace immediately, then address signing and packaging rather than repeatedly relying on unsigned local builds.

## Contributing

Small fixes with a concrete reproduction are welcome. For a new tool, include its catalog/schema, permission decision, implementation, audit behavior, instructions, and both allowed and denied tests. For an adapter, record the loaded build and a real browser/host/file/result round trip; DOM fixtures alone are not a live integration result.

Keep private transcripts and operational logs local. The repository intentionally excludes the development evidence archive, runtime configuration, databases, and generated extension artifacts.
