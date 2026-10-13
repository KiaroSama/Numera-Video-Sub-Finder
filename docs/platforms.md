# Platform packages

Required targets are Windows x64/ARM64, macOS x64/ARM64 and Linux x64/ARM64. `ci/targets.json` is the build/package inventory, not a statement that those builds have passed. Declared deployment baselines require actual artifact verification: Windows 10 1809, macOS 13, Ubuntu 24.04/glibc 2.39. Android and iPhone are excluded.

Windows GUI uses WebView2; Linux uses GTK/WebKitGTK 4.1 and platform webview dependencies; macOS uses system WKWebView. Packages must include the matching CLI worker, native runtime closure, model/provenance/license resources, and the official icon. Application data stays in a writable external location and is preserved across updates/uninstall.

Portable archives are distinct from NSIS, AppImage/deb/rpm and macOS app/dmg installers. A copied executable is not an installed-artifact test. Release qualification requires target-native ABI, relocation/load path/resource, clean-machine launch, installer/update/uninstall and offline extraction evidence for the actual advertised formats.

Development artifacts are unsigned unless their manifests say otherwise. Do not disable Gatekeeper, signature checks or platform security globally. Signing/notarization and corresponding-source/license evidence must be supplied before any signed/release-ready claim.

Hosted CPU verification cannot prove GPU driver/runtime/operator placement, device performance or signing trust. Physical-device qualification needs separately authorized hardware runners; none are currently authorized.
