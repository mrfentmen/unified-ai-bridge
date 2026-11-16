This candidate reads a bounded DOCX main-body subset in an isolated native worker. It reports omitted content, returns hash-checked text chunks, refuses macros/DTDs and opens no external references. It does not invent Word page numbers. PDF, OCR, rendering and Word editing remain pending.

The workspace suite passed **171 tests**, and the isolated installed host passed **41 self-tests**. Native Messaging extracted Apple and Banana from a public DOCX sample, matching an independent macOS textutil read. Private-file, stale-document and unsupported-PDF requests returned explicit errors. Doctor verified all 13 audit entries. Five dedicated document checks cover parsing, omissions, isolation, limits and denials.

Reviewed Git commits, history, tables, binary recovery and code navigation from the previous candidates are included. CI run 43938573686 fully passed Linux/Intel tests and Intel packaging. The 0.5.11 candidate also passed both test platforms; its packaging was still running at this observation.

**Live Chrome acceptance is pending.** The candidate is built in isolation while the normal Chrome package and installed host remain 0.5.7. Browser automation is unavailable in this session. [Document support](https://github.com/mrfentmen/unified-ai-bridge/blob/main/docs/DOCUMENTS.md) · [verification](https://github.com/mrfentmen/unified-ai-bridge/blob/main/docs/VERIFICATION.md).
