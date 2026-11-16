This release adds image inspection and approved resize, crop, rotation and conversion for PNG, JPEG and WebP. Every output goes to a new path. The approval shows the operation, dimensions, source hash and output hash; the source is checked again before writing.

Local verification passed **127 workspace tests** and **41 installed self-tests**. In Chrome, the extension resized a real 128×128 PNG to 64×64, reopened the result and preserved the original bytes. Denying another transform left no output file. The Chrome card, shadow panel and host agree on **0.5.6**, protocol **6**.

The supported subset is 8-bit still images: animation, embedded ICC profiles and high bit depth are refused. PNG/WebP preserve transparency; JPEG requires opaque pixels and is lossy. Orientation is normalized, other metadata is stripped, and originals remain unchanged. Input, decoded size, output and concurrency limits apply. See [image controls](https://github.com/mrfentmen/unified-ai-bridge/blob/main/docs/IMAGES.md).

Reload the extension and provider tabs after installation. These checks exercise panel controls, not image generation or delivery to a model. Persistent macOS consent across unsigned rebuilds, the full current-build provider matrix and a physical older Intel Mac remain unverified. [Verification record](https://github.com/mrfentmen/unified-ai-bridge/blob/main/docs/VERIFICATION.md).
