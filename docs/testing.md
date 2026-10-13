# Testing and evidence

All application compilation and execution checks run in GitHub Actions on the exact commit. The six native target rows are required, with maximum three concurrently hosted jobs, two build/test/native threads per runner, command/HTTP/job bounds and owned process cleanup. Missing jobs, wrong source SHA and missing reports fail the aggregate; no broad continue-on-error or fake passing badges.

## Tiers

1. Portable core contracts: rational source PTS, overflow, Unicode/long-hour export, strict parser, geometry, repeated appearances, checkpoint restoration and edit rollback. Tiny literal fixtures and bounded property cases avoid network/native startup.
2. Native contracts: real atomic files/locks, verified model/runtime metadata and native media/decode paths. Synthetic extraction uses actual FFmpeg and ONNX, not a mocked recognizer.
3. Actual instrumented GUI: native startup/source admission/preview, themes, Start job, review edits and exported Unicode bytes. W3C WebDriver acts through controls; only fixed test grants bypass OS picker interaction. DOM/styles/screenshots are actual artifacts.
4. Standalone package: matching native executables/resources and hashes, launch from a relocated directory with spaces, no checkout library path dependency, offline model load and actual extraction. Installer/ABI checks have separate evidence.
5. Qualification: independently annotated real en/fa/ja hardsub data, CER/WER/event recall/source-frame boundaries, coverage/mutation/fuzz/Miri/sanitizers and physical GPU placement/performance. Missing qualification does not become PASS because CPU smoke succeeds.

## Planned thresholds

English CER ≤2%; Persian/Japanese CER ≤5%; accurate-mode event recall ≥98%; p95 boundary error ≤max(100ms, two displayed-frame intervals). Allowed precision-related CER regression ≤0.5 percentage points. Instrumentable owned Rust coverage ≥80%; timing/tracking/export/persistence and changed lines ≥90%; reviewed critical mutations ≥80%. These are acceptance targets, not measured results.

Before native provisioning, `scripts/ci/guard-tests.ps1` exercises the real guard with captured Unicode output, nonzero exit propagation, a deliberately surviving child that must be rejected and reaped, a quiet build with actual log writes and a frozen-log idle failure. Each command has a 15-second wall/5-second idle ceiling; the regression step has a two-minute ceiling. Interpreter lookup selects one executable; exited root handles are released before owned-tree accounting, with a two-second bounded stream/teardown grace. Windows job checks enumerate actual associated process IDs and probe signaled process handles rather than counting retained terminated process objects as live descendants; PID reuse outside the owned job is excluded. A Windows-only regression holds a real exited child's process handle outside the job until the guard has returned, using an explicit PID/start-time signal rather than a startup sleep. Live descendants still fail the guard. Native vcpkg builds write into their own buildtree log files; the same guard can observe actual LastWrite/Size changes in that exact project-contained directory without exposing log bodies or resetting idle time merely because a process is alive. Frozen output/logs still trigger the unchanged idle ceiling.

Every created test is audited before its run: load-bearing assertions, cheapest evidence tier, shared immutable setup, isolated mutable state, bounded waits, two-worker ceiling and full owned-tree cleanup. Initial measured wall times are unknown until the first hosted run. No test is run locally and again in CI.

## Evidence integrity

Synthetic media cannot certify real OCR quality. Automatic OCR/VLM labels cannot certify themselves as ground truth. A real screenshot is not proof of exact colors or IME composition; computed styles and dedicated composition/keyboard scenarios are required. Provider availability cannot prove GPU operator execution. `docs/validation.md` separates authored, not-run, failed and verified states.
