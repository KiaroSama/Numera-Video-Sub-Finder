# Platform support

| Target | Hosted build runner | Intended formats | Evidence state |
|---|---|---|---|
| Windows x64 | windows-2025 | NSIS, portable ZIP | Hosted development checks verified at19176f8 |
| Windows ARM64 | windows-11-arm | NSIS, portable ZIP | Hosted development checks verified at19176f8 |
| macOS x64 | macos-15-intel | app, DMG, tar.gz | Hosted development checks verified at19176f8 |
| macOS ARM64 | macos-15 | app, DMG, tar.gz | Hosted development checks verified at19176f8 |
| Linux x64 | ubuntu-24.04 | AppImage, deb, rpm, tar.gz | Hosted development checks verified at19176f8 |
| Linux ARM64 | ubuntu-24.04-arm | AppImage, deb, rpm, tar.gz | Hosted development checks verified at19176f8 |

[Exact19176f8 verification](https://github.com/KiaroSama/Numera-Video-Sub-Finder/actions/runs/40866103538) passes all six native GUI+CLI development lanes and both aggregate gates, including actual runtime/extraction, GUI, relocation and package-content checks. Later source changes require their own exact-SHA evidence. These observations do not qualify platform signing, minimum OS or installer/update/uninstall lifecycle; those remain unverified or blocked, not implied by the table. No success is inferred merely from a matrix definition. Android/iPhone versions are explicitly excluded. macOS minimum13 is declared, not runtime-qualified. Windows WebView2 and Linux WebKitGTK/platform libraries remain system prerequisites where not supplied by the package.

CPU/native/GUI/package content, installer lifecycle, minimum OS, offline behavior, signing and hardware placement are separate evidence columns in validation reports. Public hosted CPU CI cannot certify real GPU execution. Unqualified CUDA/DirectML/CoreML, signing/notarization or release support is not advertised as verified. See [CI matrix](ci-matrix.md) and [validation](validation.md).
