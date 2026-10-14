# Validation status

Updated: 2026-10-14. **NOT a release acceptance report.** Hosted observations below refer to the exact named SHA; corrective source is not a passing result.

| Area | Current evidence | State |
|---|---|---|
| Rust workspace, shared pipeline/CLI/UI source | Source authored; no local compilation allowed | Hosted verification pending |
| Six native CI builds/tests | On `54919bf9514d982c5d89ff19c544396d481b2588`, both Linux targets and Windows x64 passed native provisioning, Rust static analysis and CLI compilation. Linux then failed Leptos frontend macro parsing; Windows x64 exceeded the Trunk tool-install idle ceiling. Windows ARM64 SDK/native provisioning succeeded and Rust Clippy exited zero, but an actual surviving owned process failed the guard. Apple Silicon failed the build-log sampler's inventory/work bound. Intel macOS also passed provisioning/static/CLI, then failed the same Trunk installation idle ceiling; its terminal outcome was collected at 2026-10-14 15:49:23 UTC. All six native outcomes are complete | FAILED/pending; GUI, extraction and package stages not reached |
| Actual native OCR and GUI edits/export | Real harness source, not execution | NOT_RUN |
| Locked dependencies and model pins | Live official metadata/source reviewed; lock resolved without compilation | Runtime/ABI admission pending |
| Original logo | Owner-supplied image and 240px README sizing verified | Asset identity established; native package/render pending |
| Real English corpus | Licensed real video + counterpart SRT lead | Visible-text/timing annotation pending |
| Real Persian/Japanese corpus | Licensed real media candidates, no ready timed ground truth | Annotation and evaluation blocked |
| Physical accelerators | No device/self-hosted runner authorized | Unqualified, separate from hosted CPU |
| Optional Japanese specialist/local visual retry | Source wired, pinned models, proposal-only/cancel tests authored | Native admission/real crop quality NOT_RUN |
| Native installers, signing and relocation | Actual format/content/lifecycle harness source authored | NOT_RUN/unsigned |
| Coverage/mutation/Miri/security | CodeQL and the complete supply-chain job passed on `54919bf9514d982c5d89ff19c544396d481b2588`. Miri passed 16 portable contracts with isolation enabled. Mutation recorded 123 caught, zero missed/timeouts and 6 unviable. All 129 candidate identities and reviewed core production files match the independently reviewed prior campaign: 123 critical/non-equivalent executable candidates, 100% killed; unviable cases never count as kills | Portable mutation/Miri/security PASSED; native/WASM/changed-line coverage unmeasured |
| Fuzz/Loom/native sanitizers | Parser properties/threaded stress are supplementary, not substitutes | Required deeper evidence open |

No successful CI, quality percentage, coverage number, screenshot, signing, GPU placement or release result is asserted without actual evidence. Required missing/failed evidence remains open rather than being waived to obtain a green badge. Build/test results must name their exact source SHA and actual target/jobs; hosted development checks and final release qualification are different gates.
