# Installation and first setup

**Current status: development source, not a completed release.** All six native target checks and platform-signing gates must pass before a ready-build prerelease is published. Do not treat an Actions log, an unsigned intermediate or an empty release as an installer. See [validation](validation.md) for exact observed results and open qualification.

## 1. Choose the correct build

Use only the project's [official Releases](https://github.com/KiaroSama/Numera-Video-Sub-Finder/releases). A development prerelease is experimental: package availability does not certify OCR accuracy, acceleration, minimum-OS support or installation lifecycle.

| Computer | Architecture | Intended downloads | Current prerequisite/baseline |
|---|---|---|---|
| Windows | x64 | current-user NSIS setup or portable ZIP | Windows 10 1809+ declared; system WebView2 |
| Windows | ARM64 | ARM64 application NSIS setup or portable ZIP | native ARM64 Windows; system WebView2 |
| macOS Intel | x64 | DMG / `.app` archive | macOS 13+ declared |
| macOS Apple Silicon | ARM64 | DMG / `.app` archive | macOS 13+ declared |
| Linux | x64 | AppImage, deb, rpm or portable tar.gz | Ubuntu 24.04/glibc 2.39 build baseline; WebKitGTK 4.1/system graphics libraries |
| Linux | ARM64 | ARM64 AppImage, deb, rpm or portable tar.gz | same declared Linux baseline on ARM64 |

Check your operating system's **About/System information** screen before downloading. x64 and ARM64 are not interchangeable. No Android/iPhone application is supplied. The installed application does not need Rust, Python, Node, WSL or Docker.

## 2. Verify and install

1. Download only your target's asset, accompanying checksum/provenance/signature information and required notices. Compare the exact file size and SHA-256 with the published inventory. A checksum detects byte changes; authenticity also requires the approved signature/trust path.
2. Windows final executables/installers require the approved Authenticode signer. macOS final applications require the approved developer signature and required notarization. Linux/portable artifacts require the published trusted detached signature where no native embedded format applies. Do not install when the signer is absent, unexpected, expired or invalid. The updater public key is not an Authenticode certificate or Apple developer identity.
3. NSIS: run the verified current-user setup. It may fetch Microsoft's WebView2 bootstrapper if WebView2 is missing. Portable ZIP: extract the complete directory into a writable location; do not run directly inside the ZIP or separate the GUI, `nvsf.exe`, `runtimes`, data and notices.
4. macOS: open the verified DMG and move the application to Applications, or extract its verified `.app` archive. Linux: use your distribution's normal package installer for deb/rpm, or mark the verified AppImage executable and run it. Extract portable tar.gz completely before launching. System-library compatibility must match the build baseline.
5. Keep OS security enabled. Do not globally disable Gatekeeper, SmartScreen, signature checks or quarantine controls to make an unverified package run. Missing credentials currently block signed publication; they do not authorize bypass instructions.

Minimum-OS and clean-install behavior remain unqualified until their dedicated tests pass. A successful development-runner launch is not proof of those scenarios.

## 3. First launch and native resources

Open **Settings → Resources and diagnostics → Run resource checks**. The report distinguishes actual FFmpeg media-adapter ABI, Tesseract version and ONNX API/version checks from model/operator/hardware qualification.

Compatible bundled or selected managed resources are reused. If a native component is missing/incompatible, review the explicit repair approval and choose **Repair approved dependencies**. Repair downloads only a matching project-signed application-local runtime pack, checks its inventory/hash/target/native APIs, and activates a new selection atomically. It does not install an arbitrary `ffmpeg.exe`, change system PATH, replace another program or require a system package manager. Previous packs and saved projects are retained. Close and reopen the app after successful repair.

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

In **Models**, install the baseline detector/recognizers explicitly, or import their exact checksummed artifacts using the native picker. Bundled verified baseline resources are copied to writable application data when available; inference itself never starts a model download.

The language dropdown contains **123 official Tesseract language and vertical variants**, not just English, Persian and Japanese. `eng/fas/jpn` and existing `en/fa/ja` aliases are accepted. Select a language in **Additional language packs** and download that pack, or import its exact official checksummed `.traineddata`. The default baseline installation does not fetch the entire language catalog.

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

**Pack availability and hash verification do not measure subtitle accuracy.** Original English/Persian/Japanese accuracy/timing gates remain open; other languages are explicitly unmeasured. See [OCR contracts](ocr-models.md).

## 5. Extract and save

Choose a local video with visible burned-in subtitles and a different `.srt` output. Select its language, regions/exclusions and time range. Start with a short representative section. Automatic region suggestions are editable, not automatic acceptance of scene signs.

```bash
./nvsf extract --input "video with subtitles.mp4" --output "subtitles.srt" --language fra --quality accurate --device auto
```

Review source frames and uncertain cues before exporting. Save a review project before changing source, starting another job, repairing resources or installing an update. Checkpoints require matching source/configuration/selected-resource identities; installing unrelated language packs does not change that fingerprint. A checkpoint from an incompatible older fingerprint scheme may be refused rather than silently resumed.

## Updates, data and troubleshooting

- **Settings → Application updates** controls automatic stable-release metadata checks, a persistent disable option and manual checks. Checks run at startup and at most daily while open; video/subtitles are not uploaded. Downloads and installation have separate explicit approvals bound to the offered version. Development prereleases are never served by the stable feed.
- Automatic replacement is intended only for installed Windows, macOS `.app` and Linux AppImage. Portable archives/deb/rpm provide a verified saved download and require their normal installation procedure. Jobs, previews, resource operations, closing and unsaved edits block installation; do not terminate the app to force it through.
- Writable models/projects/cache/logs use OS local application data under `Numera Video Sub Finder`. `--data-root` / `NVSF_DATA_ROOT` selects another location. A portable package's `portable.flag` uses sibling `data`. Preserve that data during an update; do not delete it to diagnose a runtime issue.
- Run `doctor` or the resource checks for exact errors. Logs are UTF-8 under the selected data root's `logs` directory; redact private paths/context before voluntarily sharing a log. Missing hardware/quality evidence is not a runtime-install error. See [troubleshooting](troubleshooting.md), [models](models.md) and [validation](validation.md).

## Source builds

Build and test **only in GitHub Actions**. On the authorized repository, manually run **Final verification** (`verify.yml`) for the completed pushed source. It invokes the six native targets and the portable/security/source-analysis children once. No local Cargo/Trunk/Tauri compilation or local application-test fallback is part of this project's workflow. Final signed release publication remains blocked until genuine target-compatible signing inputs and all required evidence are available.
