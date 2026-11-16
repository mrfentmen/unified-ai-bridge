# Initial audio tools

The 0.5.20 candidate adds `media_info` and `transform_media` for a small,
explicit PCM WAV subset. These are local file operations. They do not listen to
audio, transcribe speech, play a sound, send an attachment or generate media.

```json
{"path":"files/recording.wav"}
```

`media_info` reads a bounded file after normal path/privacy checks and reports
format, integer PCM encoding, channels, sample rate, bit depth, total frames,
duration, audio bytes and file SHA256. It validates the container length,
chunk boundaries, byte rate, block alignment and complete interleaved frames.
Metadata tags are not interpreted or returned; unknown chunk IDs are reported
as a bounded hexadecimal list.

The initial subset is little-endian RIFF/WAVE, mono or stereo, 8,000–192,000 Hz,
8/16/24/32-bit integer PCM, at most 32 MiB, 128 chunks and 30 minutes. The format
chunk must be ordinary PCM (16 bytes, or 18 with zero extra bytes). RF64,
big-endian RIFX, extensible/float/compressed WAV, MP3, AAC, FLAC, video and remote
media references are unsupported. A file's extension alone is never trusted.

## Trim an approved copy

```json
{
  "path":"files/recording.wav",
  "destination":"output/clip.wav",
  "expected_sha256":"the hash returned by media_info",
  "operation":{"kind":"trim","start_frame":8000,"end_frame":24000}
}
```

At an 8,000 Hz sample rate, this example selects seconds 1–3. Frame indices are
zero-based, start inclusive and end exclusive. Each frame includes every audio
channel. A trim copies the exact selected interleaved bytes, retains sample
rate/channels/bit depth, and builds a new PCM WAV container. It performs no
resampling, re-encoding, normalization or fade. An abrupt trim may click at its
boundary; no audio quality claim is made.

Every trim requires explicit approval, including in Trusted mode. The review
shows the source hash, new destination, exact frame range, duration and output
hash. The source hash and destination policy are checked again after approval.
Only a new `.wav` in an existing permitted directory can be published; originals
are preserved, existing files are never overwritten, and locked/private/research
paths remain protected. Writes reuse descriptor-anchored staging, path locks,
create-only publication and hash-chained audit events.

`trim_supported` is false when the input has any non-padding chunk besides its
format and audio data. Markers, lists, cue points or other metadata are refused
for transformation rather than silently lost. `JUNK`/`PAD ` chunks are container
padding and can be dropped. Odd-sized output data retains the required pad byte.
Two concurrent/pending operations are allowed; there is no codec subprocess,
shell command, external reference resolution or playback side effect.

The popup/side panel has **Inspect and trim WAV audio**. Inspect first, enter a
new output path and frame range, then request the copy. The same tools are in
the profile-filtered catalog. The current browser connection is unavailable,
so the new controls remain unverified in live Chrome; normal installation stays
0.5.7. Native tests and public-file checks are recorded in [VERIFICATION.md](VERIFICATION.md).

## Remaining work

Broader containers/codecs, video metadata/thumbnails, audio extraction,
conversion, actual playback checks, transcription and provider audio handoff
remain separate tasks. UAB-20 stays open. Independent file decoding and correct
duration do not prove audio delivery to a model or every viewer's compatibility.
