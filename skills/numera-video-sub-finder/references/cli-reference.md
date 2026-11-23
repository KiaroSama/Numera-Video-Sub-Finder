# nvsf CLI Reference

Exact command, flag, JSON output, and exit-code specification for `nvsf` (`crates/cli/`).

## Global flags

- `--data-root <PATH>`: Override the application data directory (models, projects, logs, cache, `desktop-queue.json`, `updates.json`). Can appear before or after the subcommand.

## Exit codes

| Code | Source | Meaning |
|---|---|---|
| `0` | Success | Command completed with zero failures and no cues requiring review |
| `2` | `status::project` | Extraction or batch/bulk/queue completed with zero failures, but at least one cue is unreviewed or has `confidence < 0.7` |
| `3` | `clap` or `AppError::InvalidConfig` | Invalid arguments, unknown language/model/format, concurrency outside `1..=8`, missing `--opt-in`/`--consent`, or invalid `Rect` |
| `4` | `AppError::Io` / `Core` / `Json` / `Message` | Missing input, hash mismatch, corrupt project/queue, unreadable crop, network or worker failure |
| `5` | `AppError::Cancelled` | Operation cancelled via `SIGINT` or `--fail-fast` abort |
| `6` | `AppError::Provider` | Requested hardware provider unavailable under `--gpu-required` or `--accelerator-required` |

---

## Preflight and provisioning

### `nvsf doctor [--format json]`

Checks CLI version, build commit SHA, OS/architecture, paths, native runtime diagnostics, and model catalog.

Stdout JSON:
```json
{
  "version": "0.5.0",
  "source_sha": "af4917c...",
  "os": "windows",
  "architecture": "x86_64",
  "data_root": "...",
  "runtimes": "...",
  "dependencies": [ { "name": "...", "state": "verified|missing|...", "..." : "..." } ],
  "models": [ { "id": "...", "installed": true, "..." : "..." } ],
  "acceleration_qualification": "not established by enumeration"
}
```

### `nvsf probe <INPUT>`

Probes `<INPUT>` media metadata via the native FFmpeg bridge.

Stdout JSON (`nvsf_core::MediaInfo`):
```json
{
  "width": 1920,
  "height": 1080,
  "duration_ms": 60000,
  "codec": "h264",
  "time_base_num": 1,
  "time_base_den": 1000,
  "subtitle_streams": 0,
  "rotation": 0
}
```

### `nvsf dependencies [--repair --consent]`

- Without `--repair`: prints JSON array of runtime dependency diagnostics.
- With `--repair` alone: fails with exit code `4` and downloads nothing.
- With `--repair --consent`: verifies or downloads signed app-local runtimes without modifying system software.

### `nvsf models <SUBCOMMAND>`

- `models list`: JSON array of model catalog entries.
- `models install [--id <ID>]`: downloads and hash-verifies all baseline models or one official model ID.
- `models verify`: verifies hashes of baseline models, installed optional `ja-specialist`, and `eng`/`fas`/`jpn` tessdata.
- `models import <ID> <PATH>`: imports a verified model file into `<data-root>/models`.
- `models export <ID> <PATH>`: exports an installed model file.

### `nvsf languages <SUBCOMMAND>`

- `languages list`: JSON array of language pack statuses.
- `languages install <CODE>`: downloads and hash-verifies a language pack from the official catalog.
- `languages verify <CODE>`: verifies installed language pack hashes.
- `languages import <CODE> <PATH>`: imports a language pack file.
- `languages export <CODE> <PATH>`: exports an installed language pack file.

---

## Regions and frame preview

### `nvsf regions suggest --input <VIDEO> [--language <CODE>]`

Runs isolated region detection (default `--language en`) and returns normalized bounding boxes (`0.0..=1.0`).

Stdout JSON:
```json
{
  "input": "video.mp4",
  "language": "en",
  "regions": [
    { "x": 0.05, "y": 0.78, "width": 0.90, "height": 0.16 }
  ]
}
```

### `nvsf preview frame --input <VIDEO> --at-ms <MS> --output <PNG> [--crop <X,Y,W,H>] [--grid] [--draw-box <X,Y,W,H>]... [--pixel-box <PX_X,PX_Y,PX_W,PX_H>]... [--overwrite]`

Decodes the frame at `--at-ms` and writes a PNG to `--output`. Visual inspection flags let an AI agent inspect any part of the frame and verify exact subtitle capture boxes before extraction:
- `--grid`: overlays a `0.1..0.9` normalized coordinate grid with edge digit ticks (`1`..`9`) in full-frame coordinates.
- `--draw-box <X,Y,W,H>` (repeatable): draws high-contrast colored boxes at normalized coordinates (`0..1`).
- `--pixel-box <PX_X,PX_Y,PX_W,PX_H>` (repeatable): converts frame-pixel boxes into validated normalized `Rect`s, draws them onto the PNG, and prints each box's `region_arg` (`"x,y,w,h"`) ready for `--roi`.
- `--crop <X,Y,W,H>`: crops to any sub-region of the frame after drawing full-frame grid/box overlays so coordinates stay consistent with `--roi`.

Stdout JSON:
```json
{
  "input": "video.mp4",
  "at_ms": 12000,
  "output": "frame.png",
  "frame_width": 1920,
  "frame_height": 1080,
  "width": 1728,
  "height": 216,
  "crop": { "x": 0.05, "y": 0.75, "width": 0.9, "height": 0.2 },
  "crop_pixels": { "x": 96, "y": 810, "width": 1728, "height": 216 },
  "cropped": true,
  "grid": true,
  "boxes": [
    {
      "index": 0,
      "normalized": { "x": 0.05, "y": 0.75, "width": 0.9, "height": 0.2 },
      "region_arg": "0.0500,0.7500,0.9000,0.2000",
      "pixels": { "x": 96, "y": 810, "width": 1728, "height": 216 }
    }
  ],
  "bytes": 1120000
}
```

### `nvsf preview strip --input <VIDEO> --output-dir <DIR> [--count <1..24>] [--start-ms <MS>] [--end-ms <MS>] [--crop <X,Y,W,H>] [--grid] [--draw-box <X,Y,W,H>]... [--pixel-box <PX_X,PX_Y,PX_W,PX_H>]... [--overwrite]`

Samples `--count` (default `6`, range `1..=24`) evenly spaced frames across `[start_ms, end_ms]` (or the full probed duration), applies the same `--grid`, `--draw-box`, `--pixel-box`, and `--crop` overlays, writes `frame-001-<ms>ms.png`..`frame-NNN-<ms>ms.png` into `--output-dir`, and prints a JSON manifest with `duration_ms`, `frame_width`, `frame_height`, and `frames[]`.

---

## Extraction, resume, export, batch, and bulk

### `nvsf extract` / `nvsf benchmark`

Flags:
- `--input <PATH>` (required)
- `--output <PATH>` (required)
- `--language <CODE>` (alias `--languages`, default `en`)
- `--device <auto|cpu|cuda|directml>` (default `auto`)
- `--quality <accurate|balanced|fast>` (default `balanced`)
- `--roi <X,Y,W,H>` (repeatable, normalized `Rect`, max 8)
- `--exclude <X,Y,W,H>` (repeatable, normalized `Rect`, max 32)
- `--start-ms <MS>` (default `0`)
- `--end-ms <MS>` (optional)
- `--threads <1..=16>` (default `2`)
- `--overwrite`
- `--normalize-persian`
- `--gpu-required`
- `--accelerator-required`
- `--provider-profile <ABSOLUTE_NEW_DIR>` (records two-stage ONNX placement; requires explicit non-`auto` `--device` and a new absolute directory)

Stdout JSON:
```json
{"cues":42,"completed":true,"review_needed":false,"elapsed_ms":12345,"warnings":[]}
```

### `nvsf resume <PROJECT>`

Loads `<PROJECT>` (`.nvsf.json` checkpoint) and resumes extraction with its saved `ExtractConfig`.

Stdout JSON:
```json
{"completed":true,"cues":42,"review_needed":false}
```

### `nvsf export <PROJECT> <OUTPUT> [--format srt|vtt|ass|json] [--overwrite] [--bom] [--crlf]`

Exports cues from `<PROJECT>` to `<OUTPUT>` atomically. Multi-track projects also write `<stem>.track-<N>.<format>` files without merging tracks.

### `nvsf batch <INPUTS>... --output-dir <DIR> [--language <CODE>] [--concurrency <1..=8>]`

- `--concurrency` defaults to `1` (sequential, backward-compatible).
- Writes `<output-dir>/<stem>-<index>.srt` for each input and emits one JSON object per line to stdout:
  - Sequential (`--concurrency 1`): `{"item":1,"completed":true,"cues":12,"review_needed":false}`
  - Concurrent (`--concurrency 2..=8`): `{"item":1,"completed":true,"cues":12,"review_needed":false,"status":"ok","error":null}`

### `nvsf bulk`

Concurrent multi-video extraction using a bounded worker pool (`std::thread::scope`) with isolated checkpoints and per-video error capture.

Flags:
- `--input <PATH>` (repeatable)
- `--inputs-file <PATH>` (JSON string array `["a.mp4","b.mp4"]` OR newline-delimited paths; blank lines and `#` lines are ignored; max 4 MiB)
- `--input-dir <DIR>` (scans regular files matching `--extensions`, sorted deterministically)
- `--extensions <CSV>` (default `mp4,mkv,mov,webm,avi`, case-insensitive)
- `--output-dir <DIR>` (optional; must exist if provided; omits side-by-side placement next to each input)
- `--concurrency <1..=8>` (default `2`)
- `--fail-fast` (default `false`; stops unstarted/in-flight workers after the first failure)
- `--language <CODE>` (alias `--languages`, default `en`)
- `--device <auto|cpu|cuda|directml>` (default `auto`)
- `--quality <accurate|balanced|fast>` (default `balanced`)
- `--roi <X,Y,W,H>` (alias `--region`, repeatable)
- `--exclude <X,Y,W,H>` (repeatable)
- `--start-ms <MS>` (default `0`)
- `--end-ms <MS>` (optional)
- `--threads <1..=16>` (default `2`; divided across concurrent workers via `max(1, threads / concurrency)`)
- `--format <srt|vtt|ass|json>` (default `srt`)
- `--overwrite`
- `--resume`
- `--bom`
- `--crlf`
- `--normalize-persian`
- `--min-confidence <FLOAT>` (default `0.7`)
- `--gpu-required`
- `--accelerator-required`

Stdout JSON:
```json
{
  "mode": "bulk",
  "concurrency": 2,
  "per_job_threads": 2,
  "resume": false,
  "total": 3,
  "succeeded": 2,
  "review_needed": 1,
  "failed": 1,
  "elapsed_ms": 8420,
  "items": [
    {
      "input": "clip1.mp4",
      "output": "out/clip1.srt",
      "status": "ok",
      "exit_code": 0,
      "cues": 18,
      "review_needed": false,
      "elapsed_ms": 4100,
      "error": null
    }
  ]
}
```
`status` is one of `ok`, `review_needed`, `cancelled`, or `failed`. Exit code is `0` if all items are `ok`, `2` if zero failed and at least one is `review_needed`, or the first non-zero failure exit code (`4`, `5`, or `6`).

---

## Project inspection, search, editing, and review

### `nvsf project inspect <PROJECT>`

Returns project metadata, `media`, `config`, `warnings`, `review_needed`, and `counts` (`total`, `reviewed`, `unreviewed`, `uncertain`, `edited`).

### `nvsf project cues <PROJECT> [--query <TEXT>] [--filter all|unreviewed|uncertain|edited] [--offset <N>] [--limit <N>]`

Filters cues identically to the desktop review view (`uncertain` matches `confidence < 0.7` or `null`; `--query` performs case-insensitive substring match on `cue.text`).

Stdout JSON:
```json
{
  "total": 40,
  "matched": 3,
  "offset": 0,
  "limit": 50,
  "filter": "uncertain",
  "query": "",
  "cues": [ { "id": 2, "track": 0, "start_ms": 4500, "end_ms": 8000, "text": "...", "raw_text": "...", "confidence": 0.42, "reviewed": false, "edited": false, "crop": null } ]
}
```

### `nvsf project edit-cue <PROJECT> --id <U64> [--text <STR>] [--start-ms <MS>] [--end-ms <MS>] [--output <PATH>]`

Requires at least one of `--text`, `--start-ms`, or `--end-ms`. Sets `edited: true` and `reviewed: false`, validates cue and project, and saves atomically. Returns `{"saved": "...", "cue": {...}}`.

### `nvsf project accept-cue <PROJECT> --id <U64> [--output <PATH>]`

Sets `reviewed: true` and saves atomically. Returns `{"saved": "...", "cue": {...}}`.

### `nvsf project revert-cue <PROJECT> --id <U64> [--output <PATH>]`

Restores `cue.text` from `cue.raw_text`, sets `edited: false` and `reviewed: false`, validates, and saves atomically. Returns `{"saved": "...", "cue": {...}}`.

### `nvsf project split-cue <PROJECT> --id <U64> --at-ms <MS> [--output <PATH>]`

Splits cue `--id` at `--at-ms` (`start_ms < at_ms < end_ms`), assigns `max_id + 1` to the second half, marks both `edited: true`, and saves atomically. Returns `{"saved": "...", "split_id": ..., "at_ms": ..., "total_cues": ..., "cues": [...]}`.

### `nvsf project merge-cues <PROJECT> --first <U64> --second <U64> [--output <PATH>]`

Merges two distinct cues in the same track, keeping `--first`'s ID and combining text with `\n`. Returns `{"saved": "...", "merged_id": ..., "removed_id": ..., "total_cues": ..., "cue": {...}}`.

### `nvsf project re-ocr <PROJECT> --id <U64> --input <VIDEO> [--language <CODE>] [--region <X,Y,W,H>] [--at-ms <MS>] [--apply] [--output <PATH>]`

Verifies `sha256(--input) == project.source_hash`, runs isolated OCR on the cue crop, and prints `{"cue_id", "at_ms", "language", "region", "recognition", "applied", "saved"}`. Without `--apply`, the project file is unchanged.

### `nvsf project visual-retry <PROJECT> --id <U64> --input <VIDEO> --opt-in [--region <X,Y,W,H>] [--at-ms <MS>] [--apply] [--output <PATH>]`

Fails closed with exit code `3` unless `--opt-in` is present. Verifies `source_hash`, requests a local visual retry proposal, and only mutates `cue.text` (preserving `raw_text`, timing, crop, and confidence) when `--apply` is passed and `proposal.unreadable` is `false`.

### `nvsf project visual-retry-install --opt-in`

Fails closed with exit code `3` without `--opt-in`. Downloads and hash-verifies the optional visual retry model pack into `<data-root>/models/assistant`.

### `nvsf project cue-evidence <PROJECT> --id <U64> --output <PNG> [--overwrite]`

Reads the cue's content-bound crop PNG from `<data-root>/cache` via `crop_evidence::read` and writes it to `--output`. Returns `{"cue_id", "output", "bytes"}`.

### `nvsf project save <PROJECT> [--output <PATH>]`

Validates and writes `<PROJECT>` atomically to `--output` (or rewrites in-place). Returns `{"saved", "cues", "completed", "review_needed"}`.

### `nvsf project history [--limit <N>]`

Lists `.json` project files in `<data-root>/projects` sorted newest-first (default `--limit 200`, bounded to 2000 directory entries). Returns `{"projects_dir", "total", "entries": [{"name", "path", "modified_unix_secs"}]}`.

---

## Persistent queue

Stored at `<data-root>/desktop-queue.json` (schema 1, max 100 entries, max 2 MiB) guarded by `<data-root>/desktop-queue.lock`. Fully compatible with the desktop app queue.

- `nvsf queue list`: returns `{"count", "entries": [...]}`.
- `nvsf queue add --input <VIDEO> --output <SRT> [extract flags...] [--checkpoint <PATH>]`: hashes input, validates `ExtractConfig` and optional checkpoint, saves atomically, returns `{"added": "<id>", "count", "entries"}`.
- `nvsf queue remove --id <ID>`: refuses to remove `running` entries; returns `{"removed": "<id>", "count", "entries"}`.
- `nvsf queue clear [--state all|queued|interrupted|completed|cancelled|failed]`: removes matching non-running entries; returns `{"cleared", "remaining", "entries"}`.
- `nvsf queue run [--id <ID>] [--concurrency <1..=8>] [--fail-fast]`: verifies each entry's `source_hash` before running, updates `state`/`checkpoint`/`error` in `desktop-queue.json`, and prints `{"mode": "queue", "concurrency", "total", "succeeded", "review_needed", "failed", "elapsed_ms", "items": [...]}`.

---

## Updates and preferences

- `nvsf updates status [--format portable|nsis|msi|appimage|deb|rpm]`: returns `{"current_version", "target", "enabled", "last_attempt", "offer"}`.
- `nvsf updates check [--format portable|nsis|msi|appimage|deb|rpm]`: fetches signed `latest.json` and `latest.json.sig`, verifies minisign signature, caches any available offer in `<data-root>/cache/cli-update-offer.json`, and updates `last_attempt` in `updates.json`.
- `nvsf updates config --enabled <true|false>`: updates `updates.json` preference.
- `nvsf updates download --token <TOKEN> --consent [--output <PATH>] [--overwrite]`: fails closed without `--consent` (exit `3`). Downloads the official GitHub release asset to a staging file, verifies byte size, SHA-256, minisign artifact signature, and target binary header before writing to `--output` (or `<data-root>/cache/updates/<filename>`).
- `nvsf updates verify --token <TOKEN> [--file <PATH>]` or `nvsf updates verify --file <PATH> --signature <SIG_OR_PATH> --version <SEMVER> [--sha256 <HEX>] [--target <TARGET>]`: verifies SHA-256, minisign signature, and optional target header offline.
- `nvsf preferences get`: returns `{"schema", "update_checks_enabled", "last_update_attempt", "data_root"}`.
- `nvsf preferences set [--update-checks <true|false>] [--reset-last-attempt]`: updates `<data-root>/updates.json`.

---

## Error recovery rules

- Exit `2`: Run `nvsf project cues <proj> --filter uncertain`, inspect `cue-evidence` or run `re-ocr` / `edit-cue` / `accept-cue`, then re-run `nvsf export`.
- Exit `3`: Check flag syntax, language catalog (`nvsf languages list`), model IDs (`nvsf models list`), `Rect` bounds (`0..1`, `x + width <= 1`, `y + height <= 1`), or missing `--opt-in` / `--consent`.
- Exit `4` with checkpoint mismatch: Either pass the original `ExtractConfig` flags or choose a new `--output` path.
- Exit `4` with queue lock contention: Close the running desktop app or wait for another `nvsf queue` process to exit.
- Exit `5`: Re-run `nvsf resume <checkpoint>` or `nvsf bulk --resume` to continue from `processed_ms`.
- Exit `6`: Inspect `nvsf doctor` dependencies and either switch `--device auto` / `--device cpu` or repair GPU runtimes.
