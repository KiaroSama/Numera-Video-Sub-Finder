#[path = "ci_native_platform.rs"]
mod platform;
#[path = "ci_native_sources.rs"]
mod sources;

use crate::{
    ci_guard::{Result, unlinked},
    ci_native_helpers::{
        Member, Session, Target, admission, basename, entries, json, path_text, safe_relative, sha,
    },
};
use serde_json::{Value, json};
use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

const BASELINE: &str = "e456309491fd875487bf02b8a0dca76c1b1b00cc";
const OVERLAYS: &[&str] = &[
    "vcpkg-configuration.json",
    "ci/ports/tesseract/vcpkg.json",
    "ci/ports/tesseract/portfile.cmake",
    "ci/ports/tesseract/fix_static_link_icu.patch",
    "ci/ports/tesseract/fix-link-include-path.patch",
    "ci/ports/tesseract/target-curl.diff",
    "ci/ports/tesseract/LICENSE.vcpkg.txt",
];

struct Native {
    root: PathBuf,
    target: Target,
    source: String,
    work: PathBuf,
    runtimes: PathBuf,
    notices: PathBuf,
    overlay: PathBuf,
    prefix: PathBuf,
    vcpkg: PathBuf,
    run: Session,
}

pub(super) fn execute(id: &str, stage_media: bool) -> Result<()> {
    let (root, target, source) = admission(
        id,
        "Native dependency provisioning is restricted to GitHub-hosted Actions runners.",
    )?;
    let work = root.join(".ci-work/native");
    if path_text(&work)?.contains(' ') {
        return Err("Pinned FFmpeg rejects spaces in its source build path; use a space-free hosted checkout.".into());
    }
    let runtimes = root.join(".ci-work/runtimes");
    let notices = root.join(".ci-work/native-notices");
    let run = Session::new(&root, "native", "NATIVE-CI")?;
    for dir in [
        &work,
        &runtimes,
        &notices,
        &work.join("temp"),
        &work.join("downloads"),
        &work.join("binary-cache"),
    ] {
        fs::create_dir_all(dir)?;
        if !unlinked(dir)?.is_dir() {
            return Err("Native CI directory is invalid.".into());
        }
    }
    let mut native = Native {
        prefix: work.join("installed").join(&target.triplet),
        vcpkg: work.join("vcpkg"),
        overlay: root.join("ci/triplets"),
        root,
        target,
        source,
        work,
        runtimes,
        notices,
        run,
    };
    let result = native.execute(stage_media);
    if let Err(error) = &result {
        native.run.log("ERROR", &error.to_string())?;
    }
    result
}

impl Native {
    fn execute(&mut self, stage_media: bool) -> Result<()> {
        let lock = json(&self.root.join("ci/native-lock.json"))?;
        let port = json(&self.root.join("ci/ports/tesseract/vcpkg.json"))?;
        if json(&self.root.join("vcpkg-configuration.json"))?["overlay-ports"]
            != json!(["ci/ports"])
        {
            return Err(
                "Native configuration must select the reviewed local overlay ports.".into(),
            );
        }
        if lock["vcpkg_revision"] != BASELINE
            || lock["tesseract"] != "5.5.4"
            || port["version"] != lock["tesseract"]
        {
            return Err("Native lock and reviewed Tesseract overlay version must agree.".into());
        }
        let mut overlay_inputs = serde_json::Map::new();
        for &relative in OVERLAYS {
            overlay_inputs.insert(relative.to_owned(), json!(sha(&self.root.join(relative))?));
        }
        let lock_sha = sha(&self.root.join("Cargo.lock"))?;
        if stage_media {
            return self.stage_media(&lock_sha, &Value::Object(overlay_inputs));
        }
        self.run.log(
            "INFO",
            &format!(
                "Starting target={} root={} runtime_root={} workers=2",
                self.target.id,
                self.root.display(),
                self.runtimes.display()
            ),
        )?;
        self.install_ports()?;
        self.export_toolchain()?;
        self.stage_ports()?;
        sources::onnx(self)?;
        platform::prepare_runtime(self)?;
        let mut record = sources::llama(self)?;
        let fields = json!({"schema":1, "target":self.target.id, "source_sha":self.source,
            "rust_target":self.target.triple, "cargo_lock_sha256":lock_sha, "vcpkg_commit":BASELINE,
            "tesseract_version":lock["tesseract"], "tesseract_source_commit":"7e548e6eff172f38f97664afe7f7ce40db36c8d6",
            "overlay_port_inputs":overlay_inputs, "ort_version":"1.31.0", "overlay_triplet":self.target.triplet,
            "linkage":"dynamic", "build_type":"release", "media_adapter":null,
            "runtime_execution":"NOT_RUN", "final_package_closure":"PENDING_HOSTED_TEST"});
        record.extend(
            fields
                .as_object()
                .ok_or("Invalid native inventory fields.")?
                .clone(),
        );
        let mut record = Value::Object(record);
        self.populate_inventory(&mut record)?;
        self.write_inventory(&record)?;
        self.run.log("INFO", "Provisioning complete; media adapter staging and package/model inference remain required hosted checks.")
    }

    fn install_ports(&mut self) -> Result<()> {
        if json(&self.root.join("vcpkg.json"))?["builtin-baseline"] != BASELINE
            || !self
                .overlay
                .join(format!("{}.cmake", self.target.triplet))
                .is_file()
        {
            return Err("Native manifest baseline or dynamic overlay triplet differs from the reviewed configuration.".into());
        }
        for (name, value) in [
            ("VCPKG_DISABLE_METRICS", "1"),
            ("VCPKG_MAX_CONCURRENCY", "2"),
            ("CMAKE_BUILD_PARALLEL_LEVEL", "2"),
            ("MSBUILDDISABLENODEREUSE", "1"),
            ("MSBUILD_TELEMETRY_OPTOUT", "1"),
            ("CARGO_BUILD_JOBS", "2"),
            ("OMP_NUM_THREADS", "2"),
            ("GIT_TERMINAL_PROMPT", "0"),
        ] {
            self.run.set(name, value);
        }
        for (name, value) in [
            ("VCPKG_DOWNLOADS", "downloads"),
            ("VCPKG_DEFAULT_BINARY_CACHE", "binary-cache"),
            ("TMP", "temp"),
            ("TEMP", "temp"),
            ("TMPDIR", "temp"),
        ] {
            self.run.set(name, self.work.join(value));
        }
        self.run.set("VCPKG_ROOT", &self.vcpkg);
        let vcpkg = path_text(&self.vcpkg)?;
        if !self.vcpkg.exists() {
            self.run.run(
                "vcpkg-checkout",
                "git",
                [
                    "clone",
                    "--filter=blob:none",
                    "--no-checkout",
                    "https://github.com/microsoft/vcpkg.git",
                    vcpkg,
                ],
                (600, 180),
            )?;
        }
        self.run.run(
            "vcpkg-pin",
            "git",
            ["-C", vcpkg, "checkout", "--detach", BASELINE],
            (300, 120),
        )?;
        if self.run.capture(
            "vcpkg-revision",
            "git",
            ["-C", vcpkg, "rev-parse", "HEAD"],
            (300, 120),
        )? != BASELINE
        {
            return Err("Pinned native dependency checkout verification failed.".into());
        }
        platform::bootstrap(self)?;
        let exe = self
            .vcpkg
            .join(if cfg!(windows) { "vcpkg.exe" } else { "vcpkg" });
        let mut command = Command::new(exe);
        command
            .arg("install")
            .arg(format!("--triplet={}", self.target.triplet))
            .arg(format!("--host-triplet={}", self.target.triplet))
            .arg(format!("--overlay-triplets={}", self.overlay.display()))
            .arg(format!("--x-manifest-root={}", self.root.display()))
            .arg(format!(
                "--x-install-root={}/installed",
                self.work.display()
            ))
            .arg("--disable-metrics");
        self.run.command(
            "native-ports",
            command,
            (6600, 900),
            false,
            Some(&self.vcpkg.join("buildtrees")),
        )?;
        for relative in [
            "include/libavcodec/avcodec.h",
            "include/libavformat/avformat.h",
            "include/libavutil/avutil.h",
            "include/libswscale/swscale.h",
            "include/tesseract/capi.h",
            "share/ffmpeg/copyright",
            "share/tesseract/copyright",
        ] {
            if !self.prefix.join(relative).is_file() {
                return Err(
                    format!("Native installation missing required file: {relative}.").into(),
                );
            }
        }
        Ok(())
    }

    fn export_toolchain(&mut self) -> Result<()> {
        for (name, value) in [
            ("FFMPEG_DIR", self.prefix.clone()),
            ("FFMPEG_INCLUDE_DIR", self.prefix.join("include")),
            ("FFMPEG_LIB_DIR", self.prefix.join("lib")),
            ("VCPKG_ROOT", self.vcpkg.clone()),
            ("VCPKG_INSTALLED_ROOT", self.work.join("installed")),
            ("VCPKG_OVERLAY_TRIPLETS", self.overlay.clone()),
            ("NVSF_RUNTIME_ROOT", self.runtimes.clone()),
            ("NVSF_NATIVE_NOTICES_ROOT", self.notices.clone()),
        ] {
            self.run.export(name, value)?;
        }
        for (name, value) in [
            ("VCPKG_TARGET_TRIPLET", self.target.triplet.as_str()),
            ("VCPKGRS_TRIPLET", &self.target.triplet),
            ("VCPKGRS_DYNAMIC", "1"),
            ("VCPKG_DISABLE_METRICS", "1"),
            ("VCPKG_MAX_CONCURRENCY", "2"),
            ("CMAKE_BUILD_PARALLEL_LEVEL", "2"),
            ("ORT_SKIP_DOWNLOAD", "1"),
            ("ORT_OFFLINE", "1"),
        ] {
            self.run.export(name, value)?;
        }
        platform::export(self)
    }

    fn stage_ports(&self) -> Result<()> {
        for port in entries(&self.prefix.join("share"))? {
            let copyright = port.join("copyright");
            if copyright.is_file() {
                let destination = self.notices.join(basename(&port)?);
                fs::create_dir_all(&destination)?;
                fs::copy(copyright, destination.join("copyright"))?;
            }
        }
        for file in entries(&self.prefix.join(if cfg!(windows) { "bin" } else { "lib" }))? {
            if file.is_file() && platform::shared_library(basename(&file)?) {
                self.copy_runtime(&file)?;
            }
        }
        Ok(())
    }

    fn copy_runtime(&self, path: &Path) -> Result<()> {
        let source = fs::canonicalize(path)?;
        if !unlinked(path).is_ok_and(|metadata| metadata.is_file())
            && !source.starts_with(fs::canonicalize(&self.work)?)
        {
            return Err("Native archive link escapes its owned build/extraction root.".into());
        }
        if !unlinked(&source)?.is_file() {
            return Err("Native runtime source is not a regular file.".into());
        }
        let dest = self.runtimes.join(basename(path)?);
        match unlinked(&dest) {
            Ok(metadata) if metadata.is_file() && sha(&dest)? == sha(&source)? => (),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                fs::copy(source, dest)?;
            }
            _ => {
                return Err(
                    format!("Conflicting native runtime name: {}.", basename(path)?).into(),
                );
            }
        }
        Ok(())
    }

    fn populate_inventory(&self, record: &mut Value) -> Result<()> {
        let mut files = Vec::new();
        for path in entries(&self.runtimes)? {
            if !unlinked(&path)?.is_file() {
                return Err("Unexpected non-file in native runtime inventory.".into());
            }
            files.push(Member::read(&path, &self.runtimes)?);
        }
        record["runtime_files"] = serde_json::to_value(files)?;
        Ok(())
    }

    fn write_inventory(&self, record: &Value) -> Result<()> {
        let file = tempfile::NamedTempFile::new_in(&self.notices)?;
        serde_json::to_writer_pretty(file.as_file(), record)?;
        file.persist(self.notices.join("native-inventory.json"))?;
        Ok(())
    }

    fn stage_media(&mut self, lock: &str, overlays: &Value) -> Result<()> {
        let mut record = json(&self.notices.join("native-inventory.json"))?;
        if record["schema"] != 1
            || record["target"] != self.target.id
            || record["source_sha"] != self.source
            || record["rust_target"] != self.target.triple
            || record["cargo_lock_sha256"] != lock
            || record["vcpkg_commit"] != BASELINE
            || record["tesseract_version"] != "5.5.4"
            || record["overlay_port_inputs"] != *overlays
        {
            return Err("Media staging requires the same-source/target/lock native inventory and overlay inputs.".into());
        }
        let members: Vec<Member> = serde_json::from_value(record["runtime_files"].clone())?;
        let mut known = BTreeSet::new();
        if !(1..=512).contains(&members.len()) || entries(&self.runtimes)?.len() != members.len() {
            return Err("Invalid prior native inventory size or uninventoried files.".into());
        }
        for entry in members {
            if !safe_relative(&entry.file)
                || entry.file.contains('/')
                || !known.insert(entry.file.to_ascii_lowercase())
            {
                return Err("Unsafe or duplicate prior native inventory member.".into());
            }
            entry.verify(&self.runtimes.join(&entry.file))?;
        }
        let name = if cfg!(windows) {
            "nvsf_media_native.dll"
        } else {
            "libnvsf_media_native.so"
        };
        let artifact = self
            .root
            .join("target")
            .join(&self.target.triple)
            .join("release")
            .join(name);
        let meta = unlinked(&artifact)?;
        if !meta.is_file() || meta.len() == 0 {
            return Err(
                "Media adapter must be a nonempty regular same-target Cargo release artifact."
                    .into(),
            );
        }
        platform::media_machine(&artifact, &self.target.arch)?;
        let dest = self.runtimes.join(name);
        if dest.symlink_metadata().is_ok() {
            return Err("Refusing to overwrite an already-staged media adapter.".into());
        }
        self.copy_runtime(&artifact)?;
        platform::closure(self, std::slice::from_ref(&dest))?;
        let (exe, flags): (&str, &[&str]) = if cfg!(windows) {
            ("dumpbin", &["/NOLOGO", "/EXPORTS"])
        } else {
            ("nm", &["-D", "--defined-only"])
        };
        let output = self.run.capture(
            "media-exports",
            exe,
            flags.iter().copied().chain([path_text(&dest)?]),
            (30, 10),
        )?;
        for symbol in [
            "nvsf_media_probe_v1",
            "nvsf_media_decode_v1",
            "nvsf_media_health_v1",
        ] {
            if !output
                .lines()
                .filter_map(|line| line.split_ascii_whitespace().last())
                .any(|last| last == symbol || last.strip_prefix('_') == Some(symbol))
            {
                return Err(format!("Required native media C ABI export absent: {symbol}.").into());
            }
        }
        record["media_adapter"] = json!({"file":name,"abi_version":1,"rust_target":self.target.triple,"profile":"release","cargo_lock_sha256":lock,"input_sha256":sha(&artifact)?,"sha256":sha(&dest)?});
        self.populate_inventory(&mut record)?;
        crate::package::validate_media_inventory(
            &record,
            &self.target.id,
            &self.target.triple,
            &self.source,
            lock,
            &self.runtimes,
        )?;
        self.write_inventory(&record)?;
        self.run.log("INFO", &format!("Media adapter staged target={} abi=1 closure_relocated=true inventory_finalized=true; runtime ABI/extraction remains a later hosted check.", self.target.id))
    }
}
