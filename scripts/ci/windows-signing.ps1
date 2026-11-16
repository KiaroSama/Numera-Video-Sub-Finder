#requires -Version 7.4
# Windows development Authenticode for hosted Actions bundling (D014).
# Dot-sourced by bundle.ps1 for identity setup; invoked with -Path as the Tauri/NSIS custom sign command.
# Self-signed development identity only: no trust-store import, no timestamp service, not public publisher trust.
[CmdletBinding()]
param([string]$Path)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'authenticode.ps1')

# Public, nonsecret pin of the approved development identity (DER SHA256) and its store selector.
$script:NvsfSignerSha256 = 'fd066bb47fb069723b76650fe628cf4726b6f710b8245097d055312a8a912317'
$script:NvsfSignerThumbprint = 'B2A45C4E8257197BBF067ED94F83E626AC6CE1C5'

function Get-SignToolPath {
    $kits = Join-Path ${env:ProgramFiles(x86)} 'Windows Kits/10/bin'
    $arch = if ([Runtime.InteropServices.RuntimeInformation]::ProcessArchitecture -eq 'Arm64') { 'arm64' } else { 'x64' }
    $versions = @(Get-ChildItem -LiteralPath $kits -Directory | Where-Object Name -Match '^\d+\.\d+\.\d+\.\d+$' | Sort-Object { [version]$_.Name } -Descending)
    foreach ($version in $versions) {
        foreach ($candidate in @($arch, 'x64')) {
            $tool = Join-Path $version.FullName "$candidate/signtool.exe"
            if ([IO.File]::Exists($tool)) { return $tool }
        }
    }
    throw 'Windows SDK SignTool is unavailable on this hosted runner.'
}

function Enable-DevelopmentSigningIdentity {
    # Imports the selected secret identity into the ephemeral runner CurrentUser\My store only.
    $encoded = $env:NVSF_WINDOWS_CODESIGN_PFX
    $password = $env:NVSF_WINDOWS_CODESIGN_PASSWORD
    # Remove secret inputs from this process before any child process can inherit them.
    $env:NVSF_WINDOWS_CODESIGN_PFX = $null
    $env:NVSF_WINDOWS_CODESIGN_PASSWORD = $null
    if ([string]::IsNullOrEmpty($encoded) -or [string]::IsNullOrEmpty($password)) {
        throw 'Windows development signing inputs are missing; unsigned final Windows output is not permitted.'
    }
    if ($encoded.Length -gt 65536) { throw 'Signing identity input exceeds its bound.' }
    $bytes = [Convert]::FromBase64String($encoded)
    try {
        # Validate the pin with an ephemeral key first so a wrong identity never persists key material.
        $probe = [Security.Cryptography.X509Certificates.X509Certificate2]::new($bytes, $password, [Security.Cryptography.X509Certificates.X509KeyStorageFlags]::EphemeralKeySet)
        try {
            $pin = [Convert]::ToHexString([Security.Cryptography.SHA256]::HashData($probe.RawData)).ToLowerInvariant()
            if ($pin -cne $script:NvsfSignerSha256 -or $probe.Thumbprint -cne $script:NvsfSignerThumbprint -or -not $probe.HasPrivateKey) {
                throw 'Selected signing identity differs from the approved development pin.'
            }
        } finally { $probe.Dispose() }
        $flags = [Security.Cryptography.X509Certificates.X509KeyStorageFlags]'UserKeySet, PersistKeySet'
        $cert = [Security.Cryptography.X509Certificates.X509Certificate2]::new($bytes, $password, $flags)
    } finally {
        [Array]::Clear($bytes, 0, $bytes.Length)
        $password = $null
    }
    $added = $false
    try {
        $store = [Security.Cryptography.X509Certificates.X509Store]::new('My', 'CurrentUser')
        try { $store.Open('ReadWrite'); $store.Add($cert); $added = $true } finally { $store.Dispose() }
    } catch {
        # Store add failed: delete the persisted private key exactly; never leave orphaned key material.
        if (-not $added) {
            try { [Security.Cryptography.X509Certificates.RSACertificateExtensions]::GetRSAPrivateKey($cert).Key.Delete() } catch { }
        }
        throw
    } finally { $cert.Dispose() }
    return $script:NvsfSignerThumbprint
}

function Disable-DevelopmentSigningIdentity {
    $item = "Cert:\CurrentUser\My\$($script:NvsfSignerThumbprint)"
    if (Test-Path -LiteralPath $item) { Remove-Item -LiteralPath $item -DeleteKey -ErrorAction Stop }
    if (Test-Path -LiteralPath $item) { throw 'Development signing identity cleanup failed.' }
}

function Test-PinnedSignature([string]$File) {
    try { Assert-DevelopmentSignature -Path $File -CertificateSha256 $script:NvsfSignerSha256 | Out-Null; return $true }
    catch { return $false }
}

function Invoke-DevelopmentSign([string]$File) {
    $tool = Get-SignToolPath
    # Thumbprint selects the imported key; no password or private material reaches argv.
    $output = & $tool sign /fd SHA256 /sha1 $script:NvsfSignerThumbprint /s My /d 'Numera Video Sub Finder' $File 2>&1
    $code = $LASTEXITCODE
    if ($code -ne 0) { throw "SignTool failed exit=$code output=$(($output | Out-String).Trim())" }
    return Assert-DevelopmentSignature -Path $File -CertificateSha256 $script:NvsfSignerSha256
}

function Test-UnderDirectory([string]$Child, [string]$Parent) {
    $prefix = [IO.Path]::TrimEndingDirectorySeparator([IO.Path]::GetFullPath($Parent)) + [IO.Path]::DirectorySeparatorChar
    return $Child.StartsWith($prefix, [StringComparison]::OrdinalIgnoreCase)
}

function Invoke-AllowlistedSign([string]$Target) {
    foreach ($name in @('NVSF_SIGNING_ROOT', 'NVSF_SIGNING_MAIN', 'NVSF_SIGNING_PAYLOAD', 'NVSF_SIGNING_RELEASE', 'NVSF_SIGNING_TEMP', 'NVSF_SIGNING_LEDGER', 'NVSF_SIGNING_XTASK', 'NVSF_SIGNING_ICON')) {
        if ([string]::IsNullOrEmpty([Environment]::GetEnvironmentVariable($name))) { throw "Signing context is incomplete: $name" }
    }
    $full = [IO.Path]::GetFullPath($Target)
    $item = Get-Item -LiteralPath $full -Force
    if ($item.PSIsContainer -or ($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -or -not (Test-UnderDirectory $full $env:NVSF_SIGNING_ROOT)) {
        throw 'Signing input must be a regular file inside the owning checkout.'
    }
    $release = $env:NVSF_SIGNING_RELEASE
    $role = if ($full -ieq [IO.Path]::GetFullPath($env:NVSF_SIGNING_MAIN)) { 'gui' }
        elseif (Test-PinnedSignature $full) { 'already-pinned' }
        elseif ((Test-UnderDirectory $full $env:NVSF_SIGNING_PAYLOAD) -or (Test-UnderDirectory $full (Join-Path $release 'nsis'))) { 'preserved-vendor' }
        elseif ((Test-UnderDirectory $full (Join-Path $release 'bundle/nsis')) -and $item.Name -like '*-setup.exe') { 'setup' }
        elseif (Test-UnderDirectory $full $env:NVSF_SIGNING_TEMP) { 'uninstaller' }
        else { throw "Unexpected signing request outside the allowlist: $($item.Name)" }
    # Vendor and already-pinned members are left byte-identical; only owned final PEs are signed.
    if ($role -in @('gui', 'setup', 'uninstaller')) {
        Invoke-DevelopmentSign $full | Out-Null
        if ($role -eq 'uninstaller') {
            $iconRecord = Join-Path $env:NVSF_SIGNING_LEDGER "uninstaller-icons-$([Guid]::NewGuid().ToString('N')).json"
            & $env:NVSF_SIGNING_XTASK executable-icons --binary $full --icon $env:NVSF_SIGNING_ICON --output $iconRecord
            if ($LASTEXITCODE -ne 0) { throw 'Uninstaller icon resources differ from the approved logo.' }
        }
    }
    $record = [ordered]@{ schema = 1; role = $role; file = $item.Name; bytes = ([IO.FileInfo]::new($full)).Length; sha256 = (Get-FileHash -LiteralPath $full -Algorithm SHA256).Hash.ToLowerInvariant() }
    $ledger = Join-Path $env:NVSF_SIGNING_LEDGER "$role-$([Guid]::NewGuid().ToString('N')).json"
    [IO.File]::WriteAllText($ledger, ($record | ConvertTo-Json), [Text.UTF8Encoding]::new($false))
    [Console]::Out.WriteLine("development-signing role=$role file=$($item.Name)")
}

# Dot-sourcing from bundle.ps1 defines functions only; the signer entry runs only for direct -File invocation.
if ($MyInvocation.InvocationName -ne '.' -and $PSBoundParameters.ContainsKey('Path')) {
    Initialize-DevelopmentAuthenticode
    Invoke-AllowlistedSign $Path
    exit 0
}
