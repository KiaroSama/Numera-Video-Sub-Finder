# Troubleshooting

The four supported 64-bit targets are Windows x64, Windows ARM64, Linux x64 and Linux ARM64.

- Missing native runtime: keep the target-correct GUI, nvsf worker, runtimes and resources together. Reinstall/unpack the matching package; do not copy a DLL/shared library from another architecture or silently use developer PATH.
- Missing/corrupt model: use explicit model installation or import the exact checksummed ONNX file. Existing working bytes stay intact after integrity failure. Do not delete a partial download until its useful recovery status is known.
- Resume mismatch: checkpoint source/config/model hashes must match. Current configuration includes absolute paths/data root; a moved source/checkpoint needs explicit compatible regrant and is not silently rewritten. Preserve checkpoints and corrected project data.
- Output exists or unavailable: choose another destination or explicitly approve replacement. Each output is atomically published; multi-track files cannot be a single cross-file filesystem transaction. The combined file is published last.
- Required accelerator unavailable: an ordinary CPU package cannot prove accelerator operator placement. Use CPU only if the requested policy permits; do not replace a strict failure with an unverified CPU/GPU claim.

A refused close preserves the open workspace. Finish an active review operation, apply/save or explicitly discard the current draft, then close again. Missing or changed review acknowledgements are recorded in the desktop log; do not force-terminate the application while corrections need saving.

Use doctor for resource inventory, not quality or acceleration certification. Logs are in the selected data root/logs; they use UTC and unique run names with bounded historical retention. Review private paths before sharing diagnostics. Preserve failure evidence; never attach media, crops, credentials or complete private project data to a public issue by default.

## Windows development signature warnings

The new Authenticode path is implemented pending hosted verification. Its pinned self-signed development publisher is not publicly trusted: an untrusted-publisher/SmartScreen warning, or `Get-AuthenticodeSignature` reporting `UnknownError` with an untrusted-root message (sometimes `NotTrusted`), is expected. Follow the [read-only signer-pin check](installation.md#windows-development-signature-check) and compare the exact asset inventory/report. A pin match alone is not file-digest verification, and an arbitrary error is not an accepted warning: missing/wrong signer, `HashMismatch`, expiry or another policy failure must stop installation.

No trust-store import or timestamp is used. The certificate expires 2027-11-03, after which its untimestamped signatures stop validating. Do not import it into Trusted Root/Trusted People, disable SmartScreen or alter security policy to suppress warnings. Vendor DLLs retain their original bytes/signatures. If a final icon-resource check fails, rebuild the correct artifact in hosted CI; an adjacent logo or cached shell thumbnail is not a repair.

Linux packages remain unsigned at the native level; separate updater signatures are unchanged and do not imply native package trust. Installer/signing/IME/device limitations stay open in [validation](validation.md), not hidden behind a successful compile.

GPU profiling and the separate Windows x64 CUDA variant are still in progress. Physical RTX 4070 Ti qualification has not executed and will use only exact CI-built artifacts. Provider availability or a selected GPU label is not proof of node placement or speedup; retain the strict failure rather than inventing a fallback success.
