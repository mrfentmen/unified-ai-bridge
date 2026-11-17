# Continue a task across chats

The bridge is a general local toolbox. The same continuation mechanism applies to code, game source/assets, research notes, documents and supported media operations. It does not require a separate research workflow, and it does not give a website capabilities or quota that its account lacks.

## ChatGPT controls in 0.5.23

The in-page panel offers **Continue in fresh chat** while a user task is active. It freezes that task, saves a bounded handoff, opens a fresh ChatGPT conversation in the same tab, sends the original task plus instructions and real execution receipts, and binds the remaining tool-call budget to the new conversation. The original chat remains in website history. This uses ordinary visible chat UI, not a provider API.

**Auto handoff: ON** enables this path when the adapter detects ChatGPT's visible **Context window full** notice. It starts off. It allows at most three handoffs in a task lineage and never increases the original call budget. The setting follows that lineage but resets on workspace changes. The normal task, workspace, write, command and network policies continue to apply. Session-specific grants do not automatically transfer to a new session.

The bridge uses a fresh chat, not the website's Branch in new chat control: preserving the entire old history has not been proven to resolve a context limit. Account usage limits pause delivery; opening chats is not a quota reset.

Patch previews, Git review IDs, artifact grants and command-job ownership remain bound to their original conversation. Historical IDs in the handoff are not permission to reuse them. The new chat must make a fresh preview when the original task authorizes it; a task that prohibits retries must stop and report the refusal. Handoffs do not transfer pending approvals or silently broaden grants.

A handoff ticket is restricted to its workspace, provider and Chrome tab, expires after 30 minutes, and can bind to only one new task. It survives a native-host restart. The source task stays frozen until binding or cancellation. **Cancel handoff** leaves the original task paused; resume it explicitly from Chat task controls if appropriate. Stop prevents later binding. A failed transfer does not silently resume the old task or create a second budget.

The handoff includes the original task (up to 8 KiB), up to 24 execution summaries across its lineage, and the latest result when it fits within 16 KiB. The full history remains in local activity and task receipts. It tells the model to inspect current files before further edits, preserve constraints, avoid repeated writes and stop if the objective is already satisfied. This is a factual execution handoff, not a guarantee of the model's reasoning or a full transcript summary.

## Pending results and drafts

Queued results retain their workspace, conversation/turn identity, draft ownership and submission state in Chrome extension **local** storage. This is not Chrome sync. Results can contain file contents already approved for that provider; they are removed from the conversation's saved queue after delivery or confirmed invalidation.

A page reload can recover a queued result. A changed user turn or workspace prevents delivery into the wrong context. An existing user draft is preserved. A missing transcript during page hydration is not treated as evidence of a changed turn.

The sender attempts a submit control once. If the website does not acknowledge submission, the result remains saved and automatic resubmission stops. Inspect the chat before using **Send pending result**. This prevents repeated clicks from submitting duplicate messages during slow responses. A confirmed context-full rejection can use the handoff path.

Transfers refuse unknown/running tool outcomes, running/interrupted command jobs, unrelated drafts, changed workspaces, exhausted budgets and stopped tasks. They never repeat a local tool call to make the new chat appear current.

## Boundaries

Fresh-chat automation is currently implemented for ChatGPT. Other providers keep their existing result delivery and session recovery. Their website-specific handoff controls require separate implementation and live verification. Quota/login gates, Qwen candidate selection, Gemini setup drafts, macOS consent and manual approvals can still require user attention. YOLO changes only its documented local-write approvals.

This does not yet implement scheduling, arbitrary desktop input, automatic asset downloads/uploads, game-engine control or video editing. Ordinary source/asset file work uses the existing tools. Running a game engine, renderer or media converter requires a reviewed command or application integration. See [the task backlog](TASKS.md) and [verification evidence](VERIFICATION.md).
