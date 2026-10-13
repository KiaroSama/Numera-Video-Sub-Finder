# Windows build and distribution

Builds run only in GitHub Actions: Windows 2025 x64 and Windows 11 ARM64 native hosted runners. Rust 1.99, pinned Tauri CLI, Clang/bindgen and Visual Studio SDK are prerequisites. Native FFmpeg/Tesseract are built from the pinned vcpkg baseline with dynamic release triplets; ONNX Runtime is the exact API31 CPU archive. No local build commands are part of the authorized workflow.

CI generates official ICO/PNG assets, builds GUI/CLI and assembles portable ZIP plus current-user NSIS. DLLs must be next to imported executables before main; the runtimes directory also supplies explicitly loaded libraries. Target architecture and closure are checked in provisioning/package inventories. Windows loader behavior on a hosted development runner does not prove minimum-OS clean-install behavior.

NSIS uses WebView2 download bootstrapper when a usable system WebView2 is missing; that installation step needs network. Ordinary extraction after resource provisioning remains local. Visual C++ redistribution rights and notices must be checked before public release. Packages are unsigned development artifacts until separately authorized signing evidence exists. Installer/update/uninstall/data-retention and physical DirectML/CUDA execution remain unqualified; see validation status.
