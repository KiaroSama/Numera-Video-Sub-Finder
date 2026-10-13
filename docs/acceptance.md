# Acceptance traceability

The initial desktop implementation has 30 requirement groups. This matrix distinguishes source responsibility from executed proof. None is automatically accepted because a file exists; actual evidence lives in exact-source CI reports and validation status.

| IDs | Scope | Implementation / required proof |
|---|---|---|
| FR-001,024 | Six native GUI/CLI targets and packages | ci/targets.json, native workflow, xtask package/standalone; installers/ABI/min-OS remain required |
| FR-002,003,005 | Visible OCR/source timing/coverage modes | app media/extraction/inference; real decode + annotated frame timelines |
| FR-004,006,007 | Subtitle regions/appearances/crops | app regions/vision/extraction, core tracking; gaps/numeric/scene/multi-track real fixtures |
| FR-008,009 | Priority languages/fallback/specialist/Unicode | pinned ONNX/Tesseract/JA metadata; actual en/fa/ja and vertical/script fixtures |
| FR-010,011 | Formats/atomic safe export | core export/app export/storage; literal/independent parser and failure fixtures |
| FR-012 | Explicit model operations | models/model_download/native dialogs; real checksum/resume/cancel/offline admission |
| FR-013,014 | Provider truth/native isolation | providers/worker; actual placement/device/OOM/tree cleanup evidence |
| FR-015 | Queue/history/recovery/edits | desktop queue/resume/jobs/coreedit; actual restart/cancel/resume + durable corrections |
| FR-016,017,018 | Workspace/themes/accessibility | Rust UI/native host; real GUI, styles, keyboard, IME/DPI and source-pixel evidence |
| FR-019,020,023 | Data/resource/privacy/logging | Paths/storage/Log/scoped IPC; offline enforcement, relocation, redaction/retention |
| FR-021 | Optional visual retry | assistant/visual_retry; admitted GGUF pair, cancel/unreadable/proposal-only native test |
| FR-022 | CLI parity | actual CLI commands/shared service; exit/stdout/invalid/batch/resume/export |
| FR-025,026,027 | CI/test/quality floors | ci test inventory/qualification, exact-source reports; missing/partial is not PASS |
| FR-028,029 | Legal/branding/publication | LICENSE/notices/logo/README/native icons; exact package inventory and identity |
| FR-030 | Fail-closed release | ci/qualification.json and release gate; physical/signing/quality missing blocks promotion |

CT-01–CT-30 are catalogued in ci/test-inventory.json. CT-23 is owner-cancelled mobile scope, not a passing mobile test. Real-language quality thresholds, source coverage and mutation floors are retained unchanged; any incomplete lane is explicitly reported and blocks full release qualification.
