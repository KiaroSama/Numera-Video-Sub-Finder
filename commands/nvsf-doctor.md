---
description: Check local OCR runtime, models, language packs and GPU or CPU availability.
argument-hint: "[language] [video]"
---

Use the `numera-video-sub-finder` skill for `$ARGUMENTS`. Confirm which data directory the CLI and desktop should use, then run `nvsf doctor --format json`, `nvsf dependencies`, `nvsf models list`, `nvsf models verify`, and `nvsf languages list`. For a given input, also run `nvsf probe <VIDEO>`.

Report the detected providers, usable device policy and missing or invalid resources for the requested language. Use the doctor output to distinguish a usable CPU fallback under `--device auto` from an unavailable explicitly requested GPU. Do not install models or packs, repair the runtime, or change settings until the user approves the specific action.
