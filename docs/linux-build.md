# Linux build and distribution

Build only in GitHub Actions using native Ubuntu 24.04 x64 and ARM64 runners. Provisioning uses pinned dynamic-release vcpkg triplets, native FFmpeg/Tesseract and the exact ONNX Runtime CPU archive. WebKitGTK 4.1 and GTK/platform dependencies are prerequisites; Xvfb/Openbox provide an isolated test display. No WSL, Docker, Rust, Python or Node runtime is required by the installed application itself.

Portable executables search $ORIGIN/runtimes. Installed /usr/bin executables use the package's /usr/lib/Numera Video Sub Finder/runtimes directory. Libraries have $ORIGIN internal search paths; verified models/resources are separate from writable user state. AppImage, deb, rpm and portable tar.gz are generated from target-correct production GUI/CLI bytes with native notices and official icons.

An Ubuntu-hosted build is not proof of every Linux distribution or minimum glibc compatibility. WebKitGTK/system graphics dependencies, clean artifact-only launch, ABI inventory and installation/update/uninstall/data-retention need actual evidence. CPU packages do not imply a CUDA runtime or physical GPU qualification. See platform support and CI reports.
