# Validation status

Updated: 2026-10-14. **NOT a release acceptance report.** Hosted observations below refer to the exact named SHA; corrective source is not a passing result.

| Area | Current evidence | State |
|---|---|---|
| Rust workspace, shared pipeline/CLI/UI source | Source authored; no local compilation allowed | Hosted verification pending |
| Six native CI builds/tests | On `d3ebc83f8cd7ba6ec702277371ab9a170b982633`, Linux x64/ARM64, both macOS architectures and Windows ARM64 native provisioning passed. Linux/macOS static analysis then failed four Clippy diagnostics; Windows ARM64 bindgen lacked `errno.h`. Windows x64's six guard contracts passed, but native progress observation failed during FFmpeg extraction. Intel macOS completed at 2026-10-13 23:36:57 UTC with the same four Clippy diagnostics; all six native outcomes have been collected | FAILED/pending; full application build/test not reached |
| Actual native OCR and GUI edits/export | Real harness source, not execution | NOT_RUN |
| Locked dependencies and model pins | Live official metadata/source reviewed; lock resolved without compilation | Runtime/ABI admission pending |
| Original logo | Owner-supplied image and 240px README sizing verified | Asset identity established; native package/render pending |
| Real English corpus | Licensed real video + counterpart SRT lead | Visible-text/timing annotation pending |
| Real Persian/Japanese corpus | Licensed real media candidates, no ready timed ground truth | Annotation and evaluation blocked |
| Physical accelerators | No device/self-hosted runner authorized | Unqualified, separate from hosted CPU |
| Optional Japanese specialist/local visual retry | Source wired, pinned models, proposal-only/cancel tests authored | Native admission/real crop quality NOT_RUN |
| Native installers, signing and relocation | Actual format/content/lifecycle harness source authored | NOT_RUN/unsigned |
| Coverage/mutation/Miri/security | CodeQL and the complete supply-chain job passed on `d3ebc83f8cd7ba6ec702277371ab9a170b982633`. Miri passed ten portable contracts with isolation enabled. The actual mutation baseline passed; the campaign recorded 79 caught, 44 missed, 6 unviable and zero timeouts. Independent review admitted all 123 executable mutations as critical/non-equivalent: 64.23%, below the unchanged 80% floor | Mutation FAILED; coverage unmeasured; replacement boundary fixtures are not yet execution evidence |
| Fuzz/Loom/native sanitizers | Parser properties/threaded stress are supplementary, not substitutes | Required deeper evidence open |

No successful CI, quality percentage, coverage number, screenshot, signing, GPU placement or release result is asserted without actual evidence. Required missing/failed evidence remains open rather than being waived to obtain a green badge. Build/test results must name their exact source SHA and actual target/jobs; hosted development checks and final release qualification are different gates.
