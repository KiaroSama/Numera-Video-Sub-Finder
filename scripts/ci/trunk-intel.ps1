#requires -Version 7.4
[CmdletBinding()]
param()
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
if ($env:GITHUB_ACTIONS -ne 'true' -or $env:RUNNER_ENVIRONMENT -ne 'github-hosted' -or -not $IsMacOS -or [Runtime.InteropServices.RuntimeInformation]::ProcessArchitecture -ne 'X64' -or [Runtime.InteropServices.RuntimeInformation]::OSArchitecture -ne 'X64') { throw 'Trunk Intel admission requires native macOS x64 GitHub-hosted Actions.' }
$root = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../..'))
$guard = Join-Path $PSScriptRoot 'guard.ps1'
$utf8 = [Text.UTF8Encoding]::new($false, $true)
$logs = Join-Path $root '.ci-work/logs'
[IO.Directory]::CreateDirectory($logs) | Out-Null
$log = Join-Path $logs ('trunk-intel_' + [DateTime]::UtcNow.ToString('yyyy-MM-dd_HH-mm-ss') + '_UTC.log')
if (Test-Path -LiteralPath $log) { $log = $log.Replace('.log', '_' + [Guid]::NewGuid().ToString('N') + '.log') }
try { $writer = [IO.StreamWriter]::new([IO.FileStream]::new($log,[IO.FileMode]::CreateNew),$utf8); $writer.AutoFlush = $true }
catch { [Console]::Error.WriteLine('Trunk admission file logging failed; no tool was admitted.'); throw }
function Write-ToolLog([string]$Level,[string]$Message) {
    $line = '[{0}] [{1}] [TRUNK-INTEL] {2}' -f [DateTime]::UtcNow.ToString('yyyy-MM-dd HH:mm:ss UTC'),$Level,$Message
    $writer.WriteLine($line)
    [Console]::Error.WriteLine($line)
}
try {
    Write-ToolLog 'INFO' 'Admitting official Trunk 0.22.1 Intel tool; application builds remain required.'
    $tools = Join-Path $root '.ci-work/tools'
    $stage = Join-Path $tools ('trunk-stage-' + [Guid]::NewGuid().ToString('N'))
    [IO.Directory]::CreateDirectory($stage) | Out-Null
    $archive = Join-Path $stage 'trunk.tar.gz'
    & $guard -Name trunk-download -WallSeconds 210 -IdleSeconds 60 -Executable curl -Arguments @('--fail','--location','--proto','=https','--proto-redir','=https','--connect-timeout','30','--max-time','180','--output',$archive,'https://github.com/trunk-rs/trunk/releases/download/v0.22.1/trunk-x86_64-apple-darwin.tar.gz')
    if ($LASTEXITCODE -ne 0 -or (Get-Item -LiteralPath $archive).Length -ne 9062029 -or (Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash.ToLowerInvariant() -ne '55b5cf2d6a23201f555cc561cc07157989581cc720a260505f8e98aae67abcf7') { throw 'Official Trunk archive identity mismatch.' }
    Add-Type -AssemblyName System.Formats.Tar
    $input = [IO.File]::OpenRead($archive)
    $gzip = [IO.Compression.GZipStream]::new($input,[IO.Compression.CompressionMode]::Decompress)
    $tar = [System.Formats.Tar.TarReader]::new($gzip)
    $tool = Join-Path $stage 'trunk'
    $deadline = [Diagnostics.Stopwatch]::StartNew()
    try {
        $entry = $tar.GetNextEntry($false)
        if ($null -eq $entry -or $entry.Name -cne 'trunk' -or $entry.EntryType -notin @([System.Formats.Tar.TarEntryType]::RegularFile,[System.Formats.Tar.TarEntryType]::V7RegularFile) -or $entry.Length -ne 31080496 -or [int]$entry.Mode -ne 493) { throw 'Unexpected Trunk archive inventory or member type.' }
        $output = [IO.FileStream]::new($tool,[IO.FileMode]::CreateNew)
        try {
            $buffer = [byte[]]::new(65536)
            $received = 0L
            while (($count = $entry.DataStream.Read($buffer,0,$buffer.Length)) -gt 0) {
                $received += $count
                if ($received -gt 31080496 -or $deadline.Elapsed.TotalSeconds -ge 15) { throw 'Trunk member exceeds its byte or inspection deadline.' }
                $output.Write($buffer,0,$count)
            }
            if ($received -ne 31080496) { throw 'Truncated Trunk member.' }
            $output.Flush($true)
        } finally { $output.Dispose() }
        if ($null -ne $tar.GetNextEntry($false) -or $deadline.Elapsed.TotalSeconds -ge 15) { throw 'Unexpected extra Trunk archive entry or inspection deadline.' }
    } finally { $tar.Dispose(); $gzip.Dispose(); $input.Dispose() }
    if ((Get-FileHash -LiteralPath $tool -Algorithm SHA256).Hash.ToLowerInvariant() -ne 'e14136164f9624c8f3729ebca4f6a58aea20a7c8e56e13c0aa265de2d6956f90') { throw 'Extracted Trunk tool identity mismatch.' }
    $architecture = Join-Path $stage 'architecture.txt'
    & $guard -Name trunk-architecture -WallSeconds 15 -IdleSeconds 5 -Executable lipo -Arguments @('-archs',$tool) -CaptureStdoutPath $architecture
    if ($LASTEXITCODE -ne 0 -or [IO.File]::ReadAllText($architecture,$utf8).Trim() -cne 'x86_64') { throw 'Trunk is not the required native Intel tool.' }
    & $guard -Name trunk-mode -WallSeconds 15 -IdleSeconds 5 -Executable chmod -Arguments @('755',$tool)
    if ($LASTEXITCODE -ne 0) { throw 'Trunk executable mode setup failed.' }
    $version = Join-Path $stage 'version.txt'
    & $guard -Name trunk-version -WallSeconds 15 -IdleSeconds 5 -Executable $tool -Arguments @('--version') -CaptureStdoutPath $version
    if ($LASTEXITCODE -ne 0 -or [IO.File]::ReadAllText($version,$utf8).Trim() -cne 'trunk 0.22.1') { throw 'Actual Trunk version mismatch.' }
    $bin = Join-Path $tools 'bin'
    [IO.Directory]::CreateDirectory($bin) | Out-Null
    [IO.File]::Move($tool,(Join-Path $bin 'trunk'))
    Write-ToolLog 'INFO' 'Official archive/member hashes, single regular entry, native Intel architecture and actual Trunk version admitted.'
} catch {
    Write-ToolLog 'ERROR' ('Trunk admission failed: ' + $_.Exception.Message)
    throw
} finally { $writer.Dispose() }
