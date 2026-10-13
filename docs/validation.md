# Validation status

Updated: 2026-10-13. **NOT a release acceptance report.**

| Area | Current evidence | State |
|---|---|---|
| Rust workspace, shared pipeline/CLI/UI source | Source authored; no local compilation allowed | Hosted verification pending |
| Six native CI builds/tests | On `61a5f818`, Linux x64/ARM64 native provisioning and five-case guard contracts passed; Rust Clippy then failed. Windows rows failed in visual-helper configuration/process accounting; Both macOS rows failed native font-library closure; Intel ONNX Runtime source compilation completed before that closure failure | FAILED/pending; full application build/test not reached |
| Actual native OCR and GUI edits/export | Real harness source, not execution | NOT_RUN |
| Locked dependencies and model pins | Live official metadata/source reviewed; lock resolved without compilation | Runtime/ABI admission pending |
| Original logo | Owner-supplied image and 240px README sizing verified | Asset identity established; native package/render pending |
| Real English corpus | Licensed real video + counterpart SRT lead | Visible-text/timing annotation pending |
| Real Persian/Japanese corpus | Licensed real media candidates, no ready timed ground truth | Annotation and evaluation blocked |
| Physical accelerators | No device/self-hosted runner authorized | Unqualified, separate from hosted CPU |
| Optional Japanese specialist/local visual retry | Source wired, pinned models, proposal-only/cancel tests authored | Native admission/real crop quality NOT_RUN |
| Native installers, signing and relocation | Actual format/content/lifecycle harness source authored | NOT_RUN/unsigned |
| Coverage/mutation/Miri/security | CodeQL and the complete supply-chain job passed on `61a5f818`. Miri sysroot setup passed, then isolated property persistence failed; mutation copy/core compilation passed, then baseline test arguments failed | Quality gates failed/unmeasured; full WASM/changed-line/critical review open |
| Fuzz/Loom/native sanitizers | Parser properties/threaded stress are supplementary, not substitutes | Required deeper evidence open |

No successful CI, quality percentage, coverage number, screenshot, signing, GPU placement or release result is asserted without actual evidence. Required missing/failed evidence remains open rather than being waived to obtain a green badge. Build/test results must name their exact source SHA and actual target/jobs; hosted development checks and final release qualification are different gates.
