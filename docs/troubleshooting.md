# Troubleshooting

- Missing native runtime: keep the target-correct GUI, nvsf worker, runtimes and resources together. Reinstall/unpack the matching package; do not copy a DLL/dylib from another architecture or silently use developer PATH.
- Missing/corrupt model: use explicit model installation or import the exact checksummed ONNX file. Existing working bytes stay intact after integrity failure. Do not delete a partial download until its useful recovery status is known.
- Resume mismatch: checkpoint source/config/model hashes must match. Current configuration includes absolute paths/data root; a moved source/checkpoint needs explicit compatible regrant and is not silently rewritten. Preserve checkpoints and corrected project data.
- Output exists or unavailable: choose another destination or explicitly approve replacement. Each output is atomically published; multi-track files cannot be a single cross-file filesystem transaction. The combined file is published last.
- Required accelerator unavailable: an ordinary CPU package cannot prove accelerator operator placement. Use CPU only if the requested policy permits; do not replace a strict failure with an unverified CPU/GPU claim.

A refused close preserves the open workspace. Finish an active review operation, apply/save or explicitly discard the current draft, then close again. Missing or changed review acknowledgements are recorded in the desktop log; do not force-terminate the application while corrections need saving.

Use doctor for resource inventory, not quality or acceleration certification. Logs are in the selected data root/logs; they use UTC and unique run names with bounded historical retention. Review private paths before sharing diagnostics. Preserve failure evidence; never attach media, crops, credentials or complete private project data to a public issue by default.

Unsigned macOS distribution may be rejected by platform policy. Do not globally disable Gatekeeper/signature checks. Installer/signing/IME/device limitations are listed in validation rather than hidden behind a successful compile.
