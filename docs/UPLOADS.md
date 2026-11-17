# Send a local file to a chat

Version 0.5.30 adds `attach_file_to_chat` and **Local file → chat** controls. The current adapter targets ChatGPT only. It supports UTF-8 text and source files plus PNG, JPEG and WebP images, up to 512 KiB per attachment. Other providers, larger files and other formats require the website's manual attachment control.

Reading a local file is separate from uploading it. Each upload requests a new disclosure approval showing the path, name, size, type, hash and destination conversation. YOLO and local write grants cannot approve it. The host re-reads the file after approval and refuses changed hashes, unsafe paths, symlinks, hard links, private files, locked workspaces and changed sessions.

## From a model

Ask the model to inspect the file's hash with `calculate_sha256`, then request `attach_file_to_chat` with `path` and `expected_sha256`. Approve only the file and conversation you intend. The extension selects the approved bytes in ChatGPT's actual file input, waits for a completed attachment card, and lets the matching bridge result carry that attachment into the chat. A saved image path alone does not give the model vision.

For a saved screenshot or PDF page preview, select its PNG file the same way. Review screenshots for unrelated information before approving disclosure. This tool does not grant screen capture permission or capture anything by itself.

## From the panel

Expand **Local file → chat**, select the exact ChatGPT conversation, enter a workspace file path and click **Inspect exact file hash**. Then request and review upload approval. Panel requests leave the real attachment in that conversation's composer; write your prompt and press the website's Send button. The panel never submits an unrelated draft.

## Interrupted transfers

The bridge refuses to start when the composer contains a draft or another attachment. While returning a result, it will submit only the exact completed attachment owned by that result. Unrecognized cards and user-added attachments block automatic sending.

An upload attempt is recorded before browser dispatch and survives host restart. A repeated attempt for the same file and user turn is refused. If the website times out, navigation occurs, or the extension reloads, inspect the composer and conversation. The file may already have reached the website; the bridge cannot retract uploaded bytes and never blindly retries. Removing an attachment manually does not erase the website's copy.

Native authorization tests cover approval/denial, stale hashes, locked workspaces, ended sessions, unsafe paths, unsupported providers and persisted uncertain attempts. These are transport/policy tests. Provider acceptance must additionally prove the deployed extension uploads the real file and the model reads its contents; consult [verification](VERIFICATION.md) for the recorded status.

The first 0.5.27 live image attempt reached ChatGPT but timed out because image cards differ from text-file cards. The existing attachment was sent manually and the model correctly read its text. The corrected adapter handles both observed card forms, waits on DOM changes in background tabs, and permits ordinary text setup even when the upload input has not mounted. ChatGPT then reported its attachment quota exhausted, so automatic image submission on the corrected build remains blocked pending quota reset.
