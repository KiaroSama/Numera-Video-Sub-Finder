use super::{
    Result, SIGNER_SHA256, SIGNER_THUMBPRINT, embedded, read_image, require_sha256_indirect,
    valid_pin,
};
use crate::{ci_guard::unlinked, ci_tasks::sha256_hex};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    env,
    ffi::{OsString, c_void},
    fs,
    io::Write,
    mem::size_of,
    os::windows::{ffi::OsStrExt, io::AsRawHandle},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    ptr::{null, null_mut},
};

const PFX: &str = "NVSF_WINDOWS_CODESIGN_PFX";
const PASSWORD: &str = "NVSF_WINDOWS_CODESIGN_PASSWORD";

fn admitted() -> Result<()> {
    if env::var("GITHUB_ACTIONS").as_deref() != Ok("true") {
        return Err(
            "Development Authenticode verification requires Windows GitHub Actions.".into(),
        );
    }
    Ok(())
}

#[repr(C)]
struct Guid(u32, u16, u16, [u8; 8]);
#[repr(C)]
struct TrustFile {
    size: u32,
    path: *const u16,
    handle: *mut c_void,
    subject: *const Guid,
}
#[repr(C)]
struct SignatureSettings {
    size: u32,
    index: u32,
    flags: u32,
    secondary: u32,
    verified: u32,
    policy: *const c_void,
}
#[repr(C)]
struct TrustData {
    size: u32,
    policy_data: *const c_void,
    sip_data: *const c_void,
    ui: u32,
    revocation: u32,
    choice: u32,
    file: *const TrustFile,
    action: u32,
    state: *mut c_void,
    url: *const u16,
    flags: u32,
    context: u32,
    signatures: *mut SignatureSettings,
}
#[link(name = "wintrust")]
unsafe extern "system" {
    fn WinVerifyTrust(window: *mut c_void, action: *const Guid, data: *mut TrustData) -> i32;
}
#[link(name = "crypt32")]
unsafe extern "system" {
    fn CryptQueryObject(
        kind: u32,
        object: *const c_void,
        content: u32,
        format: u32,
        flags: u32,
        encoding: *mut u32,
        actual_content: *mut u32,
        actual_format: *mut u32,
        store: *mut *mut c_void,
        msg: *mut *mut c_void,
        context: *mut *const c_void,
    ) -> i32;
    fn CryptMsgGetParam(
        msg: *mut c_void,
        parameter: u32,
        index: u32,
        data: *mut c_void,
        size: *mut u32,
    ) -> i32;
    fn CryptMsgClose(msg: *mut c_void) -> i32;
    fn CertCloseStore(store: *mut c_void, flags: u32) -> i32;
}

fn wide(path: &Path) -> Result<Vec<u16>> {
    let mut units: Vec<_> = path.as_os_str().encode_wide().collect();
    if units.contains(&0) {
        return Err("Invalid Windows signature path.".into());
    }
    units.push(0);
    Ok(units)
}

fn trust(path: &[u16], file: &fs::File, hash_only: bool) -> Result<(i32, u32, u32)> {
    if (
        size_of::<TrustFile>(),
        size_of::<TrustData>(),
        size_of::<SignatureSettings>(),
    ) != (32, 88, 32)
    {
        return Err("Unsupported Windows verification ABI.".into());
    }
    let action = Guid(
        0x00aa_c56b,
        0xcd44,
        0x11d0,
        [0x8c, 0xc2, 0, 0xc0, 0x4f, 0xc2, 0x95, 0xee],
    );
    let info = TrustFile {
        size: 32,
        path: path.as_ptr(),
        handle: file.as_raw_handle(),
        subject: null(),
    };
    let mut signatures = SignatureSettings {
        size: 32,
        index: 0,
        flags: 3,
        secondary: 0,
        verified: 0,
        policy: null(),
    };
    let mut data = TrustData {
        size: 88,
        policy_data: null(),
        sip_data: null(),
        ui: 2,
        revocation: 0,
        choice: 1,
        file: &info,
        action: 1,
        state: null_mut(),
        url: null(),
        flags: 0x3000 | if hash_only { 0x200 } else { 0 },
        context: 0,
        signatures: &mut signatures,
    };
    let no_window = usize::MAX as *mut c_void;
    // File and signature settings stay live for both the verify and close calls.
    let status = unsafe { WinVerifyTrust(no_window, &action, &mut data) };
    let result = (status, signatures.secondary, signatures.verified);
    data.action = 2;
    if unsafe { WinVerifyTrust(no_window, &action, &mut data) } != 0 {
        return Err("WinTrust state cleanup failed.".into());
    }
    Ok(result)
}

fn queried_cms(path: &[u16]) -> Result<Vec<u8>> {
    let (mut store, mut msg) = (null_mut(), null_mut());
    let (mut encoding, mut content, mut format) = (0, 0, 0);
    // Ask for a single embedded binary message; catalog and PFX objects are excluded.
    let queried = unsafe {
        CryptQueryObject(
            1,
            path.as_ptr().cast(),
            1 << 10,
            1 << 1,
            0,
            &mut encoding,
            &mut content,
            &mut format,
            &mut store,
            &mut msg,
            null_mut(),
        )
    };
    let result = (|| {
        if queried == 0 || content != 10 || format != 1 || msg.is_null() {
            return Err("Embedded Authenticode query failed.".into());
        }
        let mut count = 0_u32;
        let mut size = 4;
        // The count buffer is a DWORD; every other read is bounded by a size probe.
        if unsafe { CryptMsgGetParam(msg, 5, 0, (&raw mut count).cast(), &mut size) } == 0
            || size != 4
            || count != 1
        {
            return Err("Exactly one CMS signer is required.".into());
        }
        size = 0;
        if unsafe { CryptMsgGetParam(msg, 29, 0, null_mut(), &mut size) } == 0
            || !(1..=1_048_576).contains(&size)
        {
            return Err("Encoded CMS exceeds its byte bound.".into());
        }
        let mut blob = vec![0; size as usize];
        if unsafe { CryptMsgGetParam(msg, 29, 0, blob.as_mut_ptr().cast(), &mut size) } == 0
            || size as usize != blob.len()
        {
            return Err("Encoded CMS read failed.".into());
        }
        Ok(blob)
    })();
    // Both handles are independent and must be closed even when validation fails.
    let message_closed = msg.is_null() || unsafe { CryptMsgClose(msg) } != 0;
    let store_closed = store.is_null() || unsafe { CertCloseStore(store, 0) } != 0;
    if !message_closed || !store_closed {
        return Err("Embedded message cleanup failed.".into());
    }
    result
}

const CMS: &str = r#"
Add-Type -AssemblyName System.Security.Cryptography.Pkcs
$cms = [Security.Cryptography.Pkcs.SignedCms]::new()
$cms.Decode([Convert]::FromBase64String([Console]::ReadLine()))
$cms.CheckSignature($true)
$s = $cms.SignerInfos[0]
$c = $s.Certificate
try {
    $eku = @($c.Extensions | Where-Object { $_.Oid.Value -ceq '2.5.29.37' })
    $usage = @($c.Extensions | Where-Object { $_.Oid.Value -ceq '2.5.29.15' })
    $basic = @($c.Extensions | Where-Object { $_.Oid.Value -ceq '2.5.29.19' })
    $ekus = @(); $usages = 0; $ca = $true
    if ($eku.Count -eq 1) { $ekus = @([Security.Cryptography.X509Certificates.X509EnhancedKeyUsageExtension]::new($eku[0], $eku[0].Critical).EnhancedKeyUsages | ForEach-Object { $_.Value }) }
    if ($usage.Count -eq 1) { $usages = [int][Security.Cryptography.X509Certificates.X509KeyUsageExtension]::new($usage[0], $usage[0].Critical).KeyUsages }
    if ($basic.Count -eq 1) { $ca = [Security.Cryptography.X509Certificates.X509BasicConstraintsExtension]::new($basic[0], $basic[0].Critical).CertificateAuthority }
    [Console]::WriteLine((@{detached=$cms.Detached; count=$cms.SignerInfos.Count; content_type=$cms.ContentInfo.ContentType.Value; content=[Convert]::ToBase64String($cms.ContentInfo.Content); counters=$s.CounterSignerInfos.Count; attributes=$s.UnsignedAttributes.Count; digest=$s.DigestAlgorithm.Value; certificate=[Convert]::ToBase64String($c.RawData); not_before=([DateTimeOffset]$c.NotBefore).ToUnixTimeSeconds(); not_after=([DateTimeOffset]$c.NotAfter).ToUnixTimeSeconds(); ekus=@($ekus); usage=$usages; ca=$ca} | ConvertTo-Json -Compress))
} finally { if ($null -ne $c) { $c.Dispose() } }
"#;

#[derive(Deserialize)]
struct Cms {
    detached: bool,
    count: usize,
    content_type: String,
    content: String,
    counters: usize,
    attributes: usize,
    digest: String,
    certificate: String,
    not_before: i64,
    not_after: i64,
    ekus: Vec<String>,
    usage: u32,
    ca: bool,
}

fn powershell(script: &str, input: &str, pfx: Option<(&OsString, &OsString)>) -> Result<Vec<u8>> {
    let mut command = Command::new("pwsh");
    command
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            &format!("$ErrorActionPreference='Stop'; Set-StrictMode -Version Latest; {script}"),
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .env_remove(PFX)
        .env_remove(PASSWORD);
    if let Some((pfx, password)) = pfx {
        command.env(PFX, pfx).env(PASSWORD, password);
    }
    let mut child = command.spawn()?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or("Missing Windows cryptography input pipe.")?;
    let sent = writeln!(stdin, "{input}");
    drop(stdin);
    let output = child.wait_with_output()?;
    if !output.status.success() || output.stdout.len() > 4 * 1024 * 1024 {
        return Err("Windows cryptography operation failed.".into());
    }
    sent?;
    Ok(output.stdout)
}

fn hash(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

pub(crate) fn verify(path: &Path, pin: &str) -> Result<Value> {
    admitted()?;
    if !valid_pin(pin) {
        return Err("Invalid development signer SHA256 pin.".into());
    }
    let resolved = std::path::absolute(path)?;
    let (file, bytes) = read_image(&resolved)?;
    let path = wide(&resolved)?;
    let embedded = embedded(&bytes)?;
    if Sha256::digest(embedded) != Sha256::digest(queried_cms(&path)?) {
        return Err("Native CMS query differs from the exact PE signature blob.".into());
    }
    let cms: Cms = serde_json::from_slice(&powershell(CMS, &STANDARD.encode(embedded), None)?)?;
    if cms.detached || cms.count != 1 || cms.content_type != "1.3.6.1.4.1.311.2.1.4" {
        return Err("Exactly one embedded Authenticode signer is required.".into());
    }
    if cms.counters != 0 || cms.attributes != 0 || cms.digest != "2.16.840.1.101.3.4.2.1" {
        return Err("Only an untimestamped single SHA256 signature is permitted.".into());
    }
    require_sha256_indirect(&STANDARD.decode(cms.content)?)?;
    let actual = hash(&STANDARD.decode(cms.certificate)?);
    if !actual.eq_ignore_ascii_case(pin) {
        return Err("Development signer pin differs.".into());
    }
    let now = chrono::Utc::now().timestamp();
    if !(cms.not_before..=cms.not_after).contains(&now) {
        return Err("Development signer certificate is outside its validity period.".into());
    }
    if !cms.ekus.iter().any(|oid| oid == "1.3.6.1.5.5.7.3.3") || cms.usage & 0x80 == 0 || cms.ca {
        return Err("Certificate is not a non-CA code-signing identity.".into());
    }
    if trust(&path, &file, true)? != (0, 0, 0) {
        return Err("Native Authenticode file-digest verification failed.".into());
    }
    let (status, secondary, _) = trust(&path, &file, false)?;
    if status as u32 != 0x800b_0109 || secondary != 0 {
        return Err("Expected ordinary untrusted-root policy result was not observed.".into());
    }
    Ok(
        json!({"schema":1,"policy":"development-pinned-integrity","certificate_sha256":actual,"sha256":hash(&bytes),"bytes":bytes.len(),"cms_signature_valid":true,"native_hash_status":"0x00000000","ordinary_trust_status":"0x800b0109","publicly_trusted":false,"timestamp":"absent","revocation":"NOT_CHECKED_OFFLINE"}),
    )
}

const PROBE: &str = r#"
$bytes = [Convert]::FromBase64String($env:NVSF_WINDOWS_CODESIGN_PFX)
$pass = $env:NVSF_WINDOWS_CODESIGN_PASSWORD
try {
    $cert = [Security.Cryptography.X509Certificates.X509Certificate2]::new($bytes,$pass,'EphemeralKeySet')
    try { [Console]::WriteLine((@{der=[Convert]::ToBase64String($cert.RawData); thumbprint=$cert.Thumbprint; private_key=$cert.HasPrivateKey} | ConvertTo-Json -Compress)) } finally { $cert.Dispose() }
} finally { [Array]::Clear($bytes,0,$bytes.Length) }
"#;

const IMPORT: &str = r#"
$bytes = [Convert]::FromBase64String($env:NVSF_WINDOWS_CODESIGN_PFX)
$pass = $env:NVSF_WINDOWS_CODESIGN_PASSWORD
try { $cert = [Security.Cryptography.X509Certificates.X509Certificate2]::new($bytes,$pass,'UserKeySet, PersistKeySet') }
finally { [Array]::Clear($bytes,0,$bytes.Length) }
try {
    $store = [Security.Cryptography.X509Certificates.X509Store]::new('My','CurrentUser')
    try { $store.Open('ReadWrite'); $store.Add($cert) } finally { $store.Dispose() }
} catch {
    $key = [Security.Cryptography.X509Certificates.RSACertificateExtensions]::GetRSAPrivateKey($cert)
    if ($null -ne $key) { try { $key.Key.Delete() } finally { $key.Dispose() } }
    throw
} finally { $cert.Dispose() }
"#;

const REMOVE: &str = r#"
$thumbprint = [Console]::ReadLine()
$item = 'Cert:\CurrentUser\My\' + $thumbprint
if (Test-Path -LiteralPath $item) { Remove-Item -LiteralPath $item -DeleteKey -ErrorAction Stop }
[Console]::WriteLine((Test-Path -LiteralPath $item))
"#;

#[derive(Deserialize)]
struct Probe {
    der: String,
    thumbprint: String,
    private_key: bool,
}

pub(crate) fn enable() -> Result<()> {
    admitted()?;
    let encoded = env::var_os(PFX);
    let password = env::var_os(PASSWORD);
    // Environment mutation is supported on Windows; no later child inherits the original inputs.
    unsafe {
        env::remove_var(PFX);
        env::remove_var(PASSWORD);
    }
    let (Some(encoded), Some(password)) = (
        encoded.filter(|v| !v.is_empty()),
        password.filter(|v| !v.is_empty()),
    ) else {
        return Err("Windows development signing inputs are missing; unsigned final Windows output is not permitted.".into());
    };
    if encoded.len() > 65_536 {
        return Err("Signing identity input exceeds its bound.".into());
    }
    let credentials = Some((&encoded, &password));
    let probe: Probe = serde_json::from_slice(&powershell(PROBE, "", credentials)?)?;
    if hash(&STANDARD.decode(probe.der)?) != SIGNER_SHA256
        || probe.thumbprint != SIGNER_THUMBPRINT
        || !probe.private_key
    {
        return Err("Selected signing identity differs from the approved development pin.".into());
    }
    powershell(IMPORT, "", credentials)?;
    Ok(())
}

pub(crate) fn disable() -> Result<()> {
    admitted()?;
    let removed = powershell(REMOVE, SIGNER_THUMBPRINT, None)?;
    if !std::str::from_utf8(&removed)?
        .trim()
        .eq_ignore_ascii_case("false")
    {
        return Err("Development signing identity cleanup failed.".into());
    }
    Ok(())
}

fn signtool() -> Result<PathBuf> {
    let kits =
        PathBuf::from(env::var_os("ProgramFiles(x86)").ok_or("Windows SDK root is unavailable.")?)
            .join("Windows Kits")
            .join("10")
            .join("bin");
    let mut versions = Vec::new();
    for entry in fs::read_dir(kits)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        let Ok(parts) = name
            .split('.')
            .map(str::parse::<u32>)
            .collect::<std::result::Result<Vec<_>, _>>()
        else {
            continue;
        };
        if parts.len() == 4 {
            versions.push((parts, entry.path()));
        }
    }
    versions.sort_unstable_by(|a, b| b.0.cmp(&a.0));
    let paths = if cfg!(target_arch = "aarch64") {
        ["arm64", "x64"].as_slice()
    } else {
        ["x64"].as_slice()
    };
    for (_, directory) in versions {
        for arch in paths {
            let tool = directory.join(arch).join("signtool.exe");
            if tool.is_file() {
                return Ok(tool);
            }
        }
    }
    Err("Windows SDK SignTool is unavailable on this hosted runner.".into())
}

pub(crate) fn sign(path: &Path) -> Result<Value> {
    admitted()?;
    if !unlinked(path)?.is_file() {
        return Err("A regular signature input is required.".into());
    }
    let output = Command::new(signtool()?)
        .args([
            "sign",
            "/fd",
            "SHA256",
            "/sha1",
            SIGNER_THUMBPRINT,
            "/s",
            "My",
            "/d",
            "Numera Video Sub Finder",
        ])
        .arg(std::path::absolute(path)?)
        .env_remove(PFX)
        .env_remove(PASSWORD)
        .output()?;
    if !output.status.success() {
        return Err(format!(
            "SignTool failed exit={} output={}{}",
            output.status.code().unwrap_or(1),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    verify(path, SIGNER_SHA256)
}

fn under(path: &Path, folder: &Path) -> bool {
    let mut child = path.components();
    for component in folder.components() {
        if !child.next().is_some_and(|part| {
            part.as_os_str()
                .to_string_lossy()
                .eq_ignore_ascii_case(&component.as_os_str().to_string_lossy())
        }) {
            return false;
        }
    }
    child.next().is_some()
}

pub(super) fn sign_allowlisted(path: &Path) -> Result<()> {
    admitted()?;
    let get = |name: &str| -> Result<PathBuf> {
        env::var_os(name)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .ok_or_else(|| format!("Signing context is incomplete: {name}").into())
    };
    let root = fs::canonicalize(get("NVSF_SIGNING_ROOT")?)?;
    let main = fs::canonicalize(get("NVSF_SIGNING_MAIN")?)?;
    let payload = fs::canonicalize(get("NVSF_SIGNING_PAYLOAD")?)?;
    let release = fs::canonicalize(get("NVSF_SIGNING_RELEASE")?)?;
    let temp = fs::canonicalize(get("NVSF_SIGNING_TEMP")?)?;
    let ledger = get("NVSF_SIGNING_LEDGER")?;
    let xtask = get("NVSF_SIGNING_XTASK")?;
    let icon = get("NVSF_SIGNING_ICON")?;
    if !unlinked(path)?.is_file() {
        return Err("Signing input must be a regular file inside the owning checkout.".into());
    }
    let path = fs::canonicalize(path)?;
    if !under(&path, &root) {
        return Err("Signing input must be a regular file inside the owning checkout.".into());
    }
    let file = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or("Invalid signing filename.")?;
    let role = if path
        .to_string_lossy()
        .eq_ignore_ascii_case(&main.to_string_lossy())
    {
        "gui"
    } else if verify(&path, SIGNER_SHA256).is_ok() {
        "already-pinned"
    } else if under(&path, &payload) || under(&path, &release.join("nsis")) {
        "preserved-vendor"
    } else if under(&path, &release.join("bundle/nsis"))
        && file.to_ascii_lowercase().ends_with("-setup.exe")
    {
        "setup"
    } else if under(&path, &temp) {
        "uninstaller"
    } else {
        return Err(format!("Unexpected signing request outside the allowlist: {file}").into());
    };
    if matches!(role, "gui" | "setup" | "uninstaller") {
        sign(&path)?;
    }
    if role == "uninstaller" {
        let scratch = tempfile::Builder::new()
            .prefix("uninstaller-icons-")
            .tempdir_in(&ledger)?;
        let name = scratch
            .path()
            .file_name()
            .ok_or("Missing uninstaller icon evidence identity.")?;
        let record = scratch.path().join("icons.json");
        let status = Command::new(xtask)
            .args(["executable-icons", "--binary"])
            .arg(&path)
            .arg("--icon")
            .arg(icon)
            .arg("--output")
            .arg(&record)
            .env_remove(PFX)
            .env_remove(PASSWORD)
            .status()?;
        if !status.success() {
            return Err("Uninstaller icon resources differ from the approved logo.".into());
        }
        fs::rename(record, ledger.join(name).with_extension(".json"))?;
        scratch.close()?;
    }
    let (size, digest) = sha256_hex(&path)?;
    let mut record = tempfile::Builder::new()
        .prefix(&format!("{role}-"))
        .suffix(".json")
        .tempfile_in(ledger)?;
    serde_json::to_writer_pretty(
        &mut record,
        &json!({"schema":1,"role":role,"file":file,"bytes":size,"sha256":digest}),
    )?;
    let _ = record.keep()?;
    println!("development-signing role={role} file={file}");
    Ok(())
}
