# Validation status

Updated: 2026-10-13. **NOT a release acceptance report.**

| Area | Current evidence | State |
|---|---|---|
| Rust workspace, shared pipeline/CLI/UI source | Source authored; no local compilation allowed | Hosted verification pending |
| Six native CI builds/tests | Workflow and target inventory authored; no run yet | NOT_RUN |
| Actual native OCR and GUI edits/export | Real harness source, not execution | NOT_RUN |
| Locked dependencies and model pins | Live official metadata/source reviewed; lock resolved without compilation | Runtime/ABI admission pending |
| Original logo | Owner-supplied image and 240px README sizing verified | Asset identity established; native package/render pending |
| Real English corpus | Licensed real video + counterpart SRT lead | Visible-text/timing annotation pending |
| Real Persian/Japanese corpus | Licensed real media candidates, no ready timed ground truth | Annotation and evaluation blocked |
| Physical accelerators | No device/self-hosted runner authorized | Unqualified, separate from hosted CPU |
| Optional Japanese specialist/local visual retry | Source wired, pinned models, proposal-only/cancel tests authored | Native admission/real crop quality NOT_RUN |
| Native installers, signing and relocation | Actual format/content/lifecycle harness source authored | NOT_RUN/unsigned |
| Coverage/mutation/Miri/security | Required workflow gates and thresholds retained | Unmeasured; full WASM/changed-line/critical review open |
| Fuzz/Loom/native sanitizers | Parser properties/threaded stress are supplementary, not substitutes | Required deeper evidence open |

No successful CI, quality percentage, coverage number, screenshot, signing, GPU placement or release result is asserted without actual evidence. Required missing/failed evidence remains open rather than being waived to obtain a green badge. Build/test results must name their exact source SHA and actual target/jobs; hosted development checks and final release qualification are different gates.
