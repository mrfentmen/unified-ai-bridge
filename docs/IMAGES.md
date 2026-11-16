# Inspect and transform image files

The bridge can inspect PNG, JPEG and WebP files and create a resized, cropped, rotated or converted copy. These are local file operations. They do not generate an image, describe what is pictured, or attach it to an AI conversation.

In **Files, preview and restore → Image files**, enter a source path and choose **Inspect image**. The result reports dimensions, format, transparency, animation, orientation and SHA-256. Enter a new destination with the matching extension, choose the output format, then enter one operation:

```json
{"kind":"resize","width":640,"height":480}
```

```json
{"kind":"crop","x":20,"y":10,"width":200,"height":150}
```

```json
{"kind":"rotate","degrees":90}
```

```json
{"kind":"convert"}
```

**Request image transform** opens an approval with the source hash, exact operation, output dimensions, destination and resulting hash. The source is checked again after approval. Writes preserve the original and refuse any existing destination. A scoped text-write grant does not replace this approval.

Chats call `image_info {path}` and `transform_image {path, destination, expected_sha256, operation, format?}`. The format defaults to `png`; other values are `jpeg` and `webp`. The destination extension must agree. Inspect first to get the source hash.

## What the operations preserve

- Resize uses exact dimensions and a triangle filter. It can change the aspect ratio; choose dimensions deliberately.
- EXIF orientation is applied before cropping, resizing or rotation. Crop coordinates start at the oriented image's top-left corner. Rotation accepts 90, 180 or 270 degrees clockwise.
- PNG and lossless WebP outputs preserve supported alpha channels. JPEG is lossy and requires opaque pixels; transparent input is refused instead of silently adding a background.
- Transformations support 8-bit still images. Animated PNG/WebP, embedded ICC color profiles and high-bit-depth inputs are reported and refused, rather than silently losing those features.
- Output EXIF, XMP and text metadata are not copied. The approval reports this; the original keeps its metadata.
- Metadata inspection does not validate every pixel. Transformations decode the image and fail on corrupt or unsupported pixel data.

## Bounds and failure handling

Inputs are limited to 16 MiB, 8192 pixels on either side, and 16,777,216 pixels total. The decoder allocation budget is 128 MiB; transformations also need bounded image buffers. Outputs are capped at 32 MiB. At most two image operations can hold processing slots; additional calls return a busy result. Processing has a 15-second budget checked between processing phases and during output encoding; this is not a preemptive operating-system CPU limit.

Path, private-file, workspace-lock and research-agent rules still apply. The same descriptor-anchored staging used for archives protects publication and cleanup. An interrupted or uncertain publication must be inspected before retrying. These tools create new files and do not yet provide large-binary replacement/undo.
