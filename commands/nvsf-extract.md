---
description: Extract burned-in subtitles from one video or a list of videos.
argument-hint: "<video or list of videos> [language] [output]"
---

Use the `numera-video-sub-finder` skill for `$ARGUMENTS`. Confirm the source paths, subtitle language and output paths. Run `nvsf doctor --format json` and probe the source if its video stream is uncertain. For a shared desktop queue or project directory, check `NVSF_DATA_ROOT` first.

For one video, use `nvsf extract --input <VIDEO> --output <SRT> --language <CODE> --device auto`. If the subtitle region is unknown, use `nvsf regions suggest` and inspect a preview frame before applying `--roi`. For several named videos with the same settings, use `nvsf batch <INPUTS>... --output-dir <DIR> --language <CODE>`. Use the durable queue if the batch must survive a restart.

Keep source media intact and do not pass `--overwrite` without approval. Report the generated files, checkpoints and any cues awaiting review. Exit `2` means completed output needs review; exit `5` means cancellation. For an interrupted single extraction, use its checkpoint with `nvsf resume <CHECKPOINT>`.
