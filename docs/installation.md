# Installation and first setup

Current status: development source, not a completed release. All four native target checks and platform-signing gates must pass before a ready-build prerelease is published. Do not treat an Actions log, an unsigned intermediate or an empty release as an installer. See [validation](validation.md) for exact observed results and open qualification.

## 1. Choose the correct build

Use only the project's [official Releases](https://github.com/KiaroSama/Numera-Video-Sub-Finder/releases). A development prerelease is experimental: package availability does not certify OCR accuracy, acceleration, minimum-OS support or installation lifecycle.

| Computer | Architecture | Intended downloads | Current prerequisite/baseline |
|---|---|---|---|
| Windows | x64 | per-machine MSI installer or portable ZIP | Windows 10 1809+ declared; system WebView2 |
| Windows | ARM64 | ARM64 per-machine MSI installer or portable ZIP | native ARM64 Windows; system WebView2 |
| Linux | x64 | AppImage, deb, rpm or portable tar.gz | Ubuntu 24.04/glibc 2.39 build baseline; WebKitGTK 4.1/system graphics libraries |
| Linux | ARM64 | ARM64 AppImage, deb, rpm or portable tar.gz | same declared Linux baseline on ARM64 |

Check your operating system's About/System information screen before downloading. x64 and ARM64 are not interchangeable. No Android/iPhone application is supplied. The installed application does not need Rust, Python, Node, WSL or Docker.

## 2. Verify and install

1. Download only your target's asset, accompanying checksum/provenance/signature information and required notices. Compare the exact file size and SHA-256 with the published inventory. A checksum detects byte changes; authenticity also requires the approved signature/trust path.
2. Windows development executables/installers require the exact pinned self-signed Authenticode identity described below; this does not make the publisher publicly trusted. Linux packages remain unsigned at the native package level; check their published inventory and any separately supplied updater/detached authentication according to its documented purpose. The existing updater key/signatures are unchanged and are not Authenticode or native deb/rpm signatures. Do not install when required signatures are absent, unexpected, expired or invalid.
3. MSI: run the verified per-machine `.msi` installer (users upgrading from `0.4.0` or earlier NSIS installs should uninstall the old NSIS package once first). It may fetch Microsoft's WebView2 bootstrapper if WebView2 is missing. Portable ZIP: extract the complete directory into a writable location; do not run directly inside the ZIP or separate the GUI, `nvsf.exe`, `runtimes`, data and notices.
4. Linux: use your distribution's normal package installer for deb/rpm, or mark the verified AppImage executable and run it. Extract portable tar.gz completely before launching. System-library compatibility must match the build baseline.
5. Keep OS security enabled. Do not disable SmartScreen or signature checks to make an unverified package run.

Minimum-OS and clean-install behavior remain unqualified until their dedicated tests pass. A successful development-runner launch is not proof of those scenarios.

### Windows development signature check

The signing path covers the portable GUI and `nvsf.exe`, the MSI-packaged GUI, and the final `.msi` installer. Vendor DLLs are not re-signed.

The development signer is self-signed `CN=Numera Video Sub Finder`, RSA 3072 with code-signing EKU `1.3.6.1.5.5.7.3.3`, expiring 2027-11-03. Its public certificate DER SHA-256 pin is:

```text
fd066bb47fb069723b76650fe628cf4726b6f710b8245097d055312a8a912317
```

In PowerShell 7 on Windows, set `$file` to the downloaded setup or extracted owned executable. These commands inspect the signature and certificate without executing the application or importing a certificate:

```powershell
$file = (Resolve-Path -LiteralPath '.\nvsf.exe').Path
$signature = Get-AuthenticodeSignature -LiteralPath $file
$signature | Format-List Status, StatusMessage
$cert = $signature.SignerCertificate
if ($null -eq $cert) { throw 'No signer certificate is present.' }
$actual = [Convert]::ToHexString([Security.Cryptography.SHA256]::HashData($cert.RawData)).ToLowerInvariant()
$expected = 'fd066bb47fb069723b76650fe628cf4726b6f710b8245097d055312a8a912317'
$cert | Format-List Subject, NotBefore, NotAfter
if ($actual -cne $expected) { throw 'Signer certificate SHA-256 pin does not match.' }
"Certificate DER SHA-256 matches: $actual"
```

Expected warning: ordinary Windows policy does not trust this self-signed root. `Get-AuthenticodeSignature` can show `UnknownError` with an untrusted-root message (or `NotTrusted`); Windows may also show an untrusted publisher/SmartScreen warning. The precise required hosted ordinary-policy result is `0x800B0109`, not a general allowance for any error. A pin match alone does not prove the file's Authenticode digest: compare the complete downloaded asset's size/SHA-256 with the published inventory and require its exact-source verification report. The hosted verifier separately requires a valid CMS signature, the exact pin and WinVerifyTrust hash-only status `0`, and rejects tampered/wrong-pin copies. `HashMismatch`, missing/unexpected signer, expiry or any unrelated failure is not the expected warning.

There is no timestamp; signatures stop validating after certificate expiry. Never import this certificate into Trusted Root, Trusted People or another trust store, and do not change OS protections to hide the warning. Pinning checks identity/integrity under the explicitly limited development policy, not public publisher reputation. The hosted signer uses only an ephemeral runner's Personal store, then removes its identity. These commands and the new hosted signing checks have not been executed for the current source.

### Embedded application logo

The Windows GUI uses the existing `tauri-build` icon resource; the CLI adds a native resource through `tauri-winres 0.3.7`. Final PE resource checks compare the generated ICO with actual GUI/CLI/setup/uninstaller group-icon descriptors and image bytes. Native package checks resolve Linux hicolor PNGs and desktop `Icon=`, plus AppImage `.DirIcon`. These checks are implemented pending hosted verification; adjacent logo files or Explorer's cached thumbnail alone are not proof of correct embedding.

## 3. First launch and native resources

Open Settings → Resources and diagnostics → Run resource checks. The report distinguishes actual FFmpeg media-adapter ABI, Tesseract version and ONNX API/version checks from model/operator/hardware qualification.

Compatible bundled or selected managed resources are reused. If a native component is missing/incompatible, review the explicit repair approval and choose Repair approved dependencies. Repair downloads only a matching project-signed application-local runtime pack, checks its inventory/hash/target/native APIs, and activates a new selection atomically. It does not install an arbitrary `ffmpeg.exe`, change system PATH, replace another program or require a system package manager. Previous packs and saved projects are retained. Close and reopen the app after successful repair.

A matching signed pack must actually exist on the official release. A missing pack produces an error; no fake installation or alternate untrusted download is attempted. Explicit `NVSF_RUNTIME_ROOT` overrides remain authoritative; remove a stale override yourself before expecting a managed pack to be selected. Cancellation may wait for an in-flight blocking network read to time out.

CLI diagnosis, from the extracted package directory:

```bash
./nvsf dependencies
```

Explicit repair approval:

```bash
./nvsf dependencies --repair --consent
```

On Windows the executable is `nvsf.exe`. Keep the CLI with its matching application/native resources.

## 4. Install only the OCR resources you need

In Models, install the baseline detector/recognizers explicitly, or import their exact checksummed artifacts using the native picker. Bundled verified baseline resources are copied to writable application data when available; inference itself never starts a model download.

The language dropdown contains 123 official Tesseract language and vertical variants, covering English, Persian, Japanese, and 120 other scripts. `eng/fas/jpn` and existing `en/fa/ja` aliases are accepted. Select a language in Additional language packs and download that pack, or import its exact official checksummed `.traineddata`. The default baseline installation does not fetch the entire language catalog.

```bash
./nvsf models install
```

```bash
./nvsf languages list
```

```bash
./nvsf languages install fra
```

```bash
./nvsf languages verify fra
```

The admitted Chinese/English/Japanese and Arabic-family Paddle models are used only for their compatible languages. Other catalog languages use Tesseract CPU recognition plus the native detector; strict required-acceleration mode therefore cannot pass for those recognizers. `osd`, `equ`, arbitrary paths and combined expressions such as `eng+fra` are not selectable language IDs.

A separate Windows x64 CUDA variant (`windows-x64-cuda12`, status `development-unqualified-gpu`) is built in CI. It contains only `nvsf.exe` with ONNX Runtime 1.31.0 and its CUDA provider; the host must supply the NVIDIA driver, the CUDA 12 runtime, cuBLAS and cuDNN 9, and the media/OCR runtimes come from the same-version Windows x64 portable package (`NVSF_RUNTIME_ROOT`). cuDNN is loaded by name, so its folder must be on `PATH`. Two physical RTX 4070 Ti checks found about 1.5 to 1.8 times faster extraction on a 60-second clip with identical output, but 64 recognizer shape nodes still run on CPU. ONNX Runtime places small shape subgraphs on CPU by design, so `--gpu-required` admits CPU nodes only for shape operators (`Shape`, `Size`, `Slice`, `Concat`, `Gather`, `Unsqueeze`, `Squeeze`, `Reshape`, `Cast`, `Range`, `ConstantOfShape`) and fails if any other operator lands on CPU. See `docs/validation.md`; no other GPU, DirectML or broader performance claim is made. The four CPU baseline products remain distinct.

Pack availability and hash verification do not measure subtitle accuracy. Original English/Persian/Japanese accuracy/timing gates remain open; other languages are explicitly unmeasured. See [OCR contracts](ocr-models.md).

## 5. Extract and save

Load a local video in either of two ways:

- **Single video:** Click **Open video**, or drag and drop one video file (`.mp4`, `.mkv`, `.mov`, `.avi`, `.webm`, `.m4v`, `.ts`, `.mts`, `.m2ts`, `.mpeg`, `.wmv`) onto the window to open it in **Extract**, then choose a distinct `.srt` destination.
- **Multiple videos or a folder:** Drag and drop several video files or a folder onto the window. The app scans dropped folders up to 4 levels deep (capped at 100 videos, skipping symlinks and non-video files), creates non-colliding sibling `.srt` destinations, and adds every video to **Queue** with your current extraction settings.

Type any word, native script (`فارسی`, `日本語`), or ISO/Tesseract code (`fa`, `ja`, `chi_sim`, `deu`) into the **Subtitle language** search box to filter the 126 supported languages immediately, then click a match or press `Enter`. Set regions/exclusions and time range, starting with a short representative section. Automatic region suggestions are editable, not automatic acceptance of scene signs.

```bash
./nvsf extract --input "video with subtitles.mp4" --output "subtitles.srt" --language fra --quality accurate --device auto
```

CLI extraction, benchmark, resume and batch return 2 when completed output contains any cue still marked unreviewed, matching the workspace's Needs review filter. This is a usable completed result, not an extraction failure: the subtitles and checkpoint are preserved. An empty or fully reviewed result returns 0. Other statuses are 3 invalid arguments/configuration, 4 operational/extraction failure, 5 cancellation and 6 unavailable required provider; help/version return 0. Batch continues past completed review-needed items but stops on an actual failure. Diagnostic messages stay on stderr; extraction summaries on stdout are JSON.

SRT, VTT and ASS keep distinct tracks separate. For multiple tracks, the chosen output contains the lowest-numbered primary track, with `.track-<id>.<format>` files for every track; it does not silently combine bilingual tracks. JSON retains the complete project. ASS represents time in centiseconds, enclosing millisecond intervals with at most 9 ms expansion at each edge so a short cue is never silently erased.

Use Zoom (100% to 400%), native scrollbars or the labeled pan buttons to inspect a frame; Reset view restores fit without changing source pixels or normalized regions. In Review, expand Compare original and selected-frame crops and load the content-verified original extraction crop. The candidate shows the unmasked track region at the current source time. A moved/missing cache is reported honestly; it is never replaced with a newly decoded image and labelled original.

Review source frames and uncertain cues before exporting. Save a review project before changing source, starting another job, repairing resources or installing an update. Checkpoints require matching source/configuration/selected-resource identities; installing unrelated language packs does not change that fingerprint. A checkpoint from an incompatible older fingerprint scheme is refused instead of silently resumed.

## Updates, data and troubleshooting

- Settings → Application updates controls automatic stable-release metadata checks, a persistent disable option and manual checks. Checks run at startup and at most daily while open; video/subtitles are not uploaded. Downloads and installation have separate explicit approvals bound to the offered version. Development prereleases are never served by the stable feed.
- Automatic replacement is intended only for installed Windows and Linux AppImage. Portable archives/deb/rpm provide a verified saved download and require their normal installation procedure. Jobs, previews, resource operations, closing and unsaved edits block installation; do not terminate the app to force it through.
- Writable models/projects/cache/logs use OS local application data under `Numera Video Sub Finder`. `--data-root` / `NVSF_DATA_ROOT` selects another location. A portable package's `portable.flag` uses sibling `data`. Preserve that data during an update; do not delete it to diagnose a runtime issue.
- Run `doctor` or the resource checks for exact errors. Logs are UTF-8 under the selected data root's `logs` directory; redact private paths/context before voluntarily sharing a log. Missing hardware/quality evidence is not a runtime-install error. See [troubleshooting](troubleshooting.md), [models](models.md) and [validation](validation.md).

## Source builds

Build and test only in GitHub Actions. On the authorized repository, manually run Final verification (`verify.yml`) for the completed pushed source. It invokes reusable `ci.yml` for the four native targets and the portable/security/source-analysis children once. No local Cargo/Trunk/Tauri compilation or local application-test fallback is part of this project's workflow. The new Windows development signing/icon implementation still needs its own exact-SHA hosted verification, and final release publication still needs all required evidence.
