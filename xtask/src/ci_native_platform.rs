use super::Native;
use crate::{
    ci_guard::Result,
    ci_native_helpers::{basename, entries, path_text},
};
use std::{
    fs,
    path::{Path, PathBuf},
};

#[cfg(not(windows))]
use crate::ci_native_helpers::{elf, files};
#[cfg(windows)]
use crate::{
    ci_crt,
    ci_native_helpers::{append_runner, crt_directory, pe_machine},
    ci_notices,
};
#[cfg(windows)]
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    ffi::OsString,
};

const LIBRARIES: &[&str] = &[
    "avcodec",
    "avformat",
    "avutil",
    "swscale",
    "tesseract",
    "onnxruntime",
];

pub(super) fn shared_library(name: &str) -> bool {
    name.ends_with(".dll")
        || name.ends_with(".so")
        || name.rsplit_once(".so.").is_some_and(|(_, version)| {
            !version.is_empty() && version.bytes().all(|b| b.is_ascii_digit() || b == b'.')
        })
}

#[cfg(windows)]
fn expected_machine(arch: &str) -> u16 {
    if arch == "arm64" { 0xaa64 } else { 0x8664 }
}

#[cfg(windows)]
pub(super) fn media_machine(path: &Path, arch: &str) -> Result<()> {
    if pe_machine(path)? != expected_machine(arch) {
        return Err("Native PE architecture mismatch.".into());
    }
    Ok(())
}

#[cfg(not(windows))]
pub(super) fn media_machine(path: &Path, arch: &str) -> Result<()> {
    elf(path, arch, true)
}

#[cfg(windows)]
fn registry_opt_out(native: &mut Native, key: &str, view: &str) -> Result<()> {
    native.run.run(
        "vs-telemetry-opt-out",
        "reg.exe",
        [
            "add",
            key,
            "/v",
            "OptIn",
            "/t",
            "REG_DWORD",
            "/d",
            "0",
            "/f",
            view,
        ],
        (30, 10),
    )?;
    let result = native.run.capture(
        "vs-telemetry-readback",
        "reg.exe",
        ["query", key, "/v", "OptIn", view],
        (30, 10),
    )?;
    if !result.lines().any(|line| {
        line.split_ascii_whitespace()
            .eq(["OptIn", "REG_DWORD", "0x0"])
    }) {
        return Err("Hosted Visual Studio telemetry opt-out readback failed.".into());
    }
    Ok(())
}

#[cfg(windows)]
pub(super) fn bootstrap(native: &mut Native) -> Result<()> {
    use base64::{Engine, engine::general_purpose::STANDARD};

    registry_opt_out(
        native,
        r"HKLM\SOFTWARE\Policies\Microsoft\VisualStudio\SQM",
        "/reg:64",
    )?;
    native.run.log("INFO", "Hosted Visual Studio optional telemetry policy verified OptIn=0; owned-tree checks remain mandatory.")?;
    let vswhere = PathBuf::from(
        native
            .run
            .get("ProgramFiles(x86)")
            .ok_or("ProgramFiles(x86) is unavailable.")?,
    )
    .join("Microsoft Visual Studio/Installer/vswhere.exe");
    let arm = native.target.arch == "arm64";
    let component = if arm {
        "Microsoft.VisualStudio.Component.VC.Tools.ARM64"
    } else {
        "Microsoft.VisualStudio.Component.VC.Tools.x86.x64"
    };
    let discovery = native.run.capture(
        "vs-discovery",
        vswhere,
        [
            "-latest",
            "-products",
            "*",
            "-requires",
            component,
            "-format",
            "json",
            "-utf8",
        ],
        (300, 120),
    )?;
    let installs: Vec<serde_json::Value> =
        serde_json::from_str(discovery.trim_start_matches('\u{feff}'))?;
    let [install] = installs.as_slice() else {
        return Err("Visual Studio discovery did not return one native installation.".into());
    };
    let vs = PathBuf::from(
        install["installationPath"]
            .as_str()
            .ok_or("Visual Studio installation path missing.")?,
    );
    let version = install["installationVersion"]
        .as_str()
        .ok_or("Visual Studio installation version missing.")?;
    let major = version
        .split('.')
        .next()
        .filter(|major| matches!(*major, "17" | "18"))
        .ok_or("Unsupported Visual Studio installation version.")?;
    if !vs.is_dir() || !version.split('.').all(|part| part.parse::<u32>().is_ok()) {
        return Err("Visual Studio installation/version is invalid.".into());
    }
    let sqm = format!(r"HKLM\SOFTWARE\Microsoft\VSCommon\{major}.0\SQM");
    for view in ["/reg:64", "/reg:32"] {
        registry_opt_out(native, &sqm, view)?;
    }
    native.run.log("INFO", &format!("Hosted Visual Studio version={version} optional telemetry opt-outs verified views=64,32."))?;
    let shell = vs.join("Common7/Tools/Launch-VsDevShell.ps1");
    if !shell.is_file() {
        return Err("Installed Visual Studio developer environment entry point is missing.".into());
    }
    let arch = if arm { "arm64" } else { "amd64" };
    let shell = STANDARD.encode(path_text(&shell)?);
    // The vendor entry point is PowerShell. Capture only compiler variables, never the full runner environment.
    let code = format!(
        r#"
$ErrorActionPreference = 'Stop'
$path = [Text.Encoding]::UTF8.GetString([Convert]::FromBase64String('{shell}'))
& $path -Arch {arch} -HostArch {arch} -SkipAutomaticLocation 6>$null | Out-Null
$record = @{{}}
foreach ($name in @('PATH','INCLUDE','LIB','LIBPATH','VCINSTALLDIR','VCToolsInstallDir','VCToolsVersion','VSINSTALLDIR','WindowsSdkDir','WindowsSDKVersion','WindowsSdkBinPath','WindowsSdkVerBinPath','UniversalCRTSdkDir','UCRTVersion','VSCMD_ARG_HOST_ARCH','VSCMD_ARG_TGT_ARCH')) {{
    $value = [Environment]::GetEnvironmentVariable($name, 'Process')
    if ($value) {{ $record[$name] = $value }}
}}
[Console]::OutputEncoding = [Text.UTF8Encoding]::new($false)
[Console]::WriteLine((ConvertTo-Json $record -Compress))
"#
    );
    let encoded = STANDARD.encode(
        code.encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect::<Vec<_>>(),
    );
    let developer = native.run.capture(
        "vs-environment",
        "pwsh.exe",
        [
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-EncodedCommand",
            &encoded,
        ],
        (60, 30),
    )?;
    let developer: BTreeMap<String, String> = serde_json::from_str(&developer)?;
    for name in [
        "PATH",
        "INCLUDE",
        "LIB",
        "VCINSTALLDIR",
        "WindowsSdkDir",
        "WindowsSDKVersion",
        "VSCMD_ARG_HOST_ARCH",
        "VSCMD_ARG_TGT_ARCH",
    ] {
        if developer.get(name).is_none_or(String::is_empty) {
            return Err(
                format!("Developer environment omitted required compiler field: {name}.").into(),
            );
        }
    }
    for var in ["VSCMD_ARG_HOST_ARCH", "VSCMD_ARG_TGT_ARCH"] {
        let value = developer.get(var).ok_or("Missing compiler architecture.")?;
        if (if value == "amd64" { "x64" } else { value }) != native.target.arch {
            return Err(
                "Developer environment host/target architecture mismatches the native runner."
                    .into(),
            );
        }
    }
    for header in ["errno.h", "windows.h"] {
        if !std::env::split_paths(
            developer
                .get("INCLUDE")
                .ok_or("Missing compiler include paths.")?,
        )
        .any(|path| path.join(header).is_file())
        {
            return Err(format!("Developer environment cannot resolve {header}.").into());
        }
    }
    for (key, value) in developer {
        native.run.export(&key, value)?;
    }
    let compiler = native
        .run
        .find("cl.exe")
        .ok_or("Native Visual Studio compiler missing from PATH.")?;
    media_machine(&compiler, &native.target.arch)?;
    native.run.log("INFO", &format!("Developer environment verified host={arch} target={arch} sdk_headers=2 compiler_native=true"))?;
    let candidates = native
        .run
        .get("LIBCLANG_PATH")
        .map(PathBuf::from)
        .into_iter()
        .chain(
            native
                .run
                .get("ProgramFiles")
                .map(|base| PathBuf::from(base).join("LLVM/bin")),
        )
        .chain([
            vs.join("VC/Tools/LLVM/ARM64/bin"),
            vs.join("VC/Tools/LLVM/x64/bin"),
            vs.join("VC/Tools/LLVM/bin"),
        ])
        .chain(
            native
                .run
                .find("clang.exe")
                .and_then(|path| path.parent().map(Path::to_path_buf)),
        )
        .collect::<Vec<_>>();
    let matches = |path: &Path| {
        pe_machine(path).is_ok_and(|machine| machine == expected_machine(&native.target.arch))
    };
    let libclang = candidates
        .iter()
        .find(|dir| matches(&dir.join("libclang.dll")))
        .ok_or("No host-native libclang.dll found.")?;
    let llvm = candidates
        .iter()
        .find(|dir| {
            ["clang-cl.exe", "llvm-lib.exe", "lld-link.exe"]
                .iter()
                .all(|name| matches(&dir.join(name)))
        })
        .ok_or("Native LLVM compiler, librarian and linker are required.")?;
    native.run.export("LIBCLANG_PATH", libclang)?;
    let triple = native.target.triple.replace('-', "_");
    for (key, tool) in [
        (format!("CC_{triple}"), "clang-cl.exe"),
        (format!("CXX_{triple}"), "clang-cl.exe"),
        (format!("AR_{triple}"), "llvm-lib.exe"),
        (
            format!("CARGO_TARGET_{}_LINKER", triple.to_ascii_uppercase()),
            "lld-link.exe",
        ),
    ] {
        native.run.export(&key, llvm.join(tool))?;
    }
    for kind in ["CFLAGS", "CXXFLAGS"] {
        let key = format!("{kind}_{triple}");
        let prior = native.run.get(&key).unwrap_or_default();
        native.run.export(
            &key,
            format!(
                "{} --start-no-unused-arguments -fuse-ld=lld --end-no-unused-arguments",
                prior.to_string_lossy()
            )
            .trim(),
        )?;
    }
    native.run.export("CC_ENABLE_DEBUG_OUTPUT", "1")?;
    native.run.log(
        "INFO",
        &format!(
            "Windows Rust toolchain selected LLVM target={} native_tools=3 driver_linker=lld.",
            native.target.triple
        ),
    )?;
    native.run.export("VCPKG_VISUAL_STUDIO_PATH", vs)?;
    native.run.run(
        "vcpkg-bootstrap",
        native.vcpkg.join("bootstrap-vcpkg.bat"),
        ["-disableMetrics"],
        (600, 180),
    )
}

#[cfg(not(windows))]
pub(super) fn bootstrap(native: &mut Native) -> Result<()> {
    native.run.run(
        "vcpkg-bootstrap",
        "bash",
        [
            path_text(&native.vcpkg.join("bootstrap-vcpkg.sh"))?,
            "-disableMetrics",
        ],
        (600, 180),
    )
}

#[cfg(windows)]
pub(super) fn export(native: &mut Native) -> Result<()> {
    native.run.export(
        "PKG_CONFIG_PATH",
        std::env::join_paths([
            native.prefix.join("lib/pkgconfig"),
            native.prefix.join("share/pkgconfig"),
        ])?,
    )?;
    for (key, child) in [("LIB", "lib"), ("INCLUDE", "include")] {
        let mut val = OsString::from(native.prefix.join(child));
        val.push(";");
        val.push(
            native
                .run
                .get(key)
                .ok_or("Missing developer environment.")?,
        );
        native.run.export(key, val)?;
    }
    let bin = native.prefix.join("bin");
    append_runner("GITHUB_PATH", path_text(&bin)?)?;
    append_runner("GITHUB_PATH", path_text(&native.runtimes)?)?;
    let path = native.run.get("PATH").ok_or("Missing developer PATH.")?;
    native.run.set(
        "PATH",
        std::env::join_paths(
            [native.runtimes.clone(), bin]
                .into_iter()
                .chain(std::env::split_paths(&path)),
        )?,
    );
    Ok(())
}

#[cfg(not(windows))]
pub(super) fn export(native: &mut Native) -> Result<()> {
    let pkg = native
        .run
        .find("pkg-config")
        .or_else(|| {
            files(&native.vcpkg.join("downloads/tools"))
                .ok()?
                .into_iter()
                .find(|path| {
                    basename(path).is_ok_and(|name| matches!(name, "pkgconf" | "pkg-config"))
                })
        })
        .ok_or("Native pkg-config/pkgconf prerequisite is unavailable after vcpkg bootstrap.")?;
    native.run.export("PKG_CONFIG", &pkg)?;
    native.run.run(
        "system-gui-libraries",
        pkg,
        [
            "--libs",
            "--cflags",
            "gdk-3.0",
            "gtk+-3.0",
            "webkit2gtk-4.1",
        ],
        (30, 15),
    )?;
    native.run.log(
        "INFO",
        "System GTK/WebKit dependency closure verified; owned media paths remain explicit.",
    )?;
    native.run.export(
        "LD_LIBRARY_PATH",
        std::env::join_paths([native.runtimes.clone(), native.prefix.join("lib")])?,
    )?;
    let key = format!(
        "CARGO_TARGET_{}_RUSTFLAGS",
        native.target.triple.replace('-', "_").to_ascii_uppercase()
    );
    let prior = native.run.get(&key).unwrap_or_default();
    native.run.export(
        &key,
        format!(
            "{} -C link-arg=-Wl,-rpath,$ORIGIN/runtimes",
            prior.to_string_lossy()
        )
        .trim(),
    )
}

pub(super) fn prepare_runtime(native: &mut Native) -> Result<()> {
    let required: Vec<String> = if cfg!(windows) {
        [
            "avcodec-63.dll",
            "avformat-63.dll",
            "avutil-61.dll",
            "swscale-10.dll",
            "tesseract55.dll",
            "onnxruntime.dll",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect()
    } else {
        LIBRARIES
            .iter()
            .map(|base| format!("lib{base}.so"))
            .collect()
    };
    let staged = entries(&native.runtimes)?;
    #[cfg(windows)]
    for file in &staged {
        if basename(file)?.ends_with(".dll") {
            media_machine(file, &native.target.arch)?;
        }
    }
    #[cfg(not(windows))]
    for name in &required {
        let canonical = native.runtimes.join(name);
        if !canonical.exists() {
            let prefix = format!("{name}.");
            let file = staged
                .iter()
                .find(|file| {
                    basename(file)
                        .is_ok_and(|file| file.starts_with(&prefix) && shared_library(file))
                })
                .ok_or_else(|| format!("Native library missing: {name}."))?;
            fs::copy(file, canonical)?;
        }
    }
    let seeds = staged
        .into_iter()
        .filter(|file| basename(file).is_ok_and(|name| shared_library(name)))
        .chain(required.iter().map(|name| native.runtimes.join(name)))
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    for name in required {
        if !native.runtimes.join(&name).is_file() {
            return Err(format!("Expected native runtime absent: {name}.").into());
        }
    }
    closure(native, &seeds)
}

#[cfg(windows)]
pub(super) fn closure(native: &mut Native, seeds: &[PathBuf]) -> Result<()> {
    let vs = PathBuf::from(
        native
            .run
            .get("VCPKG_VISUAL_STUDIO_PATH")
            .ok_or("Native DLL closure requires the Visual Studio redistributable root.")?,
    );
    let (version, crt) = crt_directory(&vs, &native.target.arch)?;
    let mut queue: VecDeque<_> = seeds.iter().cloned().collect();
    let mut visited = BTreeSet::new();
    while let Some(file) = queue.pop_front() {
        if !visited.insert(basename(&file)?.to_ascii_lowercase()) {
            continue;
        }
        if visited.len() > 512 {
            return Err("Native PE dependency closure exceeded its file bound.".into());
        }
        media_machine(&file, &native.target.arch)?;
        let output = native.run.capture(
            "pe-dependencies",
            "dumpbin.exe",
            ["/NOLOGO", "/DEPENDENTS", path_text(&file)?],
            (30, 10),
        )?;
        let crt_imports = ci_crt::parse_release_crt_imports(&output)?;
        let mut imports = output
            .lines()
            .filter(|line| line.starts_with(char::is_whitespace))
            .map(str::trim)
            .filter(|name| {
                let name = name.to_ascii_lowercase();
                (name.ends_with(".dll") || name.ends_with(".drv"))
                    && name
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"_.+-".contains(&b))
            })
            .peekable();
        if imports.peek().is_none() {
            return Err("Native PE import inventory is empty or unrecognized.".into());
        }
        for dep in imports {
            let name = dep.to_ascii_lowercase();
            let dest = native.runtimes.join(dep);
            if name == "xmllite.dll" {
                if dest.exists() {
                    return Err("Windows-serviced XmlLite must not be redistributed.".into());
                }
                let system = PathBuf::from(
                    native
                        .run
                        .get("SystemRoot")
                        .ok_or("Windows system directory is unavailable.")?,
                )
                .join("System32/xmllite.dll");
                let machine = pe_machine(&system)?;
                if machine != expected_machine(&native.target.arch)
                    && !(native.target.arch == "arm64" && machine == 0xa64e)
                {
                    return Err("Windows-serviced XmlLite architecture is incompatible.".into());
                }
            } else if dest.is_file() {
                queue.push_back(dest);
            } else if !system_dll(&name) {
                let source = crt.join(dep);
                if !crt_imports.contains(&name) || !source.is_file() {
                    return Err(format!(
                        "Unresolved non-system PE dependency: {dep} ({}).",
                        basename(&file)?
                    )
                    .into());
                }
                ci_crt::assert_release_crt_machine(&source, &native.target.arch)?;
                fs::copy(source, &dest)?;
                queue.push_back(dest);
            }
        }
    }
    ci_notices::copy_visual_studio_notices(
        &vs.join("Licenses"),
        &native.notices.join("microsoft/visual-studio"),
    )?;
    fs::write(
        native.notices.join("microsoft/visual-cpp-runtime.txt"),
        format!(
            "Imported release CRT members staged from Visual Studio Redist/MSVC/{version}/{}. Redistribution remains subject to Microsoft Software License Terms and licensed-user rights, not qualified by hosted execution.\nhttps://learn.microsoft.com/en-us/cpp/windows/redistributing-visual-cpp-files\n",
            native.target.arch
        ),
    )?;
    Ok(())
}

#[cfg(windows)]
fn system_dll(name: &str) -> bool {
    const SYSTEM: &str = "kernel32 kernelbase ntdll advapi32 bcrypt bcryptprimitives crypt32 ncrypt secur32 wintrust cryptsp cryptbase ws2_32 wsock32 mswsock dnsapi netapi32 wldap32 sechost sspicli user32 userenv gdi32 gdi32full ole32 oleaut32 combase shell32 shlwapi shcore uxtheme dwmapi msimg32 winmm avicap32 msvfw32 mf mfplat mfreadwrite mfuuid mmdevapi avrt d3d9 d3d10 d3d11 d3d12 dxgi dxva2 dxcore d2d1 dwrite opengl32 glu32 windowscodecs version setupapi cfgmgr32 comdlg32 comctl32 imm32 rpcrt4 psapi dbghelp imagehlp iphlpapi normaliz powrprof winhttp wininet wtsapi32 msvcrt ucrtbase";
    name.starts_with("api-ms-")
        || name.starts_with("ext-ms-")
        || name == "winspool.drv"
        || name
            .strip_suffix(".dll")
            .is_some_and(|stem| SYSTEM.split_ascii_whitespace().any(|item| item == stem))
}

#[cfg(not(windows))]
pub(super) fn closure(native: &mut Native, seeds: &[PathBuf]) -> Result<()> {
    for file in seeds {
        let file = path_text(file)?;
        native.run.run(
            "elf-relocate",
            "patchelf",
            ["--set-rpath", "$ORIGIN", file],
            (300, 120),
        )?;
        if native.run.capture(
            "elf-rpath-verify",
            "patchelf",
            ["--print-rpath", file],
            (300, 120),
        )? != "$ORIGIN"
        {
            return Err("Relocated ELF runtime path verification failed.".into());
        }
        for dep in native
            .run
            .capture(
                "elf-dependencies",
                "patchelf",
                ["--print-needed", file],
                (300, 120),
            )?
            .lines()
            .filter(|name| !name.is_empty())
        {
            if dep.contains(['/', '\\']) {
                return Err(format!("Unrelocated ELF dependency path: {dep}.").into());
            }
            if !native.runtimes.join(dep).is_file() && !system_so(dep) {
                return Err(
                    format!("Unresolved non-system ELF dependency: {dep} ({file}).").into(),
                );
            }
        }
    }
    Ok(())
}

#[cfg(not(windows))]
fn system_so(name: &str) -> bool {
    name.split_once(".so").is_some_and(|(base, version)| {
        (matches!(
            base,
            "libc"
                | "libm"
                | "libdl"
                | "libpthread"
                | "librt"
                | "libresolv"
                | "libutil"
                | "libgcc_s"
                | "libstdc++"
        ) || base.starts_with("ld-linux"))
            && (version.is_empty()
                || version.strip_prefix('.').is_some_and(|suffix| {
                    suffix
                        .split('.')
                        .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
                }))
    })
}
