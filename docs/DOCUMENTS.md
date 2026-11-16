# Read documents and preview PDF pages

The 0.5.15 candidate extends `document_info` and `extract_document_text` from DOCX body text to PDF embedded text on macOS, and adds approved `render_document_pages` previews. In **Files, preview and restore → Read documents and preview PDF pages**, inspect the file, read text and continue with the returned byte offset. Continuation requires the original SHA-256; a changed document refuses the next chunk. Live browser acceptance is pending; the normal installation remains 0.5.7.

Legacy DOC, macro-enabled DOCM, OCR and Word layout rendering remain unavailable. Word layout is not stored as a reliable page map, so DOCX responses leave `page_count` null. Explicit page-break counts must not be treated as page counts. No document editor is implied by these readers.

## DOCX body subset

- Main-body paragraphs in package order, with tabs and line breaks. Table-cell paragraphs are flattened into the same text stream.
- Text inside inserted/moved-to tracked changes; deleted/moved-from text is omitted. A tracked-changes flag records that this choice was made.
- Cached field-result text and hyperlink labels. Field instructions are not evaluated and link targets are never opened.
- XML entities such as `&amp;`, with normal Unicode text preserved. DTDs and custom/external entity definitions are refused.

Formatting, numbering/bullet markers, hidden-text visibility and exact reading layout are not reproduced. Alternate-content branches are skipped and reported to avoid duplicating competing representations. Headers, footers, footnotes, endnotes, comments and embedded objects are listed as omitted parts. Images are counted but neither decoded nor sent to a model. An empty text result does not establish that the document is visually empty.

The current subset requires UTF-8 XML, the transitional WordprocessingML namespace and a non-macro main part at `word/document.xml`. Macro-bearing packages are refused. Strict-namespace documents, other encodings or unusual package structures may be unsupported. No package relationship is resolved; external references remain inert even if their XML names a file or network URL.

## DOCX bounds and isolation

The host reads an approved regular workspace file through the existing descriptor-based reader. Symlinks, hard links, traversal and private paths remain denied. It sends bytes to a disposable worker in the same native executable; the worker does not load bridge configuration or receive filesystem paths.

- Package: 16 MiB input, ZIP32 only, at most 256 entries and 32 MiB declared expansion. Shared ZIP checks reject traversal, name collisions, encryption, links and unsupported compression. Nothing is extracted to disk.
- Main XML: 2 MiB, 50,000 nodes, nesting at most 128. Content-type XML is capped at 128 KiB.
- Extracted body: at most 5,000 paragraphs and 256,000 UTF-8 bytes.
- Result: at most 24,000 requested text bytes and 48,000 serialized JSON bytes per chunk; use the returned offset when a smaller chunk is necessary.
- Worker: two slots, two CPU seconds, four wall seconds, bounded input/output, core dumps disabled. A parser crash or timeout returns an error while the host stays available. This is reliability isolation, not an OS sandbox for arbitrary native programs.

`max_bytes` defaults to 12,000 and accepts 256–24,000. `offset` is a UTF-8 byte offset in the extracted text, not an offset in the ZIP file or a Word page number. Use only returned offsets.

Five tests exercise Unicode/multiline chunks, omitted parts, revisions, cached fields, inert relationships, hostile XML/packages, stale input, protected paths and response limits. A public DOCX sample is also compared with macOS textutil's independent text extraction. Neither test establishes layout fidelity or browser integration.

## PDF text and previews on macOS

The native host uses Rust bindings to system PDFKit for embedded text and CoreGraphics for page-content rendering. Linux returns `unsupported` for PDF operations; DOCX stays available. No helper interpreter, external PDF executable or provider API is installed. The APIs used are available on Intel and Apple Silicon macOS, but a physical old Intel Mac and the final minimum supported OS remain release checks.

`document_info` reports physical page count, crop-box dimensions in points and rotation. It does not scan for text or declare that a document is scanned. `extract_document_text` returns the embedded text in physical page order. Each chunk includes `page_ranges`: one-based physical page numbers with UTF-8 start/end byte offsets **within the returned text**. `pages_without_text` identifies pages with no extractable text; these can be scans, vector outlines, blank pages or unsupported encodings. This is not OCR. PDFKit's reading order and hidden-text visibility can differ from the visible page.

`render_document_pages` accepts the inspected source hash, a new destination folder, `first_page`, optional inclusive `last_page`, and `max_dimension` (default 1200). It creates `page-0001.png` and similar physical-page names after explicit approval. Crop and rotation are honored, aspect ratio preserved, with an opaque white background. CoreGraphics renders page content only: annotations, interactive form appearances, attachments and active content are omitted. It never opens links, runs PDF actions/JavaScript, or follows an external document URL. This preview subset is not a complete interactive PDF viewer.

Source hashes and current path/workspace policy are checked again after approval and rendering. Every output uses an isolated staging folder and create-only publication. Existing destinations, changed sources, private paths, research datasets, another research agent's folder, symlinks and hard links are refused. Approval denial writes no output. The receipt names each PNG, its dimensions and SHA-256; creation is audited. Saving previews does **not** deliver an image to a chat or establish model visual understanding.

PDF bounds:

- Input: 16 MiB, 1–200 physical pages; encrypted or copy-restricted PDFs are refused. No password workflow.
- Text: 256,000 UTF-8 bytes across the document; chunks use the same 256–24,000 requested byte and 48,000 serialized-result budgets as DOCX. Oversized text fails explicitly rather than omitting later pages.
- Rendering: 1–4 consecutive pages per request, 128–1600 pixels on the longest side, no page dimension over 20,000 points, 16 MiB total PNG output. Native decoder working memory is not an independently enforced RSS limit.
- Worker: two shared document slots, six CPU seconds, ten wall seconds, bounded pipes and disabled core dumps. Crash/timeout becomes a tool error; the host remains available.
- The PDF worker receives bytes, not a workspace path or bridge config. A macOS Seatbelt policy denies network access, file writes and subprocess execution; file-content reads are limited to system libraries/resources and fonts. If restrictions cannot be installed, PDF parsing is refused. This uses the deprecated system sandbox API; future macOS removal would produce an explicit unavailable result. It does not claim a complete exploit-proof sandbox or protect arbitrary external programs.

Six PDF integration tests cover text pagination/page attribution, empty pages, crop/rotation/pixel results, source preservation, approvals and policy changes, malformed/encrypted/oversized input, protected research destinations and links. A separate worker test checks actual filesystem/network denials. Public-document evidence and rendered-page inspection are recorded in [verification](VERIFICATION.md).

UAB-16 remains open for live browser acceptance, OCR, DOCX rendering and additional Word content.
