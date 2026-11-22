use super::{Bundle, STATUS, helpers};
use crate::{
    ci_authenticode::{self, SIGNER_SHA256},
    ci_crt,
    ci_guard::Result,
    ci_native_helpers::{Member, basename, crt_directory, entries, files, json, path_text, sha},
    ci_notices, executable_icons,
};
use serde_json::{Value, json};
use std::{
    collections::{BTreeSet, VecDeque},
    env, fs,
    path::{Path, PathBuf},
};

pub(super) struct Identity(bool);

impl Identity {
    pub fn enable() -> Result<Self> {
        // Construct before import so a partial certificate-store change is cleaned up on error.
        let identity = Self(true);
        ci_authenticode::enable()?;
        Ok(identity)
    }

    pub fn close(mut self) -> Result<()> {
        ci_authenticode::disable()?;
        self.0 = false;
        Ok(())
    }
}

impl Drop for Identity {
    fn drop(&mut self) {
        if self.0
            && let Err(error) = ci_authenticode::disable()
        {
            eprintln!(
                "[ERROR] [DESKTOP-BUNDLE] Failed to remove the development signing identity: {error}"
            );
        }
    }
}

pub(super) fn signed_gui(expected: Vec<u8>, path: &Path) -> Result<()> {
    ci_authenticode::verify(path, SIGNER_SHA256)?;
    let baseline = ci_authenticode::unsigned_pe(expected)?;
    let actual = ci_authenticode::unsigned_file(path)?;
    let padding = (8 - baseline.len() % 8) % 8;
    if actual.len() != baseline.len() + padding
        || !actual.starts_with(&baseline)
        || actual[baseline.len()..].iter().any(|byte| *byte != 0)
    {
        return Err(
            "Signed GUI differs from the exact marker-patched baseline and zero alignment padding."
                .into(),
        );
    }
    Ok(())
}

impl Bundle {
    pub(super) fn prepare_windows(&mut self) -> Result<()> {
        let vs =
            PathBuf::from(env::var_os("VCPKG_VISUAL_STUDIO_PATH").ok_or(
                "Native provisioning must export the discovered Visual Studio installation.",
            )?);
        let (version, crt) = crt_directory(&vs, &self.target.arch)?;
        let mut queue = VecDeque::from([self.main.clone(), self.cli.clone()]);
        let mut known = BTreeSet::new();
        while let Some(binary) = queue.pop_front() {
            let output = self.command(
                "crt-imports",
                "dumpbin.exe",
                ["/NOLOGO", "/DEPENDENTS", path_text(&binary)?],
                (30, 10),
                true,
                None,
            )?;
            for name in ci_crt::parse_release_crt_imports(&output)? {
                if !known.insert(name.clone()) {
                    continue;
                }
                if known.len() > 32 {
                    return Err("Imported release CRT closure exceeds 32 members.".into());
                }
                let dll = crt.join(&name);
                ci_crt::assert_release_crt_machine(&dll, &self.target.arch)?;
                let dest = self.payload.join(&name);
                if dest.exists() {
                    if sha(&dll)? != sha(&dest)? {
                        return Err(
                            format!("Redistributable DLL bytes collide: member={name}.").into()
                        );
                    }
                } else {
                    helpers::copy(&dll, &dest)?;
                }
                self.resource(&dest, &name)?;
                queue.push_back(dest);
            }
        }
        let notice = self
            .payload
            .join("licenses/microsoft/visual-cpp-runtime.txt");
        fs::create_dir_all(notice.parent().ok_or("Missing CRT notice directory.")?)?;
        fs::write(
            &notice,
            format!(
                "Microsoft Visual C++ Runtime DLLs from installed Visual Studio Redist/MSVC/{version}/{}.\nRedistribution is subject to the applicable Microsoft Software License Terms and licensed Visual Studio user rights; this development artifact is not release-qualified.\nhttps://learn.microsoft.com/en-us/cpp/windows/redistributing-visual-cpp-files\n",
                self.target.arch
            ),
        )?;
        self.resource(&notice, "licenses/microsoft/visual-cpp-runtime.txt")?;
        let terms = self.payload.join("licenses/microsoft/visual-studio");
        for rel in ci_notices::copy_visual_studio_notices(&vs.join("Licenses"), &terms)? {
            self.resource(
                &terms.join(&rel),
                &format!("licenses/microsoft/visual-studio/{rel}"),
            )?;
        }
        self.icon(&self.main.clone(), "portable-gui")?;
        self.icon(&self.cli.clone(), "portable-cli")?;
        self.signatures
            .push(json!({"role":"cli-sidecar","evidence":ci_authenticode::sign(&self.cli)?}));
        ci_authenticode::signature_negatives(&self.cli)?;
        self.icons.push("signature-negatives".to_owned());
        Ok(())
    }

    fn icon(&mut self, binary: &Path, label: &str) -> Result<()> {
        let evidence = self.stage.path().join("icon-evidence");
        fs::create_dir_all(&evidence)?;
        executable_icons::verify(
            binary,
            &self.desktop.join("icons").join("icon.ico"),
            &evidence.join(format!("{label}.json")),
        )?;
        self.icons.push(label.to_owned());
        Ok(())
    }

    pub(super) fn signing_config(&mut self) -> Result<Value> {
        let xtask = env::current_exe()?;
        let ledger = self.stage.path().join("signing-ledger");
        fs::create_dir(&ledger)?;
        let temp = fs::canonicalize(PathBuf::from(
            env::var_os("TEMP").ok_or("Missing owned Windows temp directory.")?,
        ))?;
        for (key, path) in [
            ("NVSF_SIGNING_ROOT", self.root.clone()),
            ("NVSF_SIGNING_MAIN", self.main.clone()),
            ("NVSF_SIGNING_PAYLOAD", self.payload.clone()),
            ("NVSF_SIGNING_RELEASE", self.release.clone()),
            ("NVSF_SIGNING_TEMP", temp),
            ("NVSF_SIGNING_LEDGER", ledger),
            ("NVSF_SIGNING_XTASK", xtask.clone()),
            (
                "NVSF_SIGNING_ICON",
                self.desktop.join("icons").join("icon.ico"),
            ),
        ] {
            self.run.set(key, path);
        }
        Ok(
            json!({"signCommand":{"cmd":path_text(&xtask)?.replace('\\', "/"),"args":["ci-windows-signing","--path","%1"]}}),
        )
    }

    pub(super) fn inspect_nsis(
        &mut self,
        setup: &Path,
        inspect: &Path,
        payload: &[Member],
    ) -> Result<()> {
        self.command(
            "nsis-content",
            "7z.exe",
            [
                "x",
                "-y",
                "-mmt2",
                "-sccUTF-8",
                &format!("-o{}", inspect.display()),
                path_text(setup)?,
            ],
            (180, 90),
            false,
            None,
        )?;
        let cli = files(inspect)?
            .into_iter()
            .filter(|path| basename(path).is_ok_and(|name| name.eq_ignore_ascii_case("nvsf.exe")))
            .collect::<Vec<_>>();
        let [cli] = cli.as_slice() else {
            return Err("NSIS extraction must contain exactly one native CLI.".into());
        };
        let bin = cli.parent().ok_or("Missing NSIS binary directory.")?;
        self.installed(bin, bin, "nsis", payload)?;
        let gui = bin.join("numera-video-sub-finder.exe");
        self.icon(&gui, "nsis-gui")?;
        self.icon(cli, "nsis-cli")?;
        self.icon(setup, "nsis-setup")?;
        for (role, file) in [("nsis-gui", gui.as_path()), ("nsis-setup", setup)] {
            self.signatures
                .push(json!({"role":role,"evidence":ci_authenticode::verify(file,SIGNER_SHA256)?}));
        }
        let mut roles = BTreeSet::new();
        let mut uninstaller_icons = false;
        for file in entries(&self.stage.path().join("signing-ledger"))? {
            let name = basename(&file)?;
            if !name.ends_with(".json") {
                continue;
            }
            if name.starts_with("uninstaller-icons-") {
                uninstaller_icons = true;
            } else {
                roles.insert(
                    json(&file)?["role"]
                        .as_str()
                        .ok_or("Missing signer role.")?
                        .to_owned(),
                );
            }
        }
        for role in ["gui", "uninstaller", "setup"] {
            if !roles.contains(role) {
                return Err(format!("Custom signer did not observe required role: {role}.").into());
            }
        }
        if !uninstaller_icons {
            return Err("Uninstaller icon evidence is missing.".into());
        }
        self.signatures.push(json!({"role":"nsis-uninstaller","evidence":"signed, pinned-verified and icon-checked by the allowlisted custom signer before NSIS embedding"}));
        Ok(())
    }

    pub(super) fn zip(&mut self, members: &[Member], signing: &str) -> Result<PathBuf> {
        let zip_root = self.stage.path().join(&self.base_name);
        fs::create_dir(&zip_root)?;
        for member in members {
            helpers::copy(
                &self.portable.join(&member.file),
                &zip_root.join(&member.file),
            )?;
        }
        self.signatures.push(json!({"role":"portable-gui","evidence":ci_authenticode::sign(&zip_root.join(self.gui_name()))?}));
        fs::copy(&self.cli, zip_root.join(self.cli_name()))?;
        for member in helpers::inventory(&self.payload, &self.payload)? {
            let dest = zip_root.join(&member.file);
            if !dest.exists() {
                helpers::copy(&self.payload.join(&member.file), &dest)?;
            }
        }
        helpers::write_json(
            &zip_root.join("manifest.json"),
            &json!({"schema":1,"source_sha":self.source,"target":self.target.id,"status":STATUS,"signing":signing,"files":helpers::inventory(&zip_root,&zip_root)?}),
        )?;
        let archive = self.output.join(format!("{}.zip", self.base_name));
        if archive.exists() {
            return Err("Refusing to overwrite portable ZIP.".into());
        }
        let stage = self.stage.path().to_path_buf();
        let name = self.base_name.clone();
        self.command(
            "portable-zip",
            "7z.exe",
            [
                "a",
                "-tzip",
                "-mm=Deflate",
                "-mmt2",
                "-sccUTF-8",
                path_text(&archive)?,
                &name,
            ],
            (300, 120),
            false,
            Some(&stage),
        )?;
        let inspect = stage.join("inspect-zip");
        fs::create_dir(&inspect)?;
        self.command(
            "zip-integrity",
            "7z.exe",
            [
                "x",
                "-y",
                "-mmt2",
                "-sccUTF-8",
                &format!("-o{}", inspect.display()),
                path_text(&archive)?,
            ],
            (300, 120),
            false,
            Some(&stage),
        )?;
        if entries(&inspect)? != [inspect.join(&name)] {
            return Err("ZIP inventory has unexpected top-level entries.".into());
        }
        helpers::archive_bytes(&zip_root, &inspect.join(name))?;
        Ok(archive)
    }
}
