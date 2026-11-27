---
description: Inspect, populate or resume the durable subtitle extraction queue.
argument-hint: "[list | add | run | resume] [video or queue ID]"
---

Use the `numera-video-sub-finder` skill for `$ARGUMENTS`. Check that the CLI uses the intended data root, then run `nvsf queue list` and consult the saved ID, state, source and checkpoint before making changes.

- Add an agreed job with `nvsf queue add --input <VIDEO> --output <SRT> --language <CODE> --device auto` and record its ID.
- Run or resume saved work with `nvsf queue run --id <ID>`. Omit `--id` only when the user wants to run all eligible entries. The queue verifies the source hash before running.
- Recheck `nvsf queue list` and report each selected job's state, output and checkpoint. Keep interrupted entries for resume. If another CLI process or the desktop owns the queue lock, stop and report the contention.

Do not remove entries, clear the queue, or overwrite an output without explicit approval.
