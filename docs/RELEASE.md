0.5.21 adds bounded local English OCR for plain PNGs and has now been installed and exercised through real Chrome conversations. Source, package, Chrome card, panels and native handshake agree on 0.5.21 / protocol 6.

ChatGPT, Claude, DeepSeek, Gemini, Qwen and Kimi passed read/create/read-back, traversal denial and approved patch delivery. Thirteen provider files have matching bytes and exactly one committed recovery record each. Grok.com was quota-blocked; Grok on X created its file before reaching its daily limit. The live extension controls also passed OCR, PDF creation/read/render, WAV trim/denial and XLSX copy/read-back checks. A fresh ChatGPT conversation received the expected denial on its fifth call in one response.

Validation: **220 routine tests passed**, the separate **251.50-second command test passed**, and **41 packaged self-tests passed**. Linux tests, Intel macOS tests and packaging succeeded for 0.5.19–0.5.21. Installed doctor checks passed configuration, manifest, binary, SQLite and audit integrity.

**Automatic host-restart recovery failed.** Native Messaging reconnects, but tabs keep stale session IDs while the host has no sessions. Reloading provider tabs restores registration; previous writes were not repeated. Gemini setup drafts, Qwen foreground/candidate handling and website quota/context limits also prevent an unattended-support claim. The new unsigned host still triggered a Desktop-access prompt, so testing used an audited scratch workspace outside Desktop. Stable signing and a physical older Intel Mac remain unverified.

OCR uses only Vision fast CPU revision 1 English in a restricted worker. Recognition errors are retained in the results; accurate mode failed during the prototype and no unrestricted fallback was added. This does not attach images to chats or provide general image understanding.

[Detailed results and open checks](https://github.com/mrfentmen/unified-ai-bridge/blob/main/docs/VERIFICATION.md) · [OCR limits](https://github.com/mrfentmen/unified-ai-bridge/blob/main/docs/OCR.md) · [Next tasks](https://github.com/mrfentmen/unified-ai-bridge/blob/main/docs/TASKS.md).
