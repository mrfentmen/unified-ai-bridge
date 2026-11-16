# Creating PDF reports

The 0.5.19 candidate adds `create_document` for a deliberately small PDF report
format. The tool generates a real PDF locally in Rust. Every creation requires
explicit approval, including in Trusted mode. Browser acceptance of this
candidate is pending; the working browser installation remains 0.5.7.

```json
{
  "destination": "output/project-brief.pdf",
  "format": "pdf",
  "template": "report",
  "title": "Project brief",
  "blocks": [
    {"type": "heading", "text": "Purpose"},
    {"type": "paragraph", "text": "A short account of the project and its next steps."},
    {"type": "bullet", "text": "Review the proposed changes before applying them."},
    {"type": "page_break"},
    {"type": "paragraph", "text": "Supporting notes on a new page."}
  ]
}
```

The destination must be a new `.pdf` under an existing permitted directory.
The approval shows the complete normalized content, destination, page count,
byte count and expected output SHA256. A descriptor-anchored temporary file is
published without replacing an existing file. Path permissions are checked
before generation, after approval and immediately before publication. If the
workspace locks or a destination changes during review, the write is refused.
Research dataset and cross-agent protections still apply.

The audit chain records preparation, publication/failure and output metadata.
Raw report text is not added to the audit entry; it remains in the explicit
approval and, as with other tool requests, may be stored in the private request
database. This is creation only: it does not replace a PDF or promise binary
undo for the new file. Existing delete/trash tools remain separate.

## Supported layout

- One named `report` template: A4, fixed margins, a title, section headings,
  paragraphs, hanging bullets and numbered page footers.
- Embedded unmodified Noto Sans Regular/Bold with explicit glyph widths and
  Unicode extraction maps. [Font sources and license](../crates/bridge-native/assets/fonts/README.md).
- Simple left-to-right Latin text: printable ASCII, supported Latin characters
  through U+024F, curly quotes U+2018–U+201F, bullet, ellipsis and euro symbol.
  Soft hyphen is refused. Text is normalized to NFC; paragraph whitespace is
  collapsed within each line. Newlines in paragraphs/bullets create line breaks.
- Long words wrap by character. Titles wrap to at most three lines. Headings
  stay with the start of their following paragraph; paragraph starts/ends keep
  two lines together where possible. Explicit breaks cannot be leading,
  trailing, repeated or immediately after a heading.
- At most 240 UTF-8 bytes in a title/heading, 4,000 per paragraph/bullet,
  24,000 total text bytes, 200 blocks, 40 pages, 4 MiB output and 50,000 bytes
  of approval detail. Exceeding a bound returns an error without publishing.

Unsupported scripts, combining sequences left after normalization, emoji,
control/bidirectional characters and unknown schema fields are refused.
There is no font selection, complex shaping, kerning, rich markup, tables,
images, links, forms, attachments, scripting or external resource loading.
URLs and markup supplied as text stay literal text. This is not a claim of
PDF/A conformance, tagged-PDF accessibility or a general typesetting engine.

Generation is portable Rust; the existing `document_info`,
`extract_document_text` and `render_document_pages` PDF checks currently require
macOS. Use those tools to inspect the saved file. Saving it does not attach the
file or a preview to an AI conversation.

## Remaining work

DOCX generation/rendering, PDF conversion, existing-document edits, PPTX,
additional scripts and live browser acceptance are pending. The DOCX authoring
workflow requires rendered-page checks; its bundled renderer and LibreOffice
runtime are unavailable in this session. UAB-19 remains open.

See [verification](VERIFICATION.md) for observed native/policy/render results.
