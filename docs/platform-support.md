# Platform support

| Target | Hosted build runner | Intended formats | Evidence state |
|---|---|---|---|
| Windows x64 | windows-2025 | NSIS, portable ZIP | Hosted CPU development baseline verified at159f9d5; new signing/icons pending |
| Windows ARM64 | windows-11-arm | NSIS, portable ZIP | Hosted CPU development baseline verified at159f9d5; new signing/icons pending |
| macOS x64 | macos-15-intel | app, DMG, tar.gz | Hosted CPU development baseline verified at159f9d5; new icon checks pending |
| macOS ARM64 | macos-15 | app, DMG, tar.gz | Hosted CPU development baseline verified at159f9d5; new icon checks pending |
| Linux x64 | ubuntu-24.04 | AppImage, deb, rpm, tar.gz | Hosted CPU development baseline verified at159f9d5; new icon checks pending |
| Linux ARM64 | ubuntu-24.04-arm | AppImage, deb, rpm, tar.gz | Hosted CPU development baseline verified at159f9d5; new icon checks pending |

[Exact159f9d526ee550de52a3243ee6523b238bbdc7ea verification](https://github.com/KiaroSama/Numera-Video-Sub-Finder/actions/runs/41675452445) passes all16 jobs, six native GUI+CLI development lanes and both aggregate gates, including actual runtime/extraction, GUI, relocation and package-content checks. Later source changes require their own exact-SHA evidence. These observations do not qualify platform signing, minimum OS or installer/update/uninstall lifecycle; those remain unverified or blocked, not implied by the table. No success is inferred merely from a matrix definition. Android/iPhone versions are explicitly excluded. macOS minimum13 is declared, not runtime-qualified. Windows WebView2 and Linux WebKitGTK/platform libraries remain system prerequisites where not supplied by the package.

Windows development Authenticode and native icon checks are **implemented, pending hosted verification**, not new passing results. The self-signed `CN=Numera Video Sub Finder` RSA3072 identity has code-signing EKU and expires2027-11-03; no timestamp or trust-store import is used. Untrusted-publisher/SmartScreen warnings remain expected. The [installation guide](installation.md#windows-development-signature-check) provides the exact public DER SHA-256 pin and read-only verification commands. Signing covers portable GUI/CLI, NSIS GUI, uninstaller and setup while preserving vendor DLL bytes. macOS signing/notarization is **blocked: no Apple credentials**. Linux deb/rpm/AppImage remain unsigned at the native level; existing updater signatures are separate and unchanged.

Logo checks compare Windows final PE icon resources with the generated ICO, macOS `.app` `icon.icns` and `CFBundleIconFile`, Linux hicolor PNGs/desktop `Icon=`, and AppImage `.DirIcon`. Their current implementation still needs exact-source hosted execution and does not establish shell rendering or installer lifecycle.

GPU profiling (T112) and a separate Windows x64 CUDA variant (T113) are in progress, preserving the six CPU baseline products. Physical RTX 4070 Ti qualification (T114) accepts only exact CI-built artifacts and has not executed; no node placement, speedup, memory, cancellation or other-device/provider claim is made.

CPU/native/GUI/package content, installer lifecycle, minimum OS, offline behavior, signing and hardware placement are separate evidence columns in validation reports. Public hosted CPU CI cannot certify real GPU execution. Unqualified CUDA/DirectML/CoreML, signing/notarization or release support is not advertised as verified. See [CI matrix](ci-matrix.md) and [validation](validation.md).
