# Architecture

## Boundaries

| Component | Responsibility | Dependency direction |
|---|---|---|
| `crates/core` | Checked source time, normalized regions, cue/project contracts, appearance tracking, edits and text export | serde/errors only; no native media, network or UI |
| `crates/app` | Native FFmpeg, ONNX/Tesseract, providers, resources, atomic persistence, model admission, supervised extraction and logging | core + native adapters |
| `crates/cli` | User commands and the hidden worker protocol | app + core |
| `crates/ui` | Leptos CSR Rust/WASM controls, review, virtualization and typed IPC | core + browser bindings |
| `crates/desktop` | Tauri host, native dialogs, scoped grants, job lifetime and command ACL | app + core |
| `xtask` | CI fixture/model provisioning, real CLI/GUI checks, package inventory and gates | core + CI I/O |

## Extraction flow

A native picker grants a source token and output token; frontend paths are never arbitrary filesystem authority. The GUI and CLI invoke the same app service. A packaged `nvsf` child owns native inference and decoding per job. Owned RGB frames carry source presentation times; FFmpeg contexts and native pointers do not cross threads. A normalized stream origin and checked rational conversion precede export milliseconds.

Detector crops feed the selected recognition model. Appearance tracking separates disappearance/reappearance and changed numbers rather than globally merging identical text. Accurate mode examines each displayed decoded frame; balanced/fast modes reuse candidates with bounded refresh/change detection. Checkpoints store completed cues and active appearances at the last fully processed frame. They are source/config/model-bound, atomically persisted and process-locked. Native failures do not publish an incomplete multi-region frame as a valid checkpoint.

Cue IDs are unique within a project. Original OCR is kept separately from edited text. Undo/redo and structural edits operate through core validation; explicit re-OCR replacement never silently discards corrections. Export validates nonempty Unicode and positive intervals, then atomically writes to the selected destination.

## Isolation and limitations

The native worker has wall/idle limits, bounded progress messages, cancel/pause controls and owned-tree cleanup. Runtime libraries and model artifacts are explicit local resources, with no model download during inference. A hash proves byte identity, not language quality. Provider registration is not operator placement or accelerator certification.

The WebDriver plugin and fixed fixture/export controls exist only in the `e2e` host/frontend profiles. Production has a separate command table/ACL and no automation listener. Runtime verification must inspect both profiles, not infer production isolation from a test build.

Current axis-aligned DB boxes are a bounded approximation, not full rotated-polygon unclip parity. Final-frame duration can be estimated from the last observed source interval and is marked as such. SAR/HDR/display-matrix edge cases, optional specialist/VLM admission, held-out quality and physical accelerators remain explicit acceptance gates; see validation status.
