# Platform packages

The four required 64-bit targets are Windows x64, Windows ARM64, Linux x64 and Linux ARM64. `ci/targets.json` is the build/package inventory, not a statement that those builds have passed. Declared deployment baselines require actual artifact verification: Windows 10 1809, Ubuntu 24.04/glibc 2.39. Android, iPhone and 32-bit builds are excluded.

Windows GUI uses WebView2; Linux uses GTK/WebKitGTK 4.1 and platform webview dependencies. Packages must include the matching CLI worker, native runtime closure, model/provenance/license resources, and the official icon. Application data stays in a writable external location and is preserved across updates/uninstall.

Portable archives are distinct from MSI and AppImage/deb/rpm installers. A copied executable is not an installed-artifact test. Release qualification requires target-native ABI, relocation/load path/resource, clean-machine launch, installer/update/uninstall and offline extraction evidence for the actual advertised formats.

Internal compiler/packaging intermediates may be unsigned and are not completed distributable builds. Final publication requires approved native signatures; Windows signing inputs are currently unavailable. The dedicated updater key does not establish platform trust. Do not disable signature checks or platform security globally. Corresponding-source/license and installation-lifecycle evidence remain separate gates.

Hosted CPU verification cannot prove GPU driver/runtime/operator placement, device performance or signing trust. Physical-device qualification needs separately authorized hardware runners; none are currently authorized.
