# Validation status

Updated: 2026-10-14. **NOT a release acceptance report.** Hosted observations below refer to the exact named SHA; corrective source is not a passing result.

| Area | Current evidence | State |
|---|---|---|
| Rust workspace, shared pipeline/CLI/UI source | Source authored; no local compilation allowed | Hosted verification pending |
| Six native CI builds/tests | On `3366184ae1d83bb274fa6da0b042d85ac1b26950`, both Linux targets and Apple Silicon passed native provisioning, Rust analysis, CLI compilation and Trunk installation; the earlier macro parser and Apple Silicon sampler failures did not recur. All three then failed frontend model-ID ownership (`E0382`). Windows x64 and ARM64 each failed the unchanged process guard on a real owned `vctip` survivor after successful root exit and complete stream draining; cleanup was verified. Intel macOS passed provisioning, analysis, CLI compilation and Trunk installation, then exceeded the 1,200-second Tauri CLI source-install wall limit (exit 124, cleanup verified). Its last output was 54 seconds before termination, so this was not the 180-second idle limit. All six terminal outcomes were collected before the next correction push. Verified official Intel-only Tauri CLI archive admission is authored; the application build remains CI-only. Corrective model-ID cloning and documented hosted-only optional-telemetry policy are authored, not passing results | FAILED/pending; native GUI, extraction and packages not yet qualified |
| Actual native OCR and GUI edits/export | Real harness source, not execution | NOT_RUN |
| Locked dependencies and model pins | Live official metadata/source reviewed; lock resolved without compilation | Runtime/ABI admission pending |
| Original logo | Owner-supplied image and 240px README sizing verified | Asset identity established; native package/render pending |
| Real English corpus | Licensed real video + counterpart SRT lead | Visible-text/timing annotation pending |
| Real Persian/Japanese corpus | Licensed real media candidates, no ready timed ground truth | Annotation and evaluation blocked |
| Physical accelerators | No device/self-hosted runner authorized | Unqualified, separate from hosted CPU |
| Optional Japanese specialist/local visual retry | Source wired, pinned models, proposal-only/cancel tests authored | Native admission/real crop quality NOT_RUN |
| Native installers, signing and relocation | Actual format/content/lifecycle harness source authored | NOT_RUN/unsigned |
| Coverage/mutation/Miri/security | On `3366184ae1d83bb274fa6da0b042d85ac1b26950`, CodeQL and portable quality passed. Miri passed 16 contracts with isolation enabled. Actual mutation phases recorded 123 successful builds followed by test-failure kills, zero missed/timeouts and 6 build-unviable candidates; the 129 identities and four core production files match the prior independent critical/non-equivalent review, giving 123/123 (100%). Unviable cases never count as kills. The new supply-chain audit failed four newly issued rustls advisories against locked 0.23.45; the 0.23.46 patch is source-resolved but not yet CI-verified | Portable mutation/Miri/CodeQL PASSED; supply-chain FAILED; native/WASM/changed-line coverage unmeasured |
| Fuzz/Loom/native sanitizers | Parser properties/threaded stress are supplementary, not substitutes | Required deeper evidence open |

No successful CI, quality percentage, coverage number, screenshot, signing, GPU placement or release result is asserted without actual evidence. Required missing/failed evidence remains open rather than being waived to obtain a green badge. Build/test results must name their exact source SHA and actual target/jobs; hosted development checks and final release qualification are different gates.
