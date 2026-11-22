<div align="center">

<img src="assets/logo.svg" alt="Official Numera Video Sub Finder logo" width="240">

# Numera Video Sub Finder

[![Final verification](https://github.com/KiaroSama/Numera-Video-Sub-Finder/actions/workflows/verify.yml/badge.svg)](https://github.com/KiaroSama/Numera-Video-Sub-Finder/actions/workflows/verify.yml)
[![License GPL-3.0-or-later](https://img.shields.io/badge/license-GPL--3.0--or--later-2563eb)](LICENSE)
[![Version 0.2.0](https://img.shields.io/badge/version-0.2.0-059669)](Cargo.toml)
[![Rust 1.100.0 toolchain](https://img.shields.io/badge/rust-1.100.0-8b5cf6)](rust-toolchain.toml)
[![Runs on Windows and Linux, x64 and ARM64](https://img.shields.io/badge/platform-Windows%20%7C%20Linux%20%28x64%2C%20ARM64%29-0891b2)](ci/targets.json)
[![Project guides](https://img.shields.io/badge/docs-project%20guides-1d4ed8)](docs/architecture.md)
[![Tracked repository working tree size 7.9 MB](https://img.shields.io/badge/repo%20size-7.9%20MB-ca8a04)](docs/installation.md)
[![Rust edition 2024](https://img.shields.io/badge/edition-2024-ea580c)](Cargo.toml)
[![Tauri 2 desktop shell](https://img.shields.io/badge/Tauri-2-0d9488)](crates/desktop/Cargo.toml)
[![Leptos Rust and WebAssembly UI](https://img.shields.io/badge/UI-Leptos%20%7C%20WASM-dc2626)](crates/ui/Cargo.toml)
[![Native FFmpeg media decoding](https://img.shields.io/badge/media-FFmpeg-16a34a)](vcpkg.json)
[![ONNX Runtime OCR](https://img.shields.io/badge/OCR-ONNX%20Runtime-4b5563)](ci/native-lock.json)
[![Tesseract verifier](https://img.shields.io/badge/verifier-Tesseract-7c2d12)](ci/tessdata.json)
[![Support donations](https://img.shields.io/badge/Support-donations-d04a9a)](#donate)

</div>

An offline-first desktop workspace for extracting visible, burned-in subtitles from video, reviewing their source frames, and exporting timed Unicode subtitles. Application code, including the Leptos/WebAssembly frontend and CLI, is written in Rust. Tauri and native FFmpeg, ONNX Runtime and Tesseract provide platform integrations.

<p align="center">
  <img src="assets/screenshot-desktop.png" alt="Numera Video Sub Finder v0.2.0 desktop workspace running on Windows 11" width="900">
</p>

## Status

Development; not a qualified release. Source and CI definitions do not prove successful builds or OCR quality. The live CI badge is authoritative for hosted checks. See [validation status](docs/validation.md) for remaining gates. Android and iPhone applications are not included.

The four native 64-bit targets are Windows x64, Windows ARM64, Linux x64 and Linux ARM64. All 32-bit builds are excluded. Hosted CPU CI is separate from real-device accelerator qualification. Windows binaries and NSIS installers carry the pinned owner self-signed development Authenticode signature (`D-014`); public CA publisher trust remains separate. The dedicated updater key is not a platform certificate. Source availability does not imply public trust, GPU placement or real-corpus accuracy certification.

## Installation and setup

Start with the [installation guide](docs/installation.md): pick the matching OS and architecture, verify the package, install required resources and run a first extraction. No ready-build release is claimed while required native checks and signing gates remain open. Development prereleases, when published, are marked experimental and never appear on the stable updater channel.

## Workflow

1. Open a local video and choose a distinct `.srt` output through native file dialogs.
2. Choose from the 123 official Tesseract language and vertical variants, then set subtitle regions, exclusions, source-time range, quality and device policy. Compatible admitted Paddle recognizers are used where available; other languages require their selected Tesseract pack. English, Persian and Japanese keep their original qualification targets; catalog availability does not certify accuracy. Automatic region detection gives editable suggestions; scene signs are not accepted as dialogue on their own.
3. Start extraction. Pause and cancel controls keep compatible checkpoints; missing models return an actionable error instead of downloading during inference.
4. Review original frames, edit text and times, split or merge cues, undo or redo changes, and explicitly accept re-recognition proposals.
5. Save the project or export SRT, WebVTT, basic ASS, or JSON evidence. Replacing an existing file requires confirmation; raw OCR and user edits stay separate.

Monochrome Dark and Light themes run locally: first launch follows the operating system color scheme until you switch themes with the single-click toggle, which plays a native Rust/WASM WebGL1 brushed-silver sweep (`glimm`) and honors `prefers-reduced-motion`. There are no remote fonts, cloud transcription or authored JavaScript application components. Generated Tauri/WASM glue is required by the frameworks.

## CLI

The package includes `nvsf` (`nvsf.exe` on Windows), also used as the isolated GUI worker. Keep it beside the GUI and `runtimes` directory.

```bash
nvsf --help
```

Provision models explicitly while online, or import a known-hash model file from an offline source:

```bash
nvsf models install
```

```bash
nvsf extract --input "video with subtitles.mp4" --output "subtitles.srt" --language fa --quality accurate --device cpu
```

```bash
nvsf probe "video with subtitles.mp4"
```

```bash
nvsf doctor
```

List additional language codes, then install only the language you need:

```bash
nvsf languages list
```

```bash
nvsf languages install fra
```

```bash
nvsf dependencies
```

`dependencies --repair --consent` requests a matching signed application-local native pack when a component is missing or incompatible; it never installs system FFmpeg or changes PATH. A matching published pack is required. Restart after repair. `doctor` reports actual native API and version checks, not proven operator placement. `--gpu-required` and `--accelerator-required` fail when the requested execution policy cannot be established; only ONNX Runtime's forced CPU shape operators may run on CPU. `auto` may use CPU only with a reported fallback; an ordinary CPU package does not contain CUDA or DirectML runtime qualification. Device options are `cpu`, `auto`, `cuda` and `directml`.

`batch`, `bulk`, `resume`, `export`, `benchmark`, `project`, `regions`, `preview`, `queue`, `updates`, and `preferences` share the native application service and emit structured JSON for non-interactive automation and AI agents. Run concurrent multi-video extraction with `bulk` (or `batch --concurrency`):

```bash
nvsf preview strip --input "video.mp4" --output-dir "./frames" --count 6 --grid --overwrite
nvsf preview frame --input "video.mp4" --at-ms 12000 --output "check.png" --grid --draw-box 0.05,0.75,0.9,0.2 --overwrite
nvsf bulk --input-dir "./videos" --output-dir "./subtitles" --language en --concurrency 2
```

An installable AI Agent Skill covering every CLI subcommand, exit code, and JSON workflow lives in [skills/numera-video-sub-finder/SKILL.md](skills/numera-video-sub-finder/SKILL.md) with a full flag reference in [skills/numera-video-sub-finder/references/cli-reference.md](skills/numera-video-sub-finder/references/cli-reference.md). Run any subcommand with `--help` for exact options. Benchmark elapsed time measures that run only, not universal speed or quality.

## Data and privacy

Writable models, projects, logs and cache use the OS's local application-data directory under `Numera Video Sub Finder`. `--data-root` or `NVSF_DATA_ROOT` selects an explicit location. Placing `portable.flag` next to the executable switches storage to its sibling `data` directory. `NVSF_RUNTIME_ROOT` overrides native runtime discovery; ordinary packages load their bundled runtimes.

Video and subtitle text stay local during inference. The desktop checks signed official stable-release metadata at startup and at most daily while open; disable automatic checks in Settings or check manually. No update payload downloads or installs without separate approval. Explicit model or language provisioning and approved dependency repair also use the network. Model bytes are checked against pinned hashes; managed runtime packs and update metadata require signatures. No stable release or feed is claimed before publication.

Installed Windows packages and Linux AppImage are the intended automatic-update formats; portable archives, deb and rpm use verified download and manual format-specific installation. Save review edits and finish jobs or resource operations before installation. Checkpoints bind source, configuration and selected resources; installing unrelated language packs does not alter their identity. Private media, crop evidence, projects and logs are never uploaded by the application.

## Logging

Each CLI, worker or desktop run creates a new UTF-8 file in the selected data root's `logs` directory, named `<script-name>_YYYY-MM-DD_HH-mm-ss_UTC.log` (with a collision suffix when needed). Entries use `[YYYY-MM-DD HH:mm:ss UTC] [LEVEL] [COMPONENT] Message`. Levels are DEBUG, INFO, WARNING, ERROR and CRITICAL. Diagnostic startup, resource, job and exit events are recorded without OCR text, credentials or request bodies; logging initialization failure falls back to stderr. Handles are flushed and closed.

Logs and original crop evidence stay local. At startup, matching application logs older than 30 days or beyond 100 historical files are pruned; logs modified within 24 hours stay on disk to protect concurrent runs. Retention failures are logged as warnings. Crop evidence is not removed by log retention. Review and redact paths or private context before attaching a log to a support issue. No verbose mode is promised where the CLI does not expose one.

## Builds and tests

Builds run only in GitHub Actions. Manually dispatch Final verification (`verify.yml`) after all changes are pushed; it calls native, portable-quality, dependency and source-analysis lanes once for that commit and publishes commit statuses for all 14 verification jobs on that SHA. Pushes and pull requests do not start CI automatically. The committed lockfile, native source baseline, model manifests and four-target inventory drive the pipeline. Local Cargo, Trunk or Tauri compilation is not part of the project's authorized workflow.

See [testing](docs/testing.md), [architecture](docs/architecture.md), [OCR contracts](docs/ocr-models.md), [performance policy](docs/performance.md), [platform support](docs/platform-support.md), [CI matrix](docs/ci-matrix.md), [troubleshooting](docs/troubleshooting.md), [references](docs/references.md), and [third-party notices](THIRD_PARTY_NOTICES.md). Generated synthetic fixtures cover integration and timing mechanics only, not held-out real-world CER or subtitle-event recall.

## License

Application code: GPL-3.0-or-later. Models, native libraries and licensed media retain their own terms. The official logo was supplied by the project owner. No source or branding from unrelated subtitle-finder applications is copied.

## Donate

If this project helps you, donations are appreciated.

| Currency | Network | Address |
| --- | --- | --- |
| Bitcoin (BTC) | Bitcoin | `bc1qmth5m03pu5hujw5xw5jmywam3jj3sqwqupesdt` |
| USDT, BNB, USDC, etc. | BEP20 | `0x0Bd0BA443a8B9cf15922bf7f0Bb0a4b495fD06Ef` |
| USDT, TRX, USDC, etc. | TRC20 | `TWBA3xFTqgZAeAYMxqo85xWnzvty3DcAhw` |
| Ethereum (ETH) | ERC20 | `0x0Bd0BA443a8B9cf15922bf7f0Bb0a4b495fD06Ef` |
| TON | TON | `UQCN8Umo_OfOWqImZetQsrNStPcmLkMAKajFyiCOhso23NDb` |
| Litecoin (LTC) | LTC | `ltc1qntqnnrunadurnw4cshv3qgspywrueyyeyngwuy` |
| Solana (SOL) | Solana | `7B2wkczUjmkDhETwQuknBL8sUsbuV7nErxc317TmQuwR` |
| Polygon (POL) | Polygon | `0x0Bd0BA443a8B9cf15922bf7f0Bb0a4b495fD06Ef` |
