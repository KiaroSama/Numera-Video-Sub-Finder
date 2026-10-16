#requires -Version 7.4
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
if ($env:GITHUB_ACTIONS -ne 'true' -or $env:RUNNER_ENVIRONMENT -ne 'github-hosted') {
    throw 'Frontend tool provisioning is restricted to GitHub-hosted Actions.'
}
if (-not $IsWindows -or [Runtime.InteropServices.RuntimeInformation]::ProcessArchitecture -ne 'Arm64') {
    throw 'This tool admission requires native Windows ARM64.'
}
$root = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../..'))
$guard = Join-Path $PSScriptRoot 'guard.ps1'
$utf8 = [Text.UTF8Encoding]::new($false, $true)
$logger = $null
try {
    $logs = Join-Path $root '.ci-work/logs'
    [IO.Directory]::CreateDirectory($logs) | Out-Null
    $log = Join-Path $logs ('frontend-tools_' + [DateTime]::UtcNow.ToString('yyyy-MM-dd_HH-mm-ss') + '_UTC.log')
    if (Test-Path -LiteralPath $log) { $log = $log.Replace('.log', '_' + [Guid]::NewGuid().ToString('N') + '.log') }
    $logger = [IO.StreamWriter]::new([IO.FileStream]::new($log, [IO.FileMode]::CreateNew), $utf8)
    $logger.AutoFlush = $true
} catch { [Console]::Error.WriteLine('Frontend file logging unavailable; captured console remains active.') }
function Write-ToolLog([string]$Level, [string]$Message) {
    $line = '[{0}] [{1}] [FRONTEND-TOOLS] {2}' -f [DateTime]::UtcNow.ToString('yyyy-MM-dd HH:mm:ss UTC'), $Level, $Message
    if ($logger) { $logger.WriteLine($line) }
    [Console]::Error.WriteLine($line)
}
try {
    Write-ToolLog 'INFO' 'Admitting native Windows ARM64 frontend helpers; child commands use the owned guard.'
    $tools = Join-Path $root '.ci-work/tools'
    $bin = Join-Path $tools 'bin'
    $stage = Join-Path $tools ('frontend-' + [Guid]::NewGuid().ToString('N'))
    [IO.Directory]::CreateDirectory($stage) | Out-Null
    [IO.Directory]::CreateDirectory($bin) | Out-Null
    function Assert-NativeTool([string]$Path) {
        $stream = [IO.File]::OpenRead($Path)
        $reader = [IO.BinaryReader]::new($stream)
        try {
            if ($stream.Length -lt 64 -or $reader.ReadUInt16() -ne 0x5A4D) { throw 'Frontend tool has no PE header.' }
            $stream.Position = 0x3C
            $offset = $reader.ReadUInt32()
            if ($offset -gt $stream.Length - 6) { throw 'Frontend tool PE header is truncated.' }
            $stream.Position = $offset
            if ($reader.ReadUInt32() -ne 0x4550 -or $reader.ReadUInt16() -ne 0xAA64) { throw 'Frontend tool is not native ARM64.' }
        } finally { $reader.Dispose() }
    }
    # Trunk reuses a PATH tool only when its reported version matches the application crate.
    & $guard -Name wasm-bindgen-install -WallSeconds 1200 -IdleSeconds 180 -Executable cargo -Arguments @('install','wasm-bindgen-cli','--version','0.2.129','--locked','--bin','wasm-bindgen','--root',$tools)
    if ($LASTEXITCODE -ne 0) { throw 'Native wasm-bindgen provisioning failed.' }
    $bindgen = Join-Path $bin 'wasm-bindgen.exe'
    Assert-NativeTool $bindgen
    & $guard -Name wasm-bindgen-version -WallSeconds 15 -IdleSeconds 5 -Executable $bindgen -Arguments @('--version') -CaptureStdoutPath (Join-Path $stage 'bindgen-version.txt')
    if ($LASTEXITCODE -ne 0 -or [IO.File]::ReadAllText((Join-Path $stage 'bindgen-version.txt'), $utf8).Trim() -ne 'wasm-bindgen 0.2.129') { throw 'Native wasm-bindgen version mismatch.' }
    $archive = Join-Path $stage 'binaryen.tar.gz'
    & $guard -Name binaryen-download -WallSeconds 210 -IdleSeconds 60 -Executable curl -Arguments @('--fail','--location','--proto','=https','--proto-redir','=https','--connect-timeout','15','--max-time','180','--output',$archive,'https://github.com/WebAssembly/binaryen/releases/download/version_133/binaryen-version_133-arm64-windows.tar.gz')
    if ($LASTEXITCODE -ne 0 -or ([IO.FileInfo]::new($archive)).Length -ne 109735160 -or (Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash.ToLowerInvariant() -ne '492a8e1847a0be1554bb9a7f384227981d60bc013aedc02d8ba1372c3943178c') { throw 'Native Binaryen archive identity mismatch.' }
    $output = Join-Path $bin 'wasm-opt.exe'
    if (Test-Path -LiteralPath $output) { throw 'Refusing to replace an existing optimizer.' }
    Add-Type -AssemblyName System.Formats.Tar
    $input = [IO.File]::OpenRead($archive)
    $gzip = [IO.Compression.GZipStream]::new($input, [IO.Compression.CompressionMode]::Decompress)
    $tar = [System.Formats.Tar.TarReader]::new($gzip)
    $seen = [Collections.Generic.HashSet[string]]::new([StringComparer]::Ordinal)
    $count = 0
    $total = 0L
    $deadline = [DateTime]::UtcNow.AddSeconds(60)
    try {
        while ($null -ne ($entry = $tar.GetNextEntry($false))) {
            $count++
            $name = $entry.Name.TrimEnd('/')
            if ([DateTime]::UtcNow -ge $deadline -or $count -gt 32 -or -not $seen.Add($name) -or $name -notmatch '^binaryen-version_133(?:/(?:bin|include|lib)(?:/[A-Za-z0-9_.-]+)?)?$' -or $name.Contains('..')) { throw 'Unexpected Binaryen archive path, count or deadline.' }
            if ($entry.EntryType -eq [System.Formats.Tar.TarEntryType]::Directory) {
                if ($entry.Length -ne 0) { throw 'Binaryen directory contains data.' }
                continue
            }
            if ($entry.EntryType -notin @([System.Formats.Tar.TarEntryType]::RegularFile, [System.Formats.Tar.TarEntryType]::V7RegularFile) -or $entry.Length -lt 1 -or $entry.Length -gt 536870912) { throw 'Binaryen archive contains a link, special file or oversized entry.' }
            $total += $entry.Length
            if ($total -gt 700000000) { throw 'Binaryen archive exceeds its uncompressed byte budget.' }
            $selected = $name -eq 'binaryen-version_133/bin/wasm-opt.exe'
            $destination = if ($selected) { [IO.FileStream]::new($output, [IO.FileMode]::CreateNew) } else { $null }
            $buffer = [byte[]]::new(65536)
            $received = 0L
            try {
                while (($read = $entry.DataStream.Read($buffer, 0, $buffer.Length)) -gt 0) {
                    if ([DateTime]::UtcNow -ge $deadline) { throw 'Binaryen inspection exceeded deadline.' }
                    $received += $read
                    if ($received -gt $entry.Length) { throw 'Binaryen entry exceeds its declared size.' }
                    if ($destination) { $destination.Write($buffer, 0, $read) }
                }
                if ($received -ne $entry.Length) { throw 'Truncated Binaryen entry.' }
                if ($destination) { $destination.Flush($true) }
            } finally { if ($destination) { $destination.Dispose() } }
        }
    } finally { $tar.Dispose(); $gzip.Dispose(); $input.Dispose() }
    if ($count -ne 21 -or -not [IO.File]::Exists($output) -or ([IO.FileInfo]::new($output)).Length -ne 11148288 -or (Get-FileHash -LiteralPath $output -Algorithm SHA256).Hash.ToLowerInvariant() -ne 'd6c01791133b2eded195b6461e9393de02a9bc36bdc3379918870780ca1c6a4d') { throw 'Native optimizer inventory or bytes mismatch.' }
    Assert-NativeTool $output
    & $guard -Name binaryen-version -WallSeconds 15 -IdleSeconds 5 -Executable $output -Arguments @('--version') -CaptureStdoutPath (Join-Path $stage 'optimizer-version.txt')
    if ($LASTEXITCODE -ne 0 -or [IO.File]::ReadAllText((Join-Path $stage 'optimizer-version.txt'), $utf8).Trim() -notmatch '^wasm-opt version 133(?:\s|$)') { throw 'Native Binaryen version mismatch.' }
    $license = Join-Path $stage 'BINARYEN-LICENSE.txt'
    & $guard -Name binaryen-license -WallSeconds 45 -IdleSeconds 15 -Executable curl -Arguments @('--fail','--proto','=https','--connect-timeout','10','--max-time','30','--output',$license,'https://raw.githubusercontent.com/WebAssembly/binaryen/fba5005a132d85d578be5bd010e0487ed8e2ce3e/LICENSE')
    if ($LASTEXITCODE -ne 0 -or ([IO.FileInfo]::new($license)).Length -ne 11535 -or (Get-FileHash -LiteralPath $license -Algorithm SHA256).Hash.ToLowerInvariant() -ne '64c0d02491e16eced74826440ecd2bcf7722d37fa586faf457b2494293afbffe') { throw 'Binaryen notice identity mismatch.' }
    [IO.File]::AppendAllText($env:GITHUB_PATH, "$bin`n", $utf8)
    Write-ToolLog 'INFO' 'Native ARM64 frontend helpers admitted: wasm-bindgen 0.2.129; Binaryen 133; application build remains required.'
} catch {
    Write-ToolLog 'ERROR' ('Frontend helper admission failed: ' + $_.Exception.Message)
    throw
} finally {
    if ($logger) { $logger.Dispose() }
}
