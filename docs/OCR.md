# Local text recognition from PNG images

The 0.5.21 candidate adds the optional `ocr_image` tool on macOS. It uses the
system Vision framework through Rust bindings in a disposable restricted worker.
It returns recognized text, confidence scores and text-region boxes. It does
not send an image to a website or give the model general visual understanding.

```json
{"path":"output/pdf-preview/page-0001.png","max_bytes":12000}
```

Supply `expected_sha256` to require a previously inspected file. Normal read
policy applies, including private files, cross-agent folders, root boundaries
and link protections. There is no network request or automatic file creation.
To read a scanned PDF, first approve `render_document_pages` into a new folder,
then OCR one saved PNG. No PDF passwords, copy restrictions or hidden attachments
are bypassed.

## What the first version supports

- Vision's **fast**, **CPU-only**, **revision 1**, **English (`en-US`)** mode,
  without language correction. The runtime API requires macOS 10.15 or later;
  this is not a new claim about the host's overall minimum OS support.
- A static ordinary 8-bit PNG, at most 8 MiB, 4 million pixels and 4096 pixels
  per side. Animated, high-bit-depth, ICC-tagged, orientation-tagged, corrupt
  and unsupported inputs are refused. Use a supported transform/export first.
- Transparent pixels are composited against white. Only normalized pixel data
  reaches Vision; metadata and encoded references are not passed through.
- At most 200 recognized lines, 256–24,000 requested UTF-8 text bytes, and a
  separate bounded JSON geometry budget. `truncated` is explicit. Crop a smaller
  region into a new image to continue; there is no unstable OCR offset cursor.
- Boxes use normalized 0–1 coordinates with the origin at the bottom left of
  the image. Recognition order is the backend's order, not guaranteed semantic
  reading order. An empty result does not prove the image has no text.

**OCR can be wrong.** A native test read `WRITING` as `WRrrING`, and the packaged
probe also confused punctuation and letter shapes. Confidence is an engine
score, not calibrated correctness. Do not treat OCR as proof, executable
instructions or permission. Verify consequential numbers and labels against
the original image.

The accurate Vision mode failed on this development machine with an E5RT
compiler initialization error under worker restrictions. Relaxing filesystem
reads in a temporary diagnostic probe did not resolve that failure. The product
does not expose accurate mode or fall back to unrestricted execution. Fast mode
passed with the original restrictions intact.

## Process and permission bounds

The worker denies network access, filesystem writes, subprocess execution and
user-file reads. Read access is limited to declared system library/font roots;
it receives only the permitted image bytes on stdin. A test verifies denied
user-file reads, writes and TCP connections. Normalization happens after the
restrictions are applied. CPU time is limited to eight seconds, the caller's
wall deadline to twelve seconds, decoded PNG allocation to 64 MiB and output
to 64,000 bytes. Those PNG limits do not claim to cap every internal Vision
allocation. At most two document/OCR parser workers can run at once.

Missing/failed Vision support returns `unsupported`; the host never substitutes
invented recognized text. Non-macOS platforms report the missing backend after
normal path checks. Physical Intel/older-macOS and live Chrome checks remain
separate gates. The prototype succeeded on macOS 27.0.1, Apple Silicon, with
CPU-only processing; that alone does not establish old Intel compatibility.

The popup/side panel has **Read text from a PNG**. Live browser acceptance of
that control is pending; normal installation remains 0.5.7. UAB-16 stays open
for that acceptance, Word rendering and broader document workflows.
