# Read DOCX body text

The 0.5.13 candidate adds `document_info` and `extract_document_text` for a declared DOCX subset. In **Files, preview and restore → Read DOCX text**, inspect the file, read its body text and continue with the returned byte offset. Continuation requires the original SHA-256; a changed document refuses the next chunk.

This reader does not render pages or edit Word files. PDF, legacy DOC, macro-enabled DOCM, OCR and page rendering return an explicit unavailable result. Word layout is not stored as a reliable page map, so the response leaves `page_count` null. Explicit page-break counts are reported separately and must not be treated as page counts.

## What is extracted

- Main-body paragraphs in package order, with tabs and line breaks. Table-cell paragraphs are flattened into the same text stream.
- Text inside inserted/moved-to tracked changes; deleted/moved-from text is omitted. A tracked-changes flag records that this choice was made.
- Cached field-result text and hyperlink labels. Field instructions are not evaluated and link targets are never opened.
- XML entities such as `&amp;`, with normal Unicode text preserved. DTDs and custom/external entity definitions are refused.

Formatting, numbering/bullet markers, hidden-text visibility and exact reading layout are not reproduced. Alternate-content branches are skipped and reported to avoid duplicating competing representations. Headers, footers, footnotes, endnotes, comments and embedded objects are listed as omitted parts. Images are counted but neither decoded nor sent to a model. An empty text result does not establish that the document is visually empty.

The current subset requires UTF-8 XML, the transitional WordprocessingML namespace and a non-macro main part at `word/document.xml`. Macro-bearing packages are refused. Strict-namespace documents, other encodings or unusual package structures may be unsupported. No package relationship is resolved; external references remain inert even if their XML names a file or network URL.

## Bounds and isolation

The host reads an approved regular workspace file through the existing descriptor-based reader. Symlinks, hard links, traversal and private paths remain denied. It sends bytes to a disposable worker in the same native executable; the worker does not load bridge configuration or receive filesystem paths.

- Package: 16 MiB input, ZIP32 only, at most 256 entries and 32 MiB declared expansion. Shared ZIP checks reject traversal, name collisions, encryption, links and unsupported compression. Nothing is extracted to disk.
- Main XML: 2 MiB, 50,000 nodes, nesting at most 128. Content-type XML is capped at 128 KiB.
- Extracted body: at most 5,000 paragraphs and 256,000 UTF-8 bytes.
- Result: at most 24,000 requested text bytes and 48,000 serialized JSON bytes per chunk; use the returned offset when a smaller chunk is necessary.
- Worker: two slots, two CPU seconds, four wall seconds, bounded input/output, core dumps disabled. A parser crash or timeout returns an error while the host stays available. This is reliability isolation, not an OS sandbox for arbitrary native programs.

`max_bytes` defaults to 12,000 and accepts 256–24,000. `offset` is a UTF-8 byte offset in the extracted text, not an offset in the ZIP file or a Word page number. Use only returned offsets.

Five tests exercise Unicode/multiline chunks, omitted parts, revisions, cached fields, inert relationships, hostile XML/packages, stale input, protected paths and response limits. A public DOCX sample is also compared with macOS textutil's independent text extraction. Neither test establishes layout fidelity or browser integration.

UAB-16 remains open for PDF support, rendered-page inspection, additional Word content and live browser acceptance. No rendering button or unsupported PDF tool is presented as working.
