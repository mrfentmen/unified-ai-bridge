This candidate adds bounded CSV/TSV reads and structured queries in every workspace profile. Select columns, page with source-hash checks, filter records and calculate approximate numeric summaries with explicit skipped-cell counts. Formula-looking text remains data. XLSX and workbook edits are still pending.

The workspace suite passed **166 tests**, and the isolated installed host passed **41 self-tests**. Native Messaging read 19 CSV records from actual repository history, continued at the correct record, and returned independently checked query counts and sum. Private-file and stale-page requests were refused. Doctor verified all 13 audit entries. Five dedicated table checks cover parsing, projection, aggregate semantics, limits and denials.

Reviewed Git commits, history, binary recovery and code navigation from the previous candidates are included. The 0.5.10 search correction passed Linux and Intel macOS tests in CI run 43938573686. Its Intel packaging was still running at this observation.

**Live Chrome acceptance is pending.** The candidate is built in isolation while the normal Chrome package and installed host remain 0.5.7. Browser automation is unavailable in this session. [Table support](https://github.com/mrfentmen/unified-ai-bridge/blob/main/docs/TABLES.md) · [verification](https://github.com/mrfentmen/unified-ai-bridge/blob/main/docs/VERIFICATION.md).
