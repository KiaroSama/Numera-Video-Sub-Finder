# Platform support

The four supported 64-bit targets are Windows x64, Windows ARM64, Linux x64 and Linux ARM64. Owner decision D-018 (2026-11-17) dropped macOS; earlier macOS evidence is historical only.

| Target | Hosted build runner | Intended formats | Evidence state |
|---|---|---|---|
| Windows x64 | windows-2025 | NSIS, portable ZIP | Hosted CPU development baseline verified at 159f9d5; new signing/icons pending |
| Windows ARM64 | windows-11-arm | NSIS, portable ZIP | Hosted CPU development baseline verified at 159f9d5; new signing/icons pending |
| Linux x64 | ubuntu-24.04 | AppImage, deb, rpm, tar.gz | Hosted CPU development baseline verified at 159f9d5; new icon checks pending |
| Linux ARM64 | ubuntu-24.04-arm | AppImage, deb, rpm, tar.gz | Hosted CPU development baseline verified at 159f9d5; new icon checks pending |

[Exact 159f9d526ee550de52a3243ee6523b238bbdc7ea verification](https://github.com/KiaroSama/Numera-Video-Sub-Finder/actions/runs/41675452445) passes all 16 jobs, the then-required native GUI+CLI development lanes and both aggregate gates, including actual runtime/extraction, GUI, relocation and package-content checks. Later source changes require their own exact-SHA evidence. These observations do not qualify platform signing, minimum OS or installer/update/uninstall lifecycle; those remain unverified or blocked, not implied by the table. No success is inferred merely from a matrix definition. Android/iPhone versions are explicitly excluded. Windows WebView2 and Linux WebKitGTK/platform libraries remain system prerequisites where not supplied by the package.

Windows development Authenticode and native icon checks are implemented, pending hosted verification, not new passing results. The self-signed `CN=Numera Video Sub Finder` RSA3072 identity has code-signing EKU and expires 2027-11-03; no timestamp or trust-store import is used. Untrusted-publisher/SmartScreen warnings remain expected. The [installation guide](installation.md#windows-development-signature-check) provides the exact public DER SHA-256 pin and read-only verification commands. Signing covers portable GUI/CLI, NSIS GUI, uninstaller and setup while preserving vendor DLL bytes. Linux deb/rpm/AppImage remain unsigned at the native level; existing updater signatures are separate and unchanged.

Logo checks compare Windows final PE icon resources with the generated ICO, Linux hicolor PNGs/desktop `Icon=`, and AppImage `.DirIcon`. Their current implementation still needs exact-source hosted execution and does not establish shell rendering or installer lifecycle.

GPU profiling (T112) and a separate Windows x64 CUDA variant (T113) are in progress, preserving the four CPU baseline products. Physical RTX 4070 Ti qualification (T114) accepts only exact CI-built artifacts and has not executed; no node placement, speedup, memory, cancellation or other-device/provider claim is made.

CPU/native/GUI/package content, installer lifecycle, minimum OS, offline behavior, signing and hardware placement are separate evidence columns in validation reports. Public hosted CPU CI cannot certify real GPU execution. Unqualified CUDA/DirectML, signing or release support is not advertised as verified. See [CI matrix](ci-matrix.md) and [validation](validation.md).

The default build matrix is 64-bit only: Windows x64 and ARM64, Linux x64 and ARM64. No 32-bit target is built.
