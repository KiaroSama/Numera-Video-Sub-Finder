#[path = "ci_bundle_helpers.rs"]
mod helpers;
#[cfg(windows)]
#[path = "ci_bundle_windows.rs"]
mod windows;

use crate::{
    ci_guard::{self, Result, unlinked},
    ci_native_helpers::{
        Member, Session, Target, admission, basename, elf, entries, files, header, json, path_text,
        safe_relative, sha,
    },
};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsStr,
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    process::Command,
};

const GUI: &str = "numera-video-sub-finder";
const STATUS: &str = "development-unqualified";
const MEMBER_LIMIT: u64 = 8 * 1024 * 1024;

struct Bundle {
    root: PathBuf,
    target: Target,
    source: String,
    desktop: PathBuf,
    packages: PathBuf,
    base_name: String,
    portable: PathBuf,
    stage: tempfile::TempDir,
    payload: PathBuf,
    release: PathBuf,
    output: PathBuf,
    main: PathBuf,
    cli: PathBuf,
    product: String,
    run: Session,
    resources: BTreeMap<String, String>,
    icons: Vec<String>,
    signatures: Vec<Value>,
}

pub(super) fn execute(id: &str, packages: &Path) -> Result<()> {
    let (root, target, source) = admission(
        id,
        "Desktop bundling is restricted to GitHub-hosted Actions runners.",
    )?;
    let desktop = root.join("crates/desktop");
    let config = json(&desktop.join("tauri.conf.json"))?;
    let version = config["version"]
        .as_str()
        .ok_or("Missing desktop version.")?;
    if config["mainBinaryName"] != GUI || !helpers::desktop_version(version) {
        return Err("Unsupported version or desktop executable contract.".into());
    }
    let product = config["productName"]
        .as_str()
        .filter(|name| safe_relative(name) && !name.contains('/'))
        .ok_or("Invalid desktop product name.")?
        .to_owned();
    let packages = helpers::owned(&root, &root.join(packages))?;
    let base_name = format!("Numera-Video-Sub-Finder-{version}-{id}");
    let portable = helpers::owned(&root, &packages.join(&base_name))?;
    let output = helpers::owned(&root, &packages.join("installers").join(id))?;
    let staging = helpers::owned(&root, &root.join(".ci-work/bundle").join(id))?;
    fs::create_dir_all(&staging)?;
    let stage = tempfile::Builder::new()
        .prefix("stage-")
        .tempdir_in(&staging)?;
    let payload = stage.path().join("payload");
    let cargo = stage.path().join("cargo");
    let release = cargo.join("release");
    let run = Session::new(&root, "bundle", "DESKTOP-BUNDLE")?;
    fs::create_dir_all(&payload)?;
    fs::create_dir_all(&release)?;
    fs::create_dir_all(output.parent().ok_or("Missing installer output parent.")?)?;
    fs::create_dir(&output).map_err(|error| {
        format!("Bundle output must be a new target-specific directory: {error}")
    })?;
    let main = release.join(format!("{GUI}{}", std::env::consts::EXE_SUFFIX));
    let cli = stage.path().join(format!(
        "nvsf-{}{}",
        target.triple,
        std::env::consts::EXE_SUFFIX
    ));
    let mut bundle = Bundle {
        root,
        target,
        source,
        desktop,
        packages,
        base_name,
        portable,
        stage,
        payload,
        release,
        output,
        main,
        cli,
        product,
        run,
        resources: BTreeMap::new(),
        icons: Vec::new(),
        signatures: Vec::new(),
    };
    bundle.run.set("CARGO_TARGET_DIR", cargo);
    for key in ["CARGO_BUILD_JOBS", "RAYON_NUM_THREADS", "OMP_NUM_THREADS"] {
        bundle.run.set(key, "2");
    }
    let result = bundle.execute(&config);
    if let Err(error) = &result {
        bundle
            .run
            .log("ERROR", &format!("Bundling failed: {error}"))?;
    }
    result
}

impl Bundle {
    fn command(
        &mut self,
        name: &str,
        exe: impl AsRef<OsStr>,
        args: impl IntoIterator<Item = impl AsRef<OsStr>>,
        limits: (u32, u32),
        capture: bool,
        cwd: Option<&Path>,
    ) -> Result<String> {
        let mut command = Command::new(exe);
        command
            .args(args)
            .current_dir(cwd.unwrap_or(&self.desktop))
            .env_remove("TAURI_SIGNING_RPM_KEY")
            .env_remove("TAURI_SIGNING_RPM_KEY_PASSPHRASE");
        self.run.command(name, command, limits, capture, None)
    }

    fn execute(&mut self, config: &Value) -> Result<()> {
        #[cfg(windows)]
        let identity = windows::Identity::enable()?;
        let signing = if cfg!(windows) {
            "development-self-signed"
        } else {
            "unsigned"
        };
        self.run.log(
            "INFO",
            &format!(
                "Start target={} source_sha={} status={STATUS} signing={signing}",
                self.target.id, self.source
            ),
        )?;
        let members = self.read_portable()?;
        self.stage_payload(config, &members)?;
        #[cfg(windows)]
        self.prepare_windows()?;
        let cpus = if cfg!(windows) {
            String::new()
        } else {
            self.prepare_linux()?
        };
        let provenance = self.payload.join("bundle-provenance.json");
        helpers::write_json(
            &provenance,
            &json!({"schema":1, "source_sha":self.source, "target":self.target.id, "status":STATUS, "signing":"unsigned", "input_inventory_sha256":sha(&self.portable.join("manifest.json"))?, "files":helpers::inventory(&self.payload, &self.payload)?}),
        )?;
        self.resource(&provenance, "bundle-provenance.json")?;
        let payload = helpers::inventory(&self.payload, &self.payload)?;
        let sidecar = self.stage.path().join("nvsf");
        let tauri = json!({"bundle":{"resources":self.resources,"externalBin":[path_text(&sidecar)?.replace('\\', "/")]}});
        #[cfg(windows)]
        let tauri = {
            let mut tauri = tauri;
            tauri["bundle"]["windows"] = self.signing_config()?;
            tauri
        };
        let config_path = self.stage.path().join("bundle.conf.json");
        helpers::write_json(&config_path, &tauri)?;
        let formats = self.target.package_formats.clone();
        let native_formats = formats
            .iter()
            .map(String::as_str)
            .filter(|format| !matches!(*format, "zip" | "tar.gz"))
            .collect::<Vec<_>>();
        let format_arg = native_formats.join(",");
        let mut args = vec!["tauri", "bundle", "--ci"];
        if !cfg!(windows) {
            args.push("--no-sign");
        }
        args.extend([
            "--config",
            path_text(&config_path)?,
            "--bundles",
            &format_arg,
            "-v",
        ]);
        if cfg!(windows) {
            self.command("native-formats", "cargo", args, (1500, 600), false, None)?;
        } else {
            self.command(
                "native-formats",
                "taskset",
                ["--cpu-list", &cpus, "cargo"].into_iter().chain(args),
                (1500, 600),
                false,
                None,
            )?;
        }
        let mut artifacts = Vec::new();
        let mut proof = vec![
            "same-source input byte inventory".to_owned(),
            "real native bundler exit and all required format outputs".to_owned(),
            "portable ZIP/tar decompressed byte integrity".to_owned(),
            "verified SHA256SUMS.txt manifest".to_owned(),
        ];
        for format in native_formats {
            let file = self.inspect_format(format, &payload)?;
            artifacts.push(self.artifact(format, &file)?);
            proof.push(format!("{format}: actual package header/decompressed content checks; installed execution not claimed"));
        }
        #[cfg(windows)]
        let archive = self.zip(&members, signing)?;
        #[cfg(not(windows))]
        let archive = self.tar()?;
        artifacts.push(self.artifact(if cfg!(windows) { "zip" } else { "tar.gz" }, &archive)?);
        let actual = artifacts
            .iter()
            .filter_map(|artifact| artifact["format"].as_str())
            .collect::<BTreeSet<_>>();
        if formats.len() != artifacts.len()
            || actual != formats.iter().map(String::as_str).collect::<BTreeSet<_>>()
        {
            return Err("Required format inventory is incomplete.".into());
        }
        let checks = 2 + artifacts.len();
        self.checksums()?;
        #[cfg(windows)]
        identity.close()?;
        let report = json!({"schema":1,"source_sha":self.source,"target":self.target.id,"passed":true,"tests":checks,"failed":0,"skipped":0,"status":STATUS,"signing":signing,"signatures":self.signatures,"icon_checks":self.icons,"formats":artifacts,"files":helpers::inventory(&self.output, &self.packages)?,"proof":proof,
            "qualification":{"installer_lifecycle":"NOT_RUN","installed_launch":"NOT_RUN","physical_gpu":"NOT_RUN","production_signing":if cfg!(windows) { "development-self-signed-untrusted-root-untimestamped" } else { "NOT_RUN" }, "linuxdeploy_modified_elf_byte_parity":"NOT_CLAIMED","microsoft_redistribution_rights":if cfg!(windows) { "NOT_VALIDATED" } else { "NOT_APPLICABLE" },"windows_webview":if cfg!(windows) { "system WebView2 required for portable GUI; NSIS bootstrap may require internet" } else { "NOT_APPLICABLE" },"compression_thread_count":"NOT_MEASURED","linux_bundler_cpu_capacity":if cfg!(windows) { "NOT_APPLICABLE" } else { &cpus }}});
        let reports = helpers::owned(&self.root, &self.packages.join("reports"))?;
        fs::create_dir_all(&reports)?;
        helpers::write_json(
            &reports.join(format!("{}-bundles.json", self.target.id)),
            &report,
        )?;
        self.run.log("INFO", &format!("Complete target={} formats={} checks={checks} output={} signing={signing} icon_checks={} qualification=NOT_RUN", self.target.id, actual.into_iter().collect::<Vec<_>>().join(","), self.output.display(), self.icons.len()))
    }

    fn read_portable(&self) -> Result<Vec<Member>> {
        let manifest = self.portable.join("manifest.json");
        if unlinked(&manifest)?.len() > MEMBER_LIMIT {
            return Err("Portable package inventory exceeds its 8 MiB bound.".into());
        }
        let value = json(&manifest)?;
        let members: Vec<Member> = serde_json::from_value(value["files"].clone())?;
        if value["schema"] != 1
            || value["target"] != self.target.id
            || value["source_sha"] != self.source
            || value["status"] != STATUS
            || members.len() < 2
        {
            return Err(
                "Bundle input requires the verified same-source portable package inventory.".into(),
            );
        }
        let mut known = BTreeSet::new();
        for entry in &members {
            if !safe_relative(&entry.file) || !known.insert(entry.file.to_ascii_lowercase()) {
                return Err("Unsafe or duplicate portable package inventory path.".into());
            }
            entry.verify(&helpers::owned(
                &self.root,
                &self.portable.join(&entry.file),
            )?)?;
        }
        for path in files(&self.portable)? {
            let name = path_text(path.strip_prefix(&self.portable)?)?.replace('\\', "/");
            if name != "manifest.json" && !known.contains(&name.to_ascii_lowercase()) {
                return Err("Portable payload contains an uninventoried file.".into());
            }
        }
        for name in [
            self.gui_name(),
            self.cli_name(),
            "LICENSE",
            "THIRD_PARTY_NOTICES.md",
            "README.md",
            "portable.flag",
            "assets/logo.svg",
        ] {
            if !known.contains(&name.to_ascii_lowercase()) {
                return Err(format!("Missing required portable package member: {name}").into());
            }
        }
        if !known
            .iter()
            .any(|name| name.starts_with("licenses/native/"))
            || !known.iter().any(|name| name.starts_with("data/models/"))
        {
            return Err(
                "Native notices and actual model payload are required for desktop bundles.".into(),
            );
        }
        Ok(members)
    }

    fn cli_name(&self) -> &str {
        if cfg!(windows) { "nvsf.exe" } else { "nvsf" }
    }
    fn gui_name(&self) -> &str {
        if cfg!(windows) {
            "Numera Video Sub Finder.exe"
        } else {
            GUI
        }
    }

    fn stage_payload(&mut self, config: &Value, members: &[Member]) -> Result<()> {
        let gui = self.portable.join(self.gui_name());
        let cli = self.portable.join(self.cli_name());
        let logo = self.portable.join("assets/logo.svg");
        let approved = fs::read(self.root.join("assets/logo.svg"))?;
        if fs::read(&logo)? != approved {
            return Err("Portable package logo differs from the approved source.".into());
        }
        for (src, dst) in [(&gui, &self.main), (&cli, &self.cli)] {
            crate::package::verify_embedded_logo(src, &approved)?;
            helpers::copy(src, dst)?;
        }
        for entry in members {
            let name = &entry.file;
            if [
                "runtimes/",
                "data/models/",
                "licenses/native/",
                "licenses/data/",
            ]
            .iter()
            .any(|prefix| name.starts_with(prefix))
                || matches!(
                    name.as_str(),
                    "LICENSE" | "THIRD_PARTY_NOTICES.md" | "README.md"
                )
                || (cfg!(windows)
                    && !name.contains('/')
                    && name.to_ascii_lowercase().ends_with(".dll"))
            {
                let dest = self.payload.join(name);
                helpers::copy(&self.portable.join(name), &dest)?;
                self.resource(&dest, name)?;
            } else if ![
                self.gui_name(),
                self.cli_name(),
                "portable.flag",
                "assets/logo.svg",
            ]
            .contains(&name.as_str())
            {
                return Err(format!(
                    "Unexpected portable file outside the bundle allowlist: {name}"
                )
                .into());
            }
        }
        helpers::copy(&logo, &self.payload.join("assets/logo.svg"))?;
        for icon in config["bundle"]["icon"]
            .as_array()
            .ok_or("Missing approved desktop icon list.")?
        {
            let icon = icon
                .as_str()
                .filter(|name| safe_relative(name))
                .ok_or("Invalid desktop icon path.")?;
            if !self.desktop.join(icon).is_file() {
                return Err(format!("Missing official generated desktop icon: {icon}").into());
            }
        }
        Ok(())
    }

    fn resource(&mut self, source: &Path, name: &str) -> Result<()> {
        self.resources
            .insert(path_text(source)?.replace('\\', "/"), name.to_owned());
        Ok(())
    }

    fn prepare_linux(&mut self) -> Result<String> {
        let helper = self.payload.join("runtimes/llama-mtmd-cli");
        elf(&helper, &self.target.arch, false)?;
        self.command(
            "helper-rpath-normalize",
            "patchelf",
            ["--set-rpath", "$ORIGIN", path_text(&helper)?],
            (15, 5),
            false,
            None,
        )?;
        let rpath = format!("$ORIGIN/../lib/{}/runtimes", self.product);
        for file in [self.main.clone(), self.cli.clone()] {
            self.command(
                "binary-rpath",
                "patchelf",
                ["--set-rpath", &rpath, path_text(&file)?],
                (120, 60),
                false,
                None,
            )?;
        }
        let main = self.main.clone();
        let cli = self.cli.clone();
        self.command(
            "executable-mode",
            "chmod",
            ["755", path_text(&main)?, path_text(&cli)?],
            (120, 60),
            false,
            None,
        )?;
        let cpus = helpers::cpu_list(&fs::read_to_string("/proc/self/status")?)?;
        self.run.log(
            "INFO",
            &format!("Linux bundler CPU affinity={cpus}; compression descendants inherit it"),
        )?;
        Ok(cpus)
    }

    fn inspect_format(&mut self, format: &str, payload: &[Member]) -> Result<PathBuf> {
        let (suffix, magic): (&str, &[u8]) = match format {
            "nsis" if cfg!(windows) => ("-setup.exe", b"MZ"),
            "appimage" if !cfg!(windows) => (".AppImage", b"\x7fELF"),
            "deb" if !cfg!(windows) => (".deb", b"!<arch>\n"),
            "rpm" if !cfg!(windows) => (".rpm", b"\xed\xab\xee\xdb"),
            _ => return Err("Unsupported native installer format.".into()),
        };
        let mut found = entries(&self.release.join("bundle").join(format))?
            .into_iter()
            .filter(|file| basename(file).is_ok_and(|name| name.ends_with(suffix)));
        let file = found
            .next()
            .ok_or("Required installer format is missing.")?;
        if found.next().is_some() {
            return Err("Required installer format is ambiguous.".into());
        }
        let dest = self.output.join(basename(&file)?);
        helpers::copy(&file, &dest)?;
        header(&dest, 0, magic)?;
        let inspect = self.stage.path().join(format!("inspect-{format}"));
        fs::create_dir(&inspect)?;
        #[cfg(windows)]
        self.inspect_nsis(&dest, &inspect, payload)?;
        #[cfg(not(windows))]
        self.inspect_linux(format, &dest, &inspect, payload)?;
        Ok(dest)
    }

    #[cfg(not(windows))]
    fn inspect_linux(
        &mut self,
        format: &str,
        dest: &Path,
        inspect: &Path,
        payload: &[Member],
    ) -> Result<()> {
        let archive = path_text(dest)?;
        let directory = path_text(inspect)?;
        let tree = match format {
            "deb" => {
                self.command(
                    "deb-content",
                    "dpkg-deb",
                    ["-X", archive, directory],
                    (180, 90),
                    false,
                    None,
                )?;
                inspect.to_owned()
            }
            "rpm" => {
                self.command(
                    "rpm-content",
                    "bsdtar",
                    ["-xvf", archive, "-C", directory, "--no-same-owner"],
                    (180, 90),
                    false,
                    None,
                )?;
                inspect.to_owned()
            }
            "appimage" => {
                header(dest, 8, b"AI\x02")?;
                self.command(
                    "appimage-mode",
                    "chmod",
                    ["755", archive],
                    (120, 60),
                    false,
                    None,
                )?;
                self.command(
                    "appimage-content",
                    dest,
                    ["--appimage-extract"],
                    (180, 90),
                    false,
                    Some(inspect),
                )?;
                inspect.join("squashfs-root")
            }
            _ => return Err("Unsupported Linux installer format.".into()),
        };
        self.installed(
            &tree.join("usr/lib").join(&self.product),
            &tree.join("usr/bin"),
            format,
            payload,
        )?;
        helpers::linux_icons(&self.desktop, &tree, format, &mut self.icons)?;
        Ok(())
    }

    fn installed(
        &mut self,
        resources: &Path,
        bin: &Path,
        format: &str,
        payload: &[Member],
    ) -> Result<()> {
        let patched = format == "appimage";
        for member in payload {
            let path = helpers::owned(&self.root, &resources.join(&member.file))?;
            if patched
                && member.file.starts_with("runtimes/")
                && (member.file.ends_with(".so") || member.file.contains(".so."))
            {
                elf(&path, &self.target.arch, false)?;
            } else {
                member.verify(&path)?;
            }
            if !cfg!(windows) && member.file == "runtimes/llama-mtmd-cli" {
                elf(&path, &self.target.arch, false)?;
                if self.command(
                    "helper-rpath",
                    "patchelf",
                    ["--print-rpath", path_text(&path)?],
                    (15, 5),
                    true,
                    None,
                )? != "$ORIGIN"
                {
                    return Err(
                        "Packaged visual helper has an unexpected runtime search path.".into(),
                    );
                }
            }
        }
        let gui = helpers::owned(
            &self.root,
            &bin.join(format!("{GUI}{}", std::env::consts::EXE_SUFFIX)),
        )?;
        let cli = helpers::owned(&self.root, &bin.join(self.cli_name()))?;
        if patched {
            elf(&cli, &self.target.arch, false)?;
        } else {
            Member::read(&self.cli, self.stage.path())?.verify(&cli)?;
        }
        let marker = helpers::marker(format)?;
        let (expected, actual) = helpers::patched_gui(&self.main, &gui, marker)?;
        if patched {
            elf(&gui, &self.target.arch, false)?;
            if helpers::has_marker(&actual, helpers::ORIGINAL_MARKER)
                || !helpers::has_marker(&actual, marker)
            {
                return Err("Packaged AppImage GUI is missing its native updater marker.".into());
            }
        } else {
            #[cfg(windows)]
            windows::signed_gui(expected, &gui)?;
            #[cfg(not(windows))]
            if expected != actual {
                return Err(
                    "Packaged GUI differs beyond the reviewed native bundle marker.".into(),
                );
            }
        }
        if bin.join("portable.flag").exists() {
            return Err(
                "Installed bundles must not redirect writable data into installation resources."
                    .into(),
            );
        }
        Ok(())
    }

    #[cfg(not(windows))]
    fn tar(&mut self) -> Result<PathBuf> {
        let archive = self.output.join(format!("{}.tar.gz", self.base_name));
        let inspect = self.stage.path().join("inspect-tar");
        fs::create_dir(&inspect)?;
        let packages = self.packages.clone();
        let name = self.base_name.clone();
        self.command(
            "native-archive",
            "tar",
            [
                "-czvf",
                path_text(&archive)?,
                "-C",
                path_text(&packages)?,
                &name,
            ],
            (300, 120),
            false,
            None,
        )?;
        header(&archive, 0, b"\x1f\x8b")?;
        self.command(
            "archive-integrity",
            "tar",
            ["-xzvf", path_text(&archive)?, "-C", path_text(&inspect)?],
            (180, 90),
            false,
            None,
        )?;
        helpers::archive_bytes(&self.portable, &inspect.join(name))?;
        Ok(archive)
    }

    fn artifact(&self, format: &str, file: &Path) -> Result<Value> {
        let member = Member::read(file, &self.packages)?;
        Ok(json!({"format":format,"file":member.file,"bytes":member.bytes,"sha256":member.sha256}))
    }

    fn checksums(&self) -> Result<()> {
        let files = helpers::inventory(&self.output, &self.output)?;
        if files.is_empty() {
            return Err("Refusing to emit empty SHA256SUMS.txt.".into());
        }
        let expected = files
            .iter()
            .map(|file| format!("{}  {}\n", file.sha256, file.file))
            .collect::<String>();
        let path = self.output.join("SHA256SUMS.txt");
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?
            .write_all(expected.as_bytes())?;
        if fs::read_to_string(&path)? != expected {
            return Err("SHA256SUMS.txt write/readback mismatch.".into());
        }
        for file in files {
            file.verify(&self.output.join(&file.file))?;
        }
        Ok(())
    }
}
