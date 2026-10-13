# Platform support

| Target | Hosted build runner | Intended formats | Evidence state |
|---|---|---|---|
| Windows x64 | windows-2025 | NSIS, portable ZIP | NOT_RUN |
| Windows ARM64 | windows-11-arm | NSIS, portable ZIP | NOT_RUN |
| macOS x64 | macos-15-intel | app, DMG, tar.gz | NOT_RUN |
| macOS ARM64 | macos-15 | app, DMG, tar.gz | NOT_RUN |
| Linux x64 | ubuntu-24.04 | AppImage, deb, rpm, tar.gz | NOT_RUN |
| Linux ARM64 | ubuntu-24.04-arm | AppImage, deb, rpm, tar.gz | NOT_RUN |

All rows are intended native GUI+CLI targets; no success is inferred from a matrix definition. Android/iPhone versions are explicitly excluded. macOS minimum13 is declared, not runtime-qualified. Windows WebView2 and Linux WebKitGTK/platform libraries remain system prerequisites where not supplied by the package.

CPU/native/GUI/package content, installer lifecycle, minimum OS, offline behavior, signing and hardware placement are separate evidence columns in validation reports. Public hosted CPU CI cannot certify real GPU execution. Unqualified CUDA/DirectML/CoreML, signing/notarization or release support is not advertised as verified. See [CI matrix](ci-matrix.md) and [validation](validation.md).
