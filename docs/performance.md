# Performance and resource policy

No performance measurements are available until a real hosted or separately authorized device run has completed. Source optimizations are not speed claims.

Accurate mode inspects each decoded presentation frame. Balanced/fast combine a bounded appearance signature with periodic OCR (200/500 ms respectively); missed short events are a declared sampling risk, not equivalent coverage. Source PTS, not average FPS, controls timestamps. Regions are bounded to eight, exclusions to 32, decoded dimensions to the configured pixel limit, cues to 100000 and threads to 1–16. Hosted runs use two threads.

FFmpeg owns decoder contexts on one thread; queued data are owned RGB images. A worker process isolates native lifetime and has a 24-hour wall/300-second active-idle ceiling. Optional visual retry has 120-second wall/60-second idle ceilings and bounded tokens/context/image/response. Native preview/reOCR currently run on a host blocking task; a native hang there is not yet independently killable and is an open robustness limit.

Checkpoint rollback retains the completed cue count plus at most eight active appearances, not a whole growing-project clone each frame. Full checkpoint serialization remains periodic. Undo/redo still uses bounded snapshots; a single exceptionally large snapshot can exceed the nominal 32 MiB history budget and has not been qualified for 100000-cue workloads.

Benchmark results must record source/model/hash, range/regions, quality/precision/provider, frame count, wall time, RAM/VRAM, transfers/fallbacks, hardware/OS and warm/cold state. Compare identical workloads. GPU availability is not operator placement or a speedup. Physical acceleration and performance are currently unqualified because only public CPU runners were authorized.
