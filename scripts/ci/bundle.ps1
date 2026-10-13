#requires -Version 7.4
[CmdletBinding()]
param(
    [Parameter(Mandatory)][ValidatePattern('^(linux|windows|macos)-(x64|arm64)$')][string]$TargetId,
    [string]$PackagesRoot = '.ci-work/packages'
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
if ($env:GITHUB_ACTIONS -ne 'true' -or $env:RUNNER_ENVIRONMENT -ne 'github-hosted') {
    throw 'Desktop bundling is restricted to GitHub-hosted Actions runners.'
}
$root = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../..'))
$desktop = Join-Path $root 'crates/desktop'
$guard = Join-Path $PSScriptRoot 'guard.ps1'
$utf8 = [Text.UTF8Encoding]::new($false, $true)
$targets = [IO.File]::ReadAllText((Join-Path $root 'ci/targets.json'), $utf8) | ConvertFrom-Json
$matches = @($targets.targets | Where-Object id -EQ $TargetId)
if ($targets.scope -ne 'desktop-only' -or $matches.Count -ne 1) { throw 'Unknown desktop bundle target.' }
$target = $matches[0]
$os = if ($IsWindows) { 'windows' } elseif ($IsMacOS) { 'macos' } else { 'linux' }
$arch = [Runtime.InteropServices.RuntimeInformation]::ProcessArchitecture.ToString().ToLowerInvariant()
if ($target.os -ne $os -or $target.arch -ne $arch) { throw 'Bundling requires the matching native OS and process architecture.' }
if ($env:GITHUB_SHA -notmatch '^[a-f0-9]{40}$') { throw 'A real Actions source SHA is required.' }
$config = [IO.File]::ReadAllText((Join-Path $desktop 'tauri.conf.json'), $utf8) | ConvertFrom-Json
if ($config.version -notmatch '^\d+\.\d+\.\d+$' -or $config.mainBinaryName -ne 'numera-video-sub-finder') {
    throw 'Unsupported version or desktop executable contract.'
}
function Resolve-OwnedPath([string]$Path) {
    $full = [IO.Path]::GetFullPath($Path, $root)
    if (-not $full.StartsWith("$root$([IO.Path]::DirectorySeparatorChar)", [StringComparison]::OrdinalIgnoreCase)) {
        throw 'Bundle input/output must remain inside the owning checkout.'
    }
    $cursor = $full
    while ($cursor.Length -gt $root.Length) {
        if ((Test-Path -LiteralPath $cursor) -and ((Get-Item -LiteralPath $cursor -Force).Attributes -band [IO.FileAttributes]::ReparsePoint)) {
            throw 'Bundle input/output ancestors may not be symlinks or reparse points.'
        }
        $cursor = [IO.Path]::GetDirectoryName($cursor)
    }
    return $full
}
$packages = Resolve-OwnedPath $PackagesRoot
$baseName = "Numera-Video-Sub-Finder-$($config.version)-$TargetId"
$portable = Resolve-OwnedPath (Join-Path $packages $baseName)
$stage = Resolve-OwnedPath (Join-Path $root ".ci-work/bundle/$TargetId/$([Guid]::NewGuid().ToString('N'))")
$payload = Join-Path $stage 'payload'
$cargoDir = Join-Path $stage 'cargo'
$release = Join-Path $cargoDir 'release'
$output = Resolve-OwnedPath (Join-Path $packages "installers/$TargetId")
if (Test-Path -LiteralPath $output) { throw 'Bundle output must be a new target-specific directory.' }
$logs = Join-Path $root '.ci-work/logs'
foreach ($dir in @($payload, $release, $output, $logs)) { [IO.Directory]::CreateDirectory($dir) | Out-Null }
$logPath = Join-Path $logs "bundle_$([DateTime]::UtcNow.ToString('yyyy-MM-dd_HH-mm-ss'))_UTC.log"
if (Test-Path -LiteralPath $logPath) { $logPath = $logPath.Replace('.log', "_$([Guid]::NewGuid().ToString('N')).log") }
$writer = $null
try { $writer = [IO.StreamWriter]::new([IO.FileStream]::new($logPath, [IO.FileMode]::CreateNew), $utf8); $writer.AutoFlush = $true }
catch { [Console]::Error.WriteLine('Bundle file logging failed; refusing an unlogged packaging run.'); throw }
function Write-BundleLog([string]$Level, [string]$Text) {
    $line = '[{0}] [{1}] [DESKTOP-BUNDLE] {2}' -f [DateTime]::UtcNow.ToString('yyyy-MM-dd HH:mm:ss UTC'), $Level, $Text
    $writer.WriteLine($line)
    [Console]::Error.WriteLine($line)
}
function Invoke-BundleCommand {
    param([string]$Name, [string]$Executable, [string[]]$Arguments, [int]$Wall = 120, [int]$Idle = 60, [switch]$Capture, [string]$WorkingDirectory = $desktop)
    Write-BundleLog INFO "Starting command=$Name"
    $guardArgs = @{ Name = "bundle-$Name"; WallSeconds = $Wall; IdleSeconds = $Idle; FilePath = $Executable; Arguments = $Arguments; WorkingDirectory = $WorkingDirectory }
    $capturePath = Join-Path $stage "$Name.stdout"
    if ($Capture) { $guardArgs.CaptureStdoutPath = $capturePath }
    & $guard @guardArgs
    if ($LASTEXITCODE -ne 0) { throw "Bundle command failed: $Name" }
    if ($Capture) { return [IO.File]::ReadAllText($capturePath, $utf8) }
}
function Copy-BundleFile([string]$Source, [string]$Destination) {
    [IO.Directory]::CreateDirectory([IO.Path]::GetDirectoryName($Destination)) | Out-Null
    [IO.File]::Copy($Source, $Destination, $false)
}
function Assert-FileBytes([string]$Path, [string]$Sha, [long]$Bytes) {
    if (-not [IO.File]::Exists($Path) -or ([IO.FileInfo]::new($Path)).Length -ne $Bytes -or (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant() -ne $Sha) {
        throw 'Bundle payload byte inventory does not match the verified package.'
    }
}
function Get-Inventory([string]$Directory, [string]$RelativeTo) {
    foreach ($file in Get-ChildItem -LiteralPath $Directory -File -Recurse -Force | Sort-Object FullName) {
        if ($file.Attributes -band [IO.FileAttributes]::ReparsePoint) { throw 'Unexpected link in bundle output inventory.' }
        @{ file = [IO.Path]::GetRelativePath($RelativeTo, $file.FullName).Replace('\', '/'); bytes = $file.Length; sha256 = (Get-FileHash -LiteralPath $file.FullName -Algorithm SHA256).Hash.ToLowerInvariant() }
    }
}
function Assert-InstalledPayload([string]$ResourceDirectory, [string]$BinDirectory, [switch]$LoaderPatched) {
    foreach ($entry in $payloadInventory) {
        $path = Resolve-OwnedPath (Join-Path $ResourceDirectory $entry.file)
        if ($LoaderPatched -and $entry.file -match '^runtimes/.*\.so(\.|$)') { Assert-ElfArchitecture $path }
        else { Assert-FileBytes $path $entry.sha256 $entry.bytes }
    }
    foreach ($pair in @(@{ name = "nvsf$ext"; source = $sidecar }, @{ name = "numera-video-sub-finder$ext"; source = $mainBinary })) {
        $path = Join-Path $BinDirectory $pair.name
        if ($LoaderPatched) { Assert-ElfArchitecture $path }
        else { Assert-FileBytes $path (Get-FileHash -LiteralPath $pair.source).Hash.ToLowerInvariant() ([IO.FileInfo]::new($pair.source)).Length }
    }
    if ([IO.File]::Exists((Join-Path $BinDirectory 'portable.flag'))) { throw 'Installed bundles must not redirect writable data into installation resources.' }
}
function Assert-ElfArchitecture([string]$Path) {
    Assert-Header $Path '7F454C460201'
    $stream = [IO.File]::OpenRead($Path)
    $reader = [IO.BinaryReader]::new($stream)
    try {
        $stream.Position = 18
        if ($reader.ReadUInt16() -ne $(if ($target.arch -eq 'arm64') { 183 } else { 62 })) { throw 'Packaged ELF architecture differs from native target.' }
    } finally { $reader.Dispose() }
}
function Assert-Header([string]$Path, [string]$Hex, [int]$Offset = 0) {
    $stream = [IO.File]::OpenRead($Path)
    try {
        $stream.Position = $Offset
        $bytes = [byte[]]::new($Hex.Length / 2)
        $read = $stream.Read($bytes, 0, $bytes.Length)
        if ($read -ne $bytes.Length -or [Convert]::ToHexString($bytes) -ne $Hex) { throw 'Generated artifact has an invalid package header.' }
    } finally { $stream.Dispose() }
}
$savedEnv = @{}
foreach ($name in @('CARGO_TARGET_DIR', 'CARGO_BUILD_JOBS', 'RAYON_NUM_THREADS', 'OMP_NUM_THREADS', 'TAURI_SIGNING_RPM_KEY', 'TAURI_SIGNING_RPM_KEY_PASSPHRASE')) {
    $savedEnv[$name] = [Environment]::GetEnvironmentVariable($name, 'Process')
}
try {
    Write-BundleLog INFO "Start target=$TargetId source_sha=$env:GITHUB_SHA status=development-unqualified signing=unsigned"
    if (([IO.FileInfo]::new((Join-Path $portable 'manifest.json'))).Length -gt 8388608) { throw 'Portable package inventory exceeds its 8 MiB bound.' }
    $manifest = [IO.File]::ReadAllText((Join-Path $portable 'manifest.json'), $utf8) | ConvertFrom-Json
    if ($manifest.schema -ne 1 -or $manifest.target -ne $TargetId -or $manifest.source_sha -ne $env:GITHUB_SHA -or $manifest.status -ne 'development-unqualified' -or $manifest.files.Count -lt 2) {
        throw 'Bundle input requires the verified same-source portable package inventory.'
    }
    $known = [Collections.Generic.HashSet[string]]::new([StringComparer]::OrdinalIgnoreCase)
    foreach ($entry in $manifest.files) {
        $rel = [string]$entry.file
        if ($rel -match '(^|/)(\.|\.\.|\.[^/]+|secrets\.md|explain-AI\.md)(/|$)' -or $rel -match '[\\:]' -or $rel.StartsWith('/') -or $entry.sha256 -notmatch '^[a-f0-9]{64}$' -or -not $known.Add($rel)) {
            throw 'Unsafe or duplicate portable package inventory path.'
        }
        $path = Resolve-OwnedPath (Join-Path $portable $rel)
        if (-not $path.StartsWith("$portable$([IO.Path]::DirectorySeparatorChar)", [StringComparison]::OrdinalIgnoreCase)) { throw 'Portable inventory path escapes its package.' }
        Assert-FileBytes $path $entry.sha256 $entry.bytes
    }
    foreach ($item in Get-ChildItem -LiteralPath $portable -Recurse -Force) {
        $rel = [IO.Path]::GetRelativePath($portable, $item.FullName).Replace('\', '/')
        if ($item.Attributes -band [IO.FileAttributes]::ReparsePoint) { throw 'Portable payload contains an unverified link.' }
        if (-not $item.PSIsContainer -and $rel -ne 'manifest.json' -and -not $known.Contains($rel)) { throw 'Portable payload contains an uninventoried file.' }
    }
    $ext = if ($IsWindows) { '.exe' } else { '' }
    $cliName = "nvsf$ext"
    $guiName = if ($IsWindows) { 'Numera Video Sub Finder.exe' } else { 'numera-video-sub-finder' }
    foreach ($required in @($cliName, $guiName, 'LICENSE', 'THIRD_PARTY_NOTICES.md', 'README.md', 'portable.flag')) {
        if (-not $known.Contains($required)) { throw "Missing required portable package member: $required" }
    }
    if (-not @($manifest.files | Where-Object file -Like 'licenses/native/*').Count -or -not @($manifest.files | Where-Object file -Like 'data/models/*').Count) {
        throw 'Native notices and actual model payload are required for desktop bundles.'
    }
    $mainBinary = Join-Path $release "numera-video-sub-finder$ext"
    $sidecarBase = Join-Path $stage 'nvsf'
    $sidecar = "$sidecarBase-$($target.target)$ext"
    Copy-BundleFile (Join-Path $portable $guiName) $mainBinary
    Copy-BundleFile (Join-Path $portable $cliName) $sidecar
    $resources = [ordered]@{}
    foreach ($entry in $manifest.files) {
        $rel = [string]$entry.file
        if ($rel -match '^(runtimes/|data/models/|licenses/native/|licenses/data/)' -or $rel -in @('LICENSE', 'THIRD_PARTY_NOTICES.md', 'README.md') -or ($IsWindows -and $rel -match '^[^/]+\.dll$')) {
            $dest = Join-Path $payload $rel
            Copy-BundleFile (Join-Path $portable $rel) $dest
            $resources[$dest.Replace('\', '/')] = $rel
        } elseif ($rel -notin @($guiName, $cliName, 'portable.flag')) {
            throw "Unexpected portable file outside the bundle allowlist: $rel"
        }
    }
    $logo = Join-Path $payload 'assets/logo.png'
    Copy-BundleFile (Join-Path $root 'assets/logo.png') $logo
    $resources[$logo.Replace('\', '/')] = 'assets/logo.png'
    foreach ($icon in $config.bundle.icon) {
        if (-not [IO.File]::Exists((Join-Path $desktop $icon))) { throw "Missing official generated desktop icon: $icon" }
    }
    $env:CARGO_TARGET_DIR = $cargoDir
    $env:CARGO_BUILD_JOBS = '2'
    $env:RAYON_NUM_THREADS = '2'
    $env:OMP_NUM_THREADS = '2'
    foreach ($name in @('TAURI_SIGNING_RPM_KEY', 'TAURI_SIGNING_RPM_KEY_PASSPHRASE')) { [Environment]::SetEnvironmentVariable($name, $null, 'Process') }
    if ($IsWindows) {
        $vs = [Environment]::GetEnvironmentVariable('VCPKG_VISUAL_STUDIO_PATH', 'Process')
        if (-not $vs) { throw 'Native provisioning must export the discovered Visual Studio installation.' }
        $redist = Join-Path $vs 'VC/Redist/MSVC'
        $versions = @(Get-ChildItem -LiteralPath $redist -Directory | Where-Object Name -Match '^\d+\.\d+\.\d+$' | Sort-Object { [version]$_.Name } -Descending)
        if (-not $versions.Count) { throw 'Visual C++ redistributable version directory is missing.' }
        $crtDirs = @(Get-ChildItem -LiteralPath (Join-Path $versions[0].FullName $target.arch) -Directory -Filter 'Microsoft.VC*.CRT')
        if ($crtDirs.Count -ne 1) { throw 'Matching target Visual C++ CRT distribution directory is missing or ambiguous.' }
        $dlls = @(Get-ChildItem -LiteralPath $crtDirs[0].FullName -File -Filter '*.dll')
        if (-not $dlls.Count) { throw 'Visual C++ runtime DLL closure is empty.' }
        foreach ($dll in $dlls) {
            $stream = [IO.File]::OpenRead($dll.FullName)
            $reader = [IO.BinaryReader]::new($stream)
            try {
                if ($reader.ReadUInt16() -ne 0x5A4D) { throw 'Invalid redistributable PE image.' }
                $stream.Position = 0x3C; $pe = $reader.ReadInt32(); $stream.Position = $pe
                if ($reader.ReadUInt32() -ne 0x4550) { throw 'Invalid redistributable PE signature.' }
                $machine = $reader.ReadUInt16()
                if ($machine -ne $(if ($target.arch -eq 'arm64') { 0xAA64 } else { 0x8664 })) { throw 'Redistributable DLL architecture differs from target.' }
            } finally { $reader.Dispose() }
            $dest = Join-Path $payload $dll.Name
            if ([IO.File]::Exists($dest)) { throw 'Redistributable DLL name collides with native payload.' }
            Copy-BundleFile $dll.FullName $dest
            $resources[$dest.Replace('\', '/')] = $dll.Name
        }
        $notice = Join-Path $payload 'licenses/microsoft/visual-cpp-runtime.txt'
        [IO.Directory]::CreateDirectory([IO.Path]::GetDirectoryName($notice)) | Out-Null
        [IO.File]::WriteAllText($notice, "Microsoft Visual C++ Runtime DLLs from installed Visual Studio Redist/MSVC/$($versions[0].Name)/$($target.arch).`nRedistribution is subject to the applicable Microsoft Software License Terms and licensed Visual Studio user rights; this development artifact is not release-qualified.`nhttps://learn.microsoft.com/en-us/cpp/windows/redistributing-visual-cpp-files`n", $utf8)
        $resources[$notice.Replace('\', '/')] = 'licenses/microsoft/visual-cpp-runtime.txt'
        $licenseInputs = @(Get-ChildItem -LiteralPath (Join-Path $vs 'Licenses') -File -Recurse -ErrorAction SilentlyContinue | Where-Object { $_.Extension -in @('.txt', '.rtf', '.html', '.htm', '.pdf') })
        if (-not $licenseInputs.Count) { throw 'Visual Studio distribution license terms are missing; refusing notice-incomplete artifact.' }
        foreach ($terms in $licenseInputs) {
            $rel = [IO.Path]::GetRelativePath((Join-Path $vs 'Licenses'), $terms.FullName).Replace('\', '/')
            $dest = Join-Path $payload "licenses/microsoft/visual-studio/$rel"
            Copy-BundleFile $terms.FullName $dest
            $resources[$dest.Replace('\', '/')] = "licenses/microsoft/visual-studio/$rel"
        }
    } elseif ($IsLinux) {
        foreach ($binary in @($mainBinary, $sidecar)) {
            Invoke-BundleCommand -Name "rpath-$([IO.Path]::GetFileName($binary))" -Executable patchelf -Arguments @('--set-rpath', ('$ORIGIN/../lib/' + $config.productName + '/runtimes'), $binary)
        }
        $allowed = [Collections.Generic.List[int]]::new()
        $status = [IO.File]::ReadAllText('/proc/self/status', $utf8)
        $cpuMatch = [regex]::Match($status, '(?m)^Cpus_allowed_list:\s*(\S+)')
        if (-not $cpuMatch.Success) { throw 'Unable to discover the native bundle process CPU allowance.' }
        foreach ($part in $cpuMatch.Groups[1].Value.Split(',')) {
            $range = $part.Split('-'); $first = [int]$range[0]; $last = if ($range.Length -eq 2) { [int]$range[1] } else { $first }
            for ($cpu = $first; $cpu -le $last -and $allowed.Count -lt 2; $cpu++) { $allowed.Add($cpu) }
            if ($allowed.Count -eq 2) { break }
        }
        if (-not $allowed.Count) { throw 'No allowed native bundle CPUs.' }
        $bundleCpuList = $allowed -join ','
        Write-BundleLog INFO "Linux bundler CPU affinity=$bundleCpuList; inherited by compression descendants"
    }
    if (-not $IsWindows) {
        Invoke-BundleCommand -Name executable-mode -Executable chmod -Arguments @('755', $mainBinary, $sidecar)
    }
    $provenance = Join-Path $payload 'bundle-provenance.json'
    $payloadRecord = @{ schema = 1; source_sha = $env:GITHUB_SHA; target = $TargetId; status = 'development-unqualified'; signing = 'unsigned'; input_inventory_sha256 = (Get-FileHash -LiteralPath (Join-Path $portable 'manifest.json')).Hash.ToLowerInvariant(); files = @(Get-Inventory $payload $payload) }
    [IO.File]::WriteAllText($provenance, ($payloadRecord | ConvertTo-Json -Depth 12), $utf8)
    $resources[$provenance.Replace('\', '/')] = 'bundle-provenance.json'
    $payloadInventory = @(Get-Inventory $payload $payload)
    $bundleConfig = @{ bundle = @{ resources = $resources; externalBin = @($sidecarBase.Replace('\', '/')) } }
    if ($IsMacOS) {
        $bundleConfig.bundle.macOS = @{ files = @{ 'MacOS/runtimes' = (Join-Path $payload 'runtimes').Replace('\', '/'); 'MacOS/data/models' = (Join-Path $payload 'data/models').Replace('\', '/') } }
    }
    $configPath = Join-Path $stage 'bundle.conf.json'
    [IO.File]::WriteAllText($configPath, ($bundleConfig | ConvertTo-Json -Depth 12), $utf8)
    $formats = @($target.package_formats)
    $nativeFormats = @($formats | Where-Object { $_ -notin @('zip', 'tar.gz') })
    $bundleArguments = @('tauri', 'bundle', '--ci', '--no-sign', '--no-binary-patching', '--config', $configPath, '--bundles', ($nativeFormats -join ','), '-v')
    if ($IsLinux) {
        Invoke-BundleCommand -Name native-formats -Executable taskset -Arguments (@('--cpu-list', $bundleCpuList, 'cargo') + $bundleArguments) -Wall 1500 -Idle 600
    } else { Invoke-BundleCommand -Name native-formats -Executable cargo -Arguments $bundleArguments -Wall 1500 -Idle 600 }
    $artifacts = [Collections.Generic.List[object]]::new()
    $checks = 2
    $formatProofs = [Collections.Generic.List[string]]::new()
    foreach ($format in $nativeFormats) {
        $dir = if ($format -eq 'app') { 'macos' } else { $format }
        $pattern = switch ($format) { 'nsis' { '*-setup.exe' } 'app' { '*.app' } 'dmg' { '*.dmg' } 'appimage' { '*.AppImage' } 'deb' { '*.deb' } 'rpm' { '*.rpm' } }
        $found = @(Get-ChildItem -LiteralPath (Join-Path $release "bundle/$dir") -Filter $pattern)
        if ($found.Count -ne 1) { throw "Required format is missing or ambiguous: $format" }
        $dest = Join-Path $output $found[0].Name
        if (Test-Path -LiteralPath $dest) { throw 'Refusing to overwrite an existing bundle output.' }
        if ($format -eq 'app') {
            Invoke-BundleCommand -Name copy-app -Executable ditto -Arguments @($found[0].FullName, $dest) -Wall 180 -Idle 120
            $contents = Join-Path $dest 'Contents'
            foreach ($member in @('Info.plist', 'MacOS/numera-video-sub-finder', 'MacOS/nvsf', 'Resources/icon.icns')) {
                if (-not [IO.File]::Exists((Join-Path $contents $member))) { throw "Application bundle is missing: $member" }
            }
            Assert-InstalledPayload (Join-Path $contents 'Resources') (Join-Path $contents 'MacOS')
            foreach ($branch in @('runtimes', 'data/models')) {
                foreach ($entry in @(Get-Inventory (Join-Path $payload $branch) $payload)) { Assert-FileBytes (Join-Path $contents "MacOS/$($entry.file)") $entry.sha256 $entry.bytes }
            }
            $formatProofs.Add('app: GUI/CLI/resource byte inventory and expected native layout')
            $appFiles = @(Get-Inventory $dest $packages)
            $artifacts.Add(@{ format = $format; path = [IO.Path]::GetRelativePath($packages, $dest).Replace('\', '/'); files = $appFiles; bytes = ($appFiles | Measure-Object bytes -Sum).Sum })
        } else {
            Copy-BundleFile $found[0].FullName $dest
            switch ($format) {
                'nsis' { Assert-Header $dest '4D5A' }
                'appimage' { Assert-Header $dest '7F454C46'; Assert-Header $dest '414902' 8 }
                'deb' { Assert-Header $dest '213C617263683E0A' }
                'rpm' { Assert-Header $dest 'EDABEEDB' }
                'dmg' { Invoke-BundleCommand -Name dmg-integrity -Executable hdiutil -Arguments @('verify', $dest) -Wall 180 -Idle 120 }
            }
            if ($format -in @('nsis', 'deb', 'rpm', 'appimage')) {
                $inspect = Join-Path $stage "inspect-$format"
                [IO.Directory]::CreateDirectory($inspect) | Out-Null
                switch ($format) {
                    'nsis' {
                        Invoke-BundleCommand -Name nsis-content -Executable 7z -Arguments @('x', '-y', '-mmt2', '-sccUTF-8', "-o$inspect", $dest) -Wall 180 -Idle 90
                        $workers = @(Get-ChildItem -LiteralPath $inspect -Filter 'nvsf.exe' -File -Recurse)
                        if ($workers.Count -ne 1) { throw 'NSIS extraction does not contain exactly one native CLI.' }
                        $binDir = $workers[0].DirectoryName
                        Assert-InstalledPayload $binDir $binDir
                    }
                    'deb' {
                        Invoke-BundleCommand -Name deb-content -Executable dpkg-deb -Arguments @('-X', $dest, $inspect) -Wall 180 -Idle 90
                        Assert-InstalledPayload (Join-Path $inspect "usr/lib/$($config.productName)") (Join-Path $inspect 'usr/bin')
                    }
                    'rpm' {
                        Invoke-BundleCommand -Name rpm-content -Executable bsdtar -Arguments @('-xvf', $dest, '-C', $inspect, '--no-same-owner') -Wall 180 -Idle 90
                        Assert-InstalledPayload (Join-Path $inspect "usr/lib/$($config.productName)") (Join-Path $inspect 'usr/bin')
                    }
                    'appimage' {
                        Invoke-BundleCommand -Name appimage-mode -Executable chmod -Arguments @('755', $dest)
                        Invoke-BundleCommand -Name appimage-content -Executable $dest -Arguments @('--appimage-extract') -WorkingDirectory $inspect -Wall 180 -Idle 90
                        $appDir = Join-Path $inspect 'squashfs-root'
                        Assert-InstalledPayload (Join-Path $appDir "usr/lib/$($config.productName)") (Join-Path $appDir 'usr/bin') -LoaderPatched
                    }
                }
            }
            $formatProofs.Add($(if ($format -eq 'dmg') { 'dmg: native hdiutil checksum verification; mounted content/install not claimed' } else { "${format}: actual package header/decompressed content checks; installed execution not claimed" }))
            $artifacts.Add(@{ format = $format; file = [IO.Path]::GetRelativePath($packages, $dest).Replace('\', '/'); bytes = ([IO.FileInfo]::new($dest)).Length; sha256 = (Get-FileHash -LiteralPath $dest).Hash.ToLowerInvariant() })
        }
        $checks++
    }
    if ($IsWindows) {
        # Add the same app-local CRT closure to the portable ZIP, never mutate the hashed input directory.
        $zipRoot = Join-Path $stage $baseName
        [IO.Directory]::CreateDirectory($zipRoot) | Out-Null
        foreach ($entry in $manifest.files) { Copy-BundleFile (Join-Path $portable $entry.file) (Join-Path $zipRoot $entry.file) }
        foreach ($entry in @(Get-Inventory $payload $payload)) {
            $dest = Join-Path $zipRoot $entry.file
            if (-not [IO.File]::Exists($dest)) { Copy-BundleFile (Join-Path $payload $entry.file) $dest }
        }
        $zipManifest = @{ schema = 1; source_sha = $env:GITHUB_SHA; target = $TargetId; status = 'development-unqualified'; signing = 'unsigned'; files = @(Get-Inventory $zipRoot $zipRoot) }
        [IO.File]::WriteAllText((Join-Path $zipRoot 'manifest.json'), ($zipManifest | ConvertTo-Json -Depth 12), $utf8)
        $archive = Join-Path $output "$baseName.zip"
        if (Test-Path -LiteralPath $archive) { throw 'Refusing to overwrite portable ZIP.' }
        [IO.Compression.ZipFile]::CreateFromDirectory($zipRoot, $archive, [IO.Compression.CompressionLevel]::Optimal, $true)
        $zip = [IO.Compression.ZipFile]::OpenRead($archive)
        try {
            $expected = @(Get-Inventory $zipRoot $zipRoot)
            $zipFiles = @($zip.Entries | Where-Object { -not $_.FullName.EndsWith('/') })
            if ($zipFiles.Count -ne $expected.Count) { throw 'ZIP inventory contains missing or additional files.' }
            foreach ($file in $expected) {
                $entry = $zip.GetEntry("$baseName/$($file.file)")
                if (-not $entry -or $entry.Length -ne $file.bytes) { throw 'ZIP entry differs from portable payload.' }
                $stream = $entry.Open()
                try { $hash = [Convert]::ToHexString([Security.Cryptography.SHA256]::HashData($stream)).ToLowerInvariant() }
                finally { $stream.Dispose() }
                if ($hash -ne $file.sha256) { throw 'ZIP decompressed bytes differ from portable payload.' }
            }
        } finally { $zip.Dispose() }
        $checks++
        $artifacts.Add(@{ format = 'zip'; file = [IO.Path]::GetRelativePath($packages, $archive).Replace('\', '/'); bytes = ([IO.FileInfo]::new($archive)).Length; sha256 = (Get-FileHash -LiteralPath $archive).Hash.ToLowerInvariant() })
    } else {
        $archive = Join-Path $output "$baseName.tar.gz"
        if (Test-Path -LiteralPath $archive) { throw 'Refusing to overwrite native archive.' }
        $archiveParent = if ($IsMacOS) { $output } else { $packages }
        $archiveName = if ($IsMacOS) { "$($config.productName).app" } else { $baseName }
        Invoke-BundleCommand -Name native-archive -Executable tar -Arguments @('-czvf', $archive, '-C', $archiveParent, $archiveName) -Wall 300 -Idle 120
        Assert-Header $archive '1F8B'
        $inspect = Join-Path $stage 'inspect-tar'
        [IO.Directory]::CreateDirectory($inspect) | Out-Null
        Invoke-BundleCommand -Name archive-integrity -Executable tar -Arguments @('-xzvf', $archive, '-C', $inspect) -Wall 180 -Idle 90
        $expectedRoot = Join-Path $archiveParent $archiveName
        foreach ($file in @(Get-Inventory $expectedRoot $expectedRoot)) {
            Assert-FileBytes (Join-Path (Join-Path $inspect $archiveName) $file.file) $file.sha256 $file.bytes
        }
        $checks++
        $artifacts.Add(@{ format = 'tar.gz'; file = [IO.Path]::GetRelativePath($packages, $archive).Replace('\', '/'); bytes = ([IO.FileInfo]::new($archive)).Length; sha256 = (Get-FileHash -LiteralPath $archive).Hash.ToLowerInvariant() })
    }
    $actualFormats = @($artifacts | ForEach-Object { $_.format } | Sort-Object)
    if (Compare-Object ($formats | Sort-Object) $actualFormats) { throw 'Required format inventory is incomplete.' }
    $report = @{ schema = 1; source_sha = $env:GITHUB_SHA; target = $TargetId; passed = $true; tests = $checks; failed = 0; skipped = 0; status = 'development-unqualified'; signing = 'unsigned'; formats = @($artifacts.ToArray()); files = @(Get-Inventory $output $packages); proof = @('same-source input byte inventory', 'real native bundler exit and all required format outputs', 'portable ZIP/tar decompressed byte integrity') + @($formatProofs.ToArray()); qualification = @{ installer_lifecycle = 'NOT_RUN'; installed_launch = 'NOT_RUN'; physical_gpu = 'NOT_RUN'; production_signing = 'NOT_RUN'; linuxdeploy_modified_elf_byte_parity = 'NOT_CLAIMED'; microsoft_redistribution_rights = $(if ($IsWindows) { 'NOT_VALIDATED' } else { 'NOT_APPLICABLE' }); windows_webview = $(if ($IsWindows) { 'system WebView2 required for portable GUI; NSIS bootstrap may require internet' } else { 'NOT_APPLICABLE' }); compression_thread_count = 'NOT_MEASURED'; linux_bundler_cpu_capacity = $(if ($IsLinux) { $bundleCpuList } else { 'NOT_APPLICABLE' }) } }
    $reports = Join-Path $packages 'reports'
    [IO.Directory]::CreateDirectory($reports) | Out-Null
    $reportPath = Join-Path $reports "$TargetId-bundles.json"
    if (Test-Path -LiteralPath $reportPath) { throw 'Refusing to overwrite an existing bundle report.' }
    [IO.File]::WriteAllText($reportPath, ($report | ConvertTo-Json -Depth 14), $utf8)
    Write-BundleLog INFO "Complete target=$TargetId formats=$($actualFormats -join ',') checks=$checks output=$output signing=unsigned qualification=NOT_RUN"
    $global:LASTEXITCODE = 0
} catch {
    $global:LASTEXITCODE = 1
    Write-BundleLog ERROR "Bundling failed: $($_.Exception.Message)"
    throw
} finally {
    foreach ($name in $savedEnv.Keys) { [Environment]::SetEnvironmentVariable($name, $savedEnv[$name], 'Process') }
    $writer.Dispose()
}
