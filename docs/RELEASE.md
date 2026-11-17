0.5.24 adds **Task call budgets** to the popup and side-panel controls. Choose limits for the current workspace before a new task: up to 256 local calls per task and 32 per assistant response. Settings persist across host restarts. Existing tasks keep their original ceilings; raising settings cannot refill them, and lowering ceilings restricts subsequent calls. File, command, network and desktop permissions remain separate.

Long-task handoffs now carry the actual latest 24 execution receipts. A 150-call test preserved the remaining 106-call allowance. Setup delivery also recognizes inline-code formatting stripped by ChatGPT and saves unconfirmed submissions so automatic setup retries stop instead of repeatedly sending instructions.

Live Chrome verification saved a two-call ceiling, created and read back a real 23-byte file, returned the third-call denial to ChatGPT, and observed the model stop. A new task received updated limits while the exhausted task remained at zero. Reload, deliberate instruction reinjection and resetting workspace defaults passed. The earlier verification conversation reached its context limit during the faulty setup attempts; the final acceptance used a fresh chat.

Validation: **245 tests passed, 0 failed, 1 ignored**, documentation tests passed, and **41 installed self-tests passed**. Doctor verified the installed host and 923 matching audit entries. Source, package, Chrome card, panel and host hello agree on **0.5.24 / protocol 8**.

This supports longer coding, game-project, research, document and supported media tasks with the same guarded local tools. Fully unattended work still has gaps: other-provider handoffs, file/image exchange, broader application controls, scheduling and stable macOS permission persistence. No game-engine run, finished movie or full provider matrix is claimed by this release.

[Verification](https://github.com/mrfentmen/unified-ai-bridge/blob/main/docs/VERIFICATION.md) · [Budget controls](https://github.com/mrfentmen/unified-ai-bridge/blob/main/docs/OVERNIGHT.md#workspace-call-budgets-0524) · [Remaining tasks](https://github.com/mrfentmen/unified-ai-bridge/blob/main/docs/TASKS.md).
