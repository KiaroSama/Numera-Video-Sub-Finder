---
name: numera-video-sub-finder
description: Use when extracting burned-in video subtitles locally, detecting subtitle regions, inspecting or editing .nvsf.json subtitle projects, running concurrent multi-video bulk extraction, managing the persistent extraction queue, exporting SRT/VTT/ASS/JSON subtitles, or verifying runtimes, models, language packs, and updates through the nvsf CLI.
---

# Numera Video Sub Finder CLI

Use `nvsf` for local, non-interactive video subtitle extraction, cue review, and batch or bulk processing. Every subcommand writes structured JSON to stdout (or plain confirmation text for legacy maintenance commands) and sends progress lines to stderr.

Read `references/cli-reference.md` for exact flags, JSON schemas, and recovery rules before constructing unfamiliar commands.

## Global option and exit codes

Pass `--data-root <DIR>` before any subcommand to isolate models, projects, queue state, logs, and cache in a specific directory.

| Exit code | Meaning | Agent action |
|---|---|---|
| `0` | Success; no cues flagged for review | Continue pipeline |
| `2` | Completed with low-confidence or unreviewed cues (`review_needed: true`) | Inspect uncertain cues via `project cues --filter uncertain`, edit or re-OCR, then export |
| `3` | Invalid arguments, flags, or missing opt-in/consent | Fix flags or obtain explicit approval; do not retry unchanged |
| `4` | Runtime, I/O, hash mismatch, or worker failure | Read `ERROR:` on stderr and run `nvsf doctor` or `nvsf probe` |
| `5` | Cancelled by `SIGINT` | Resume from `<output>.nvsf.json` with `nvsf resume` or `nvsf bulk --resume` |
| `6` | Required execution provider unavailable (`--gpu-required` or `--accelerator-required`) | Verify GPU runtime in `nvsf doctor` or switch `--device` |

## Standard agent workflow

### 1. Preflight and provisioning

1. Check runtimes, models, and data root:
   ```sh
   nvsf doctor --format json
   ```
2. Verify native media and OCR libraries without modifying system software:
   ```sh
   nvsf dependencies
   ```
   Repair app-local runtimes only with explicit approval:
   ```sh
   nvsf dependencies --repair --consent
   ```
3. Inspect or install OCR models and language packs:
   ```sh
   nvsf models list
   nvsf models install
   nvsf languages list
   nvsf languages install en
   ```
4. Probe a source video for dimensions, codec, duration, and rotation:
   ```sh
   nvsf probe path/to/video.mp4
   ```

### 2. Region detection, preview, and single-video extraction

1. Detect candidate subtitle bounding boxes (`x,y,width,height` normalized to `0..1`):
   ```sh
   nvsf regions suggest --input video.mp4 --language en
   ```
2. Render a verification frame (optionally cropped to a region) as PNG:
   ```sh
   nvsf preview frame --input video.mp4 --at-ms 12000 --output frame.png --crop 0.05,0.75,0.9,0.2 --overwrite
   ```
3. Run extraction (produces `out.srt` and checkpoint `out.nvsf.json`):
   ```sh
   nvsf extract --input video.mp4 --output out.srt --language en --device auto --quality balanced --roi 0.05,0.75,0.9,0.2 --overwrite
   ```
4. Resume an interrupted extraction from its checkpoint:
   ```sh
   nvsf resume out.nvsf.json
   ```

### 3. Concurrent multi-video extraction

Use `bulk` for directory scans, file lists, multi-format export, and isolated per-video error capture:

```sh
nvsf bulk --input-dir ./clips --extensions mp4,mkv,mov --output-dir ./subs --concurrency 2 --threads 4 --language en --format srt --overwrite
```

- Inputs accept repeatable `--input <path>`, `--inputs-file <path>` (JSON string array or newline-delimited text with `#` comments), and `--input-dir <dir>`.
- `--concurrency` accepts `1..=8` (default `2`). Per-job thread count is automatically divided from `--threads` (`max(1, threads / concurrency)`).
- By default (`--fail-fast` omitted), one failing video records its error in `items[]` without aborting sibling jobs.
- For legacy positional batch runs, `nvsf batch clip1.mp4 clip2.mp4 --output-dir ./subs --language en --concurrency 2` also supports `1..=8` workers (`1` by default).

### 4. Non-interactive cue review, editing, and export

Every `project` mutation validates cues against `nvsf-core` invariants and writes atomically (in-place by default, or to `--output <copy>` to keep the original untouched):

1. Inspect project counts and warnings:
   ```sh
   nvsf project inspect out.nvsf.json
   ```
2. List cues needing review (`all`, `unreviewed`, `uncertain`, `edited`):
   ```sh
   nvsf project cues out.nvsf.json --filter uncertain --offset 0 --limit 50
   ```
3. Extract a cue's cached crop evidence PNG for visual inspection:
   ```sh
   nvsf project cue-evidence out.nvsf.json --id 12 --output cue-12.png --overwrite
   ```
4. Re-run OCR on one cue (verifies `source_hash` against the linked video; add `--apply` to save the result):
   ```sh
   nvsf project re-ocr out.nvsf.json --id 12 --input video.mp4 --apply
   ```
5. Run opt-in local visual retry (requires `--opt-in`; install the optional pack once with `nvsf project visual-retry-install --opt-in`):
   ```sh
   nvsf project visual-retry out.nvsf.json --id 12 --input video.mp4 --opt-in --apply
   ```
6. Edit, accept, revert, split, or merge cues:
   ```sh
   nvsf project edit-cue out.nvsf.json --id 12 --text "Corrected line" --start-ms 4600 --end-ms 7900
   nvsf project accept-cue out.nvsf.json --id 12
   nvsf project revert-cue out.nvsf.json --id 12
   nvsf project split-cue out.nvsf.json --id 12 --at-ms 6200
   nvsf project merge-cues out.nvsf.json --first 12 --second 13
   ```
7. Export reviewed subtitles (`srt`, `vtt`, `ass`, `json`):
   ```sh
   nvsf export out.nvsf.json out.vtt --format vtt --overwrite
   ```

### 5. Persistent queue, updates, and preferences

- `queue` shares `<data-root>/desktop-queue.json` (schema 1) with the desktop app under `desktop-queue.lock`:
  ```sh
  nvsf queue add --input video.mp4 --output out.srt --language en
  nvsf queue list
  nvsf queue run --concurrency 2
  nvsf queue clear --state completed
  ```
- `updates` checks signed GitHub release feeds and verifies SHA-256, minisign signatures, and target executable headers before saving (never self-replaces binaries):
  ```sh
  nvsf updates status
  nvsf updates check --format portable
  nvsf updates download --token <token> --consent --output update.zip --overwrite
  nvsf updates verify --token <token>
  ```
- `preferences` reads and writes `<data-root>/updates.json`:
  ```sh
  nvsf preferences get
  nvsf preferences set --update-checks false
  ```

## Safety rules

- Never pass `--consent` (on `dependencies --repair` or `updates download`) or `--opt-in` (on `project visual-retry` / `project visual-retry-install`) unless the user explicitly requested that network download or optional model pack.
- Keep original OCR text intact by using `--output <copy>` when testing edits, or restore raw OCR with `project revert-cue`.
- Never manually edit `desktop-queue.json` or `updates.json`; always use `nvsf queue` and `nvsf preferences` / `nvsf updates` so locking and schema validation remain intact.
