---
description: Inspect, edit or export a saved subtitle project.
argument-hint: "<project.nvsf> [inspect | edit | export]"
---

Use the `numera-video-sub-finder` skill for `$ARGUMENTS`. A project may be `.nvsf`, `.nvsf.json`, or `.json`. Run `nvsf project inspect <PROJECT>` first. A desktop project saved before extraction can have zero cues; report that instead of editing or exporting it as a completed extraction.

List cues with `nvsf project cues <PROJECT> --filter uncertain` and use the returned IDs for edits. Apply only the requested changes, for example `nvsf project edit-cue <PROJECT> --id <ID> --text <TEXT> --output <EDITED_PROJECT>`. Keep the original unless the user requests an in-place edit. Use `nvsf export <EDITED_PROJECT> <OUTPUT> --format <FORMAT>` for SRT, VTT, ASS or JSON; pass `--overwrite` only with approval. Report the saved path and remaining cues that need review.
