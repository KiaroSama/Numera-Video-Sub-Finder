use super::{Bundle, STATUS, helpers};
use crate::{
    ci_authenticode::{self, SIGNER_SHA256},
    ci_crt,
    ci_guard::Result,
    ci_guard::unlinked,
    ci_native_helpers::{
        Member, basename, crt_directory, entries, json, path_text, safe_relative, sha,
    },
    ci_notices, executable_icons,
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde::Deserialize;
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

#[derive(Deserialize)]
struct MsiInventory {
    template: String,
    language: String,
    product_icon: String,
    shortcut_icon: String,
    files: Vec<MsiFile>,
}

#[derive(Deserialize)]
struct MsiFile {
    id: String,
    path: String,
    bytes: u64,
}

// Dark extracts the cabinet by File-table key, not by the installed filename.
const MSI_TABLES: &str = r#"
$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = [Text.UTF8Encoding]::new($false)
$options = [Xml.XmlReaderSettings]::new()
$options.DtdProcessing = [Xml.DtdProcessing]::Prohibit
$options.MaxCharactersInDocument = 4194304
$reader = [Xml.XmlReader]::Create($env:NVSF_MSI_TABLES, $options)
try { $xml = [Xml.XmlDocument]::new(); $xml.Load($reader) } finally { $reader.Dispose() }
function Rows([string]$table) {
    $xml.SelectNodes("/*[local-name()='wixOutput' and @type='Product']/*[local-name()='table' and @name='$table']/*[local-name()='row']")
}
function Fields($row) { $row.SelectNodes("*[local-name()='field']") | ForEach-Object { $_.InnerText } }
function LongName([string]$name) { ($name -split '\|')[-1] }
$directories = @{}
foreach ($row in (Rows 'Directory')) {
    $key, $parent, $name = Fields $row
    $directories.Add($key, @($parent, (LongName (($name -split ':')[0]))))
}
$components = @{}
foreach ($row in (Rows 'Component')) {
    $fields = @(Fields $row)
    $components.Add($fields[0], $fields[2])
}
$files = @(foreach ($row in (Rows 'File')) {
    $fields = @(Fields $row)
    $id, $component, $name, $size = $fields[0..3]
    $path = LongName $name
    $dir = $components[$component]
    $depth = 0
    while ($dir -cne 'INSTALLDIR') {
        if (++$depth -gt 16 -or !$dir -or !$directories.ContainsKey($dir)) {
            throw 'MSI file is not rooted under INSTALLDIR'
        }
        $dir, $folder = $directories[$dir]
        if ($folder -cne '.') { $path = "$folder/$path" }
    }
    @{id=$id; path=$path; bytes=[UInt64]$size}
})
$properties = @{}
foreach ($row in (Rows 'Property')) { $key, $value = Fields $row; $properties.Add($key, $value) }
$template = foreach ($row in (Rows '_SummaryInformation')) {
    $key, $value = Fields $row
    if ($key -ceq '7') { $value }
}
$shortcut = foreach ($row in (Rows 'Shortcut')) {
    $fields = @(Fields $row)
    if ($fields[0] -ceq 'ApplicationStartMenuShortcut') { $fields[8] }
}
[Console]::WriteLine((@{
    template=$template; language=$properties.ProductLanguage; product_icon=$properties.ARPPRODUCTICON;
    shortcut_icon=$shortcut; files=$files
} | ConvertTo-Json -Compress -Depth 4))
"#;

fn restore_msi_files(cabinet: &Path, installed: &Path, rows: Vec<MsiFile>) -> Result<()> {
    if !unlinked(cabinet)?.is_dir()
        || rows.is_empty()
        || rows.len() > 4096
        || rows.len() != entries(cabinet)?.len()
    {
        return Err("MSI cabinet differs from its bounded File table.".into());
    }
    let mut ids = BTreeSet::new();
    let mut paths = BTreeSet::new();
    for row in rows {
        if !safe_relative(&row.id)
            || !row
                .id
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'.'))
            || !safe_relative(&row.path)
            || row.path.contains(['<', '>', '"', '|', '?', '*'])
            || !ids.insert(row.id.to_ascii_lowercase())
            || !paths.insert(row.path.to_ascii_lowercase())
        {
            return Err("Invalid or duplicate MSI File table path.".into());
        }
        let src = cabinet.join(row.id);
        let metadata = unlinked(&src)?;
        if !metadata.is_file() || metadata.len() != row.bytes {
            return Err("MSI cabinet member differs from its File table size.".into());
        }
        let dest = installed.join(row.path);
        fs::create_dir_all(dest.parent().ok_or("Missing MSI file parent directory.")?)?;
        fs::rename(src, dest)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cabinet_ids_restore_only_the_declared_install_paths() -> Result<()> {
        let root = tempfile::tempdir()?;
        let cabinet = root.path().join("File");
        let installed = root.path().join("installed");
        fs::create_dir(&cabinet)?;
        fs::create_dir(&installed)?;
        fs::write(cabinet.join("Path"), b"binary")?;
        let member = |id: &str, path: &str, bytes| MsiFile {
            id: id.into(),
            path: path.into(),
            bytes,
        };
        assert!(
            restore_msi_files(&cabinet, &installed, vec![member("Path", "../escape", 6)]).is_err()
        );
        assert!(
            restore_msi_files(
                &cabinet,
                &installed,
                vec![member("Path", "runtimes/exe", 5)]
            )
            .is_err()
        );
        restore_msi_files(
            &cabinet,
            &installed,
            vec![member("Path", "runtimes/exe", 6)],
        )?;
        assert_eq!(fs::read(installed.join("runtimes/exe"))?, b"binary");
        Ok(())
    }
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
        let temp = self.stage.path().join("wix-temp");
        fs::create_dir(&temp)?;
        // Tauri copies renamed sidecars into TEMP while constructing the MSI.
        self.run.set("TEMP", &temp);
        self.run.set("TMP", &temp);
        self.run.set("NVSF_SIGNING_ARCH", &self.target.arch);
        for (key, path) in [
            ("NVSF_SIGNING_ROOT", self.root.clone()),
            ("NVSF_SIGNING_MAIN", self.main.clone()),
            ("NVSF_SIGNING_PAYLOAD", self.payload.clone()),
            ("NVSF_SIGNING_RELEASE", self.release.clone()),
            ("NVSF_SIGNING_LEDGER", ledger),
        ] {
            self.run.set(key, path);
        }
        Ok(
            json!({"signCommand":{"cmd":path_text(&xtask)?.replace('\\', "/"),"args":["ci-windows-signing","--path","%1"]}}),
        )
    }

    pub(super) fn inspect_msi(
        &mut self,
        setup: &Path,
        inspect: &Path,
        payload: &[Member],
    ) -> Result<()> {
        let signature = ci_authenticode::verify(setup, SIGNER_SHA256)?;
        if ci_authenticode::verify(setup, &"0".repeat(64)).is_ok() {
            return Err("MSI signature accepted the wrong signer pin.".into());
        }
        let dark = helpers::owned(
            &self.root,
            &self.stage.path().join("cargo/.tauri/WixTools314/dark.exe"),
        )?;
        let tables = inspect.join("package.wixout");
        self.command(
            "msi-content",
            dark,
            [
                "-nologo",
                "-xo",
                "-x",
                path_text(inspect)?,
                path_text(setup)?,
                path_text(&tables)?,
            ],
            (180, 90),
            false,
            None,
        )?;
        let metadata = unlinked(&tables)?;
        if !metadata.is_file() || metadata.len() > 4 * 1024 * 1024 {
            return Err("WiX installer table export exceeds its byte budget.".into());
        }
        let rows = self.read_msi_tables(&tables)?;
        let arch = if self.target.arch == "arm64" {
            "Arm64"
        } else {
            "x64"
        };
        let (platform, language) = rows
            .template
            .split_once(';')
            .ok_or("Missing MSI platform and language summary.")?;
        if !platform.eq_ignore_ascii_case(arch)
            || !matches!(language, "0" | "1033")
            || rows.language != "1033"
        {
            return Err(format!(
                "Unexpected MSI platform or language: {} / {}",
                rows.template, rows.language
            )
            .into());
        }
        let cabinet = inspect.join("File");
        let bin = inspect.join("installed");
        fs::create_dir(&bin)?;
        restore_msi_files(&cabinet, &bin, rows.files)?;
        self.installed(&bin, &bin, "msi", payload)?;
        let gui = bin.join("numera-video-sub-finder.exe");
        let cli = bin.join("nvsf.exe");
        self.icon(&gui, "msi-gui")?;
        self.icon(&cli, "msi-cli")?;
        if rows.product_icon != "ProductIcon" || rows.shortcut_icon != "ProductIcon" {
            return Err("MSI product and Start menu shortcut must use the approved icon.".into());
        }
        let icon = helpers::owned(&self.root, &inspect.join("Icon/ProductIcon"))?;
        let approved = self.desktop.join("icons/icon.ico");
        let metadata = unlinked(&icon)?;
        if !metadata.is_file() || metadata.len() > 8 * 1024 * 1024 {
            return Err("MSI ProductIcon exceeds its byte budget.".into());
        }
        let bytes = fs::read(&icon)?;
        crate::ci_icons::validate_ico(&bytes)?;
        if bytes != fs::read(approved)? {
            return Err("MSI ProductIcon differs from the approved icon bytes.".into());
        }
        self.icons.push("msi-product-icon".to_owned());
        for (role, file) in [("gui", gui.as_path()), ("msi", setup)] {
            let member = Member::read(file, &self.root)?;
            let name = basename(file)?;
            let mut count = 0;
            for record in entries(&self.stage.path().join("signing-ledger"))? {
                let record = json(&record)?;
                if record["role"] == role {
                    if record["file"] != name
                        || record["bytes"] != member.bytes
                        || record["sha256"] != member.sha256
                    {
                        return Err(
                            format!("Signer ledger does not bind the MSI {role} bytes.").into()
                        );
                    }
                    count += 1;
                }
            }
            if count != 1 {
                return Err(format!("Custom signer did not observe exactly one {role}.").into());
            }
        }
        self.signatures.extend([
            json!({"role":"msi-gui","evidence":ci_authenticode::verify(&gui,SIGNER_SHA256)?}),
            json!({"role":"msi-cli","evidence":ci_authenticode::verify(&cli,SIGNER_SHA256)?}),
            json!({"role":"msi-installer","evidence":signature}),
        ]);
        Ok(())
    }

    fn read_msi_tables(&mut self, tables: &Path) -> Result<MsiInventory> {
        let encoded = STANDARD.encode(
            MSI_TABLES
                .encode_utf16()
                .flat_map(u16::to_le_bytes)
                .collect::<Vec<_>>(),
        );
        self.run.set("NVSF_MSI_TABLES", tables);
        let output = self.command(
            "msi-tables",
            "pwsh.exe",
            [
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-EncodedCommand",
                &encoded,
            ],
            (30, 15),
            true,
            None,
        )?;
        Ok(serde_json::from_str(&output)?)
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
