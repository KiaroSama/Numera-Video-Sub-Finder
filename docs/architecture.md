# Architecture

## Boundaries

| Component | Responsibility | Dependency direction |
|---|---|---|
| `crates/core` | Checked source time, normalized regions, cue/project contracts, appearance tracking, edits and text export | serde/errors only; no native media, network or UI |
| `crates/app` | Explicit media loader, ONNX/Tesseract, providers, resources, signed maintenance, atomic persistence, supervised extraction and logging | core + native adapters |
| `crates/media-native` | FFmpeg-owned decoder cdylib and versioned synchronous borrowed-buffer C ABI | core + FFmpeg; no application/UI/network |
| `crates/cli` | User commands and the hidden worker protocol | app + core |
| `crates/ui` | Leptos CSR Rust/WASM controls, review, virtualization and typed IPC | core + browser bindings |
| `crates/desktop` | Tauri host, native dialogs, scoped grants, job lifetime and command ACL | app + core |
| `xtask` | CI fixture/model provisioning, real CLI/GUI checks, signed candidate inventory and gates | app + core + CI I/O |

## Extraction flow

A native picker grants a source token and output token; frontend paths are never arbitrary filesystem authority. The GUI and CLI invoke the same app service. A packaged `nvsf` child owns native inference and decoding per job. Owned RGB frames carry source presentation times; FFmpeg contexts and native pointers do not cross threads. A normalized stream origin and checked rational conversion precede export milliseconds.

Detector crops feed the selected recognition model. Appearance tracking separates disappearance/reappearance and changed numbers rather than globally merging identical text. Accurate mode examines each displayed decoded frame; balanced/fast modes reuse candidates with bounded refresh/change detection. Checkpoints store completed cues and active appearances at the last fully processed frame. They are source/config/model-bound, atomically persisted and process-locked. Native failures do not publish an incomplete multi-region frame as a valid checkpoint.

Cue IDs are unique within a project. Original OCR is kept separately from edited text. Undo/redo and structural edits operate through core validation; explicit re-OCR replacement never silently discards corrections. Export validates nonempty Unicode and positive intervals, then atomically writes to the selected destination.

Native close and application exit use the same host-owned handshake: freeze WebView input, acknowledge actual Rust UI dirty/draft/revision/busy state, obtain discard consent when needed, recheck that state and then perform bounded shutdown. Missing or changed acknowledgements preserve the open workspace. Completed extraction projects wait in UI state until a deferred close returns control; global save/undo cannot mutate the review while consent is pending.

## Native admission and maintenance

The app/CLI/desktop do not startup-link FFmpeg. `MediaRuntime` loads only the selected explicit native root and retains the library across callbacks. Requests/metadata use bounded JSON; RGB descriptors are borrowed synchronously and copied before return. ABI version/layout/pointer width/architecture/library majors are checked before frame delivery; panics never unwind across C. Every media caller receives its existing `Paths`, including worker, preview, region suggestions and re-OCR.

Bundled, authenticated managed and explicit runtime roots share that path contract. Managed packs require project signature, exact version/target, complete bounded inventory, hashes and native API checks before atomic selection; corrupt selection leaves diagnostics/repair reachable. Tesseract/ORT API-version health is not model/operator/provider qualification. Native package provenance binds the final relocated adapter/closure to source SHA and lockfile. System PATH executables are not native-library admission.

The host performs bounded signed official stable-release metadata checks, with persisted max-daily cadence/disable and manual check. Session-held offers require distinct download/install approval; cancellation, resource/job/source/closing/unsaved state is rechecked before native installation. Portable/deb/rpm are save-and-manual-install only. Hosted runtime and updater asset producers consume the same-source native inventories without rebuilding, authenticate candidate manifests and final payload bytes, and do not publish a stable feed. Updater signatures do not replace platform-native final signing. See [installation](installation.md) for network/data behavior and current distribution blockers.

The shared official language catalog supplies canonical IDs/aliases, on-demand packs and exact recognizer-family policy. Unrelated scripts never fall back to the English/Japanese recognizer; Tesseract-primary languages report CPU recognition and reject required whole-pipeline acceleration. Selected-resource fingerprints exclude unrelated installed languages. Catalog inclusion does not establish measured accuracy.

## Isolation and limitations

The native worker has wall/idle limits, bounded progress messages, cancel/pause controls and owned-tree cleanup. On Unix its isolated group is additionally bound to a supervisor-owned unwritten stdin pipe: a worker-side guardian terminates that group if the parent dies or closes the lease, even when forced parent termination bypasses Rust destructors. On Windows, the supervisor creates a noninherited kill-on-close Job Object and associates the worker before sending its single startup byte; the worker waits for that byte before native work. Closing the owner's last job handle, including on forced supervisor exit, terminates job members. At exact `96dc417`, both Windows targets pass the native job-close contract and their ordinary extraction/GUI/package checks. Worker output now has one supervisor-owned reader rather than a blocking reader thread: Unix uses nonblocking descriptor reads and Windows observes available bytes on the standard library's overlapped child pipe before reading. Partial UTF-8/JSON events remain buffered within64MiB, and an exited root whose output makes no further read/event progress fails after two seconds. The framing and actual-child pipe regressions pass on the inspected Windows/Linux logs at exact `96dc417`; the complete six-target native workflow succeeds. Native preview/re-OCR inside desktop blocking threads and full descendant-fault qualification remain separate unresolved isolation/bounding gaps. Runtime libraries and model artifacts are explicit local resources, with no model download during inference. A hash proves byte identity, not language quality. Provider registration is not operator placement or accelerator certification.

The WebDriver plugin and fixed fixture/export controls exist only in the `e2e` host/frontend profiles. Production has a separate command table/ACL and no automation listener. Runtime verification must inspect both profiles, not infer production isolation from a test build.

Current axis-aligned DB boxes are a bounded approximation, not full rotated-polygon unclip parity. Final-frame duration can be estimated from the last observed source interval and is marked as such. SAR/HDR/display-matrix edge cases, optional specialist/VLM admission, held-out quality and physical accelerators remain explicit acceptance gates; see validation status.
