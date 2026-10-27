#requires -Version 7.4
[CmdletBinding()]
param()
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
if ($env:GITHUB_ACTIONS -ne 'true' -or $env:RUNNER_ENVIRONMENT -ne 'github-hosted' -or -not $IsLinux -or [Runtime.InteropServices.RuntimeInformation]::OSArchitecture -ne 'X64') {
    throw 'SRT fuzz admission requires a public Linux x64 hosted runner.'
}
$root = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../..'))
Set-Location -LiteralPath $root
if ($env:GITHUB_SHA -notmatch '^[0-9a-f]{40}$') { throw 'Fuzz admission requires an exact source SHA.' }
$nightly = 'nightly-2026-10-27'
$utf8 = [Text.UTF8Encoding]::new($false)
$work = Join-Path $root '.ci-work/fuzz'
if ((Get-Item -Force -LiteralPath (Join-Path $root '.ci-work')).LinkType) { throw 'Fuzz state parent must not be a link.' }
if (Test-Path -LiteralPath $work) { throw 'Fuzz work directory must be new for this run.' }
[IO.Directory]::CreateDirectory($work) | Out-Null
foreach ($name in 'corpus','artifacts','logs') { [IO.Directory]::CreateDirectory((Join-Path $work $name)) | Out-Null }
$reportPath = Join-Path $work 'fuzz-report.json'
$logPath = Join-Path $work ('logs/fuzz_' + [DateTime]::UtcNow.ToString('yyyy-MM-dd_HH-mm-ss') + '_UTC.log')
try {
    $writer = [IO.StreamWriter]::new([IO.FileStream]::new($logPath, [IO.FileMode]::CreateNew), $utf8)
    $writer.AutoFlush = $true
} catch {
    [Console]::Error.WriteLine('Fuzz file logging initialization failed; no admission result was produced.')
    throw
}
function Write-FuzzLog([string]$Level, [string]$Message) {
    $line = '[{0}] [{1}] [SRT-FUZZ] {2}' -f [DateTime]::UtcNow.ToString('yyyy-MM-dd HH:mm:ss UTC'), $Level, $Message
    $writer.WriteLine($line)
    [Console]::Error.WriteLine($line)
}
function Hash-File([string]$Path) { (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant() }
function Save-Report { [IO.File]::WriteAllText($reportPath, ($report | ConvertTo-Json -Depth 8), $utf8) }
function Invoke-Guard([string]$Name, [int]$Wall, [int]$Idle, [string]$Executable, [string[]]$ArgumentList, [string]$Capture = '') {
    $arguments = @{ Name=$Name; WallSeconds=$Wall; IdleSeconds=$Idle; Executable=$Executable; Arguments=$ArgumentList; WorkingDirectory=$root }
    if ($Capture) { $arguments.CaptureStdoutPath = $Capture }
    & (Join-Path $PSScriptRoot 'guard.ps1') @arguments
    if ($LASTEXITCODE -ne 0) { throw "Fuzz stage $Name failed with exit code $LASTEXITCODE" }
}
function Read-GuardEvidence([string]$Name) {
    $files = @(Get-ChildItem -LiteralPath (Join-Path $root '.ci-work/logs') -Filter "guard-$Name`_*_UTC.log" -File)
    if ($files.Count -ne 1 -or $files[0].Length -gt 8388608) { throw "Missing, duplicate or oversized guard evidence: $Name" }
    $text = [IO.File]::ReadAllText($files[0].FullName, $utf8)
    if ($text -notmatch "Command finished name=$Name exit_code=0 " -or $text -notmatch 'Cleanup verified pid=\d+') {
        throw "Fuzz stage lacks successful exit and owned cleanup: $Name"
    }
    return $text
}
$started = [DateTime]::UtcNow
$report = [ordered]@{
    schema=1; source_sha=$env:GITHUB_SHA; target='linux-x64'; profile='bounded-srt-libfuzzer';
    passed=$false; tests=1; failed=1; skipped=0; sanitizer='address'; workers=1;
    nightly=$nightly; cargo_fuzz='0.13.2'; libfuzzer_sys='0.4.13';
    limits=@{ campaign_seconds=30; wall_seconds=90; idle_seconds=45; max_len=4096; input_timeout_seconds=2; rss_mb=512; malloc_mb=64 };
    qualification='bounded parser campaign only; not native FFI, Loom or full CT-06 qualification'
}
Save-Report
try {
    Write-FuzzLog 'INFO' 'Starting exact-source parser fuzz admission'
    $head = & git -C $root rev-parse HEAD
    if ($LASTEXITCODE -ne 0 -or $head.Trim() -ne $env:GITHUB_SHA) { throw 'Fuzz checkout SHA differs from workflow source.' }
    $manifest = Join-Path $root 'fuzz/Cargo.toml'
    $lock = Join-Path $root 'fuzz/Cargo.lock'
    $target = Join-Path $root 'fuzz/fuzz_targets/parse_srt.rs'
    $lockBefore = Hash-File $lock
    $report.inputs = @(@{file='fuzz/Cargo.toml';sha256=(Hash-File $manifest)}, @{file='fuzz/Cargo.lock';sha256=$lockBefore}, @{file='fuzz/fuzz_targets/parse_srt.rs';sha256=(Hash-File $target)}, @{file='crates/core/src/export.rs';sha256=(Hash-File (Join-Path $root 'crates/core/src/export.rs'))})
    foreach ($item in $report.inputs) {
        & git -C $root diff --quiet HEAD -- $item.file
        if ($LASTEXITCODE -ne 0) { throw 'Fuzz admission input differs from exact checked-out source.' }
    }
    $seedDirectory = Join-Path $root 'fuzz/seeds/parse_srt'
    foreach ($directory in @((Join-Path $root 'fuzz'), (Join-Path $root 'fuzz/seeds'), $seedDirectory)) {
        if ((Get-Item -Force -LiteralPath $directory).LinkType) { throw 'Maintained fuzz seed directory must not be a link.' }
    }
    $seeds = @(Get-ChildItem -LiteralPath $seedDirectory -File | Sort-Object Name)
    if ($seeds.Count -lt 6 -or $seeds.Count -gt 32) { throw 'Fuzz maintained seed inventory must contain six to32 files.' }
    $seedInventory = @()
    foreach ($seed in $seeds) {
        & git -C $root diff --quiet HEAD -- ('fuzz/seeds/parse_srt/' + $seed.Name)
        if ($LASTEXITCODE -ne 0) { throw 'Maintained fuzz seed differs from exact checked-out source.' }
        if ($seed.LinkType -or $seed.Length -gt 4096 -or $seed.Name -notmatch '^[a-z0-9-]+\.srt$') { throw 'Unsafe or oversized maintained fuzz seed.' }
        $seedInventory += @{file=$seed.Name;bytes=$seed.Length;sha256=(Hash-File $seed.FullName)}
        Copy-Item -LiteralPath $seed.FullName -Destination (Join-Path $work ('corpus/' + $seed.Name)) -ErrorAction Stop
    }
    $report.seeds = $seedInventory
    $report.seed_count = $seeds.Count
    Save-Report
    $version = Join-Path $work 'rustc-version.txt'
    Invoke-Guard 'fuzz-rustc-version' 15 5 'rustup' @('run',$nightly,'rustc','--version','--verbose') $version
    $rustVersion = [IO.File]::ReadAllText($version, $utf8)
    if ($rustVersion -notmatch 'rustc 1\.101\.0-nightly' -or $rustVersion -notmatch 'commit-hash: 16d1ffac1[0-9a-f]{31}' -or $rustVersion -notmatch 'host: x86_64-unknown-linux-gnu') { throw 'Pinned fuzz nightly version/host mismatch.' }
    $tool = Join-Path $root '.ci-work/tools/bin/cargo-fuzz'
    $toolVersion = Join-Path $work 'cargo-fuzz-version.txt'
    Invoke-Guard 'fuzz-tool-version' 15 5 $tool @('--version') $toolVersion
    if ([IO.File]::ReadAllText($toolVersion,$utf8).Trim() -ne 'cargo-fuzz 0.13.2') { throw 'Pinned cargo-fuzz version mismatch.' }
    $report.rustc = $rustVersion.Trim()
    $targetDir = Join-Path $work 'target'
    Invoke-Guard 'fuzz-build' 600 180 'cargo' @("+$nightly",'fuzz','build','parse_srt','--fuzz-dir',(Join-Path $root 'fuzz'),'--target','x86_64-unknown-linux-gnu','--target-dir',$targetDir,'--sanitizer','address','--verbose')
    $buildEvidence = Read-GuardEvidence 'fuzz-build'
    if ($buildEvidence -notmatch 'sanitizer=address' -or $buildEvidence -notmatch 'sanitizer-coverage') { throw 'Fuzz build lacks actual ASan and coverage instrumentation evidence.' }
    if ((Hash-File $lock) -ne $lockBefore) { throw 'Fuzz build changed its committed lockfile.' }
    $binary = Join-Path $targetDir 'x86_64-unknown-linux-gnu/release/parse_srt'
    foreach ($directory in @($targetDir, (Join-Path $targetDir 'x86_64-unknown-linux-gnu'), (Split-Path -Parent $binary))) {
        if ((Get-Item -Force -LiteralPath $directory).LinkType) { throw 'Fuzz executable directory must not be a link.' }
    }
    if (-not (Test-Path -LiteralPath $binary -PathType Leaf)) { throw 'Missing actual instrumented SRT fuzz executable.' }
    $binaryItem = Get-Item -Force -LiteralPath $binary
    if (($binaryItem.Attributes -band [IO.FileAttributes]::ReparsePoint) -or ($binaryItem.LinkType -and $binaryItem.LinkType -ne 'HardLink')) { throw 'Fuzz executable must be a regular file, not a symbolic link.' }
    $report.binary_sha256 = Hash-File $binary
    if ($binaryItem.LinkType -eq 'HardLink') {
        # This pinned Cargo uplifts Linux binaries with hard links from its new build layout.
        $packageBuild = Join-Path (Split-Path -Parent $binary) 'build/nvsf-parser-fuzz'
        foreach ($directory in @((Split-Path -Parent $packageBuild), $packageBuild)) {
            $item = Get-Item -Force -LiteralPath $directory
            if (-not $item.PSIsContainer -or $item.LinkType) { throw 'Fuzz compiler artifact directory must be an unlinked directory.' }
        }
        $units = @(Get-ChildItem -Force -LiteralPath $packageBuild -Directory)
        if ($units.Count -ne 1 -or $units[0].Name -cnotmatch '^[0-9a-f]{16}$' -or $units[0].LinkType) { throw 'Fuzz compiler artifact requires one exact regular build unit.' }
        $out = Get-Item -Force -LiteralPath (Join-Path $units[0].FullName 'out')
        if (-not $out.PSIsContainer -or $out.LinkType) { throw 'Fuzz compiler output directory must not be a link.' }
        $compiled = Get-Item -Force -LiteralPath (Join-Path $out.FullName 'parse_srt')
        if ($compiled.PSIsContainer -or ($compiled.Attributes -band [IO.FileAttributes]::ReparsePoint) -or ($compiled.LinkType -and $compiled.LinkType -ne 'HardLink') -or (Hash-File $compiled.FullName) -ne $report.binary_sha256) {
            throw 'Fuzz hard-linked executable differs from its exact regular compiler artifact.'
        }
        $report.compiler_artifact = [IO.Path]::GetRelativePath($targetDir, $compiled.FullName)
    }
    $report.binary_link_type = $binaryItem.LinkType
    Write-FuzzLog 'INFO' 'Instrumented parser built; replaying maintained seeds'
    $common = @('-max_len=4096','-timeout=2','-rss_limit_mb=512','-malloc_limit_mb=64',('-artifact_prefix=' + (Join-Path $work 'artifacts') + '/'),'-print_final_stats=1')
    Invoke-Guard 'fuzz-seed-replay' 30 15 $binary ($common + @('-runs=0',(Join-Path $work 'corpus')))
    $replay = Read-GuardEvidence 'fuzz-seed-replay'
    if ($replay -notmatch 'INFO: Loaded \d+ modules' -or $replay -notmatch 'INITED.*cov:\s*[1-9]\d*') { throw 'Seed replay lacks actual instrumentation or coverage observations.' }
    Write-FuzzLog 'INFO' 'Starting one30-second single-process coverage-guided campaign'
    Invoke-Guard 'fuzz-campaign' 90 45 $binary ($common + @('-max_total_time=30','-seed=20261027',(Join-Path $work 'corpus')))
    $evidence = Read-GuardEvidence 'fuzz-campaign'
    $runs = [regex]::Match($evidence, 'stat::number_of_executed_units:\s*(\d+)')
    $done = [regex]::Match($evidence, '#(\d+)\s+DONE\s+cov:\s*(\d+)\s+ft:\s*(\d+)')
    if (-not $runs.Success -or -not $done.Success -or [long]$runs.Groups[1].Value -le $seeds.Count -or [long]$done.Groups[2].Value -lt 1 -or [long]$done.Groups[3].Value -lt 1 -or $evidence -notmatch 'INFO: Loaded \d+ modules') {
        throw 'Fuzz campaign lacks nonzero actual execution/coverage completion statistics.'
    }
    if (@(Get-ChildItem -LiteralPath (Join-Path $work 'artifacts') -Force).Count -ne 0) { throw 'Fuzz campaign produced failure artifacts.' }
    if ((Hash-File $lock) -ne $lockBefore) { throw 'Fuzz campaign changed its committed lockfile.' }
    foreach ($seed in $seedInventory) { if ((Hash-File (Join-Path $root ('fuzz/seeds/parse_srt/' + $seed.file))) -ne $seed.sha256) { throw 'Maintained fuzz seed changed during execution.' } }
    foreach ($item in $report.inputs) { if ((Hash-File (Join-Path $root $item.file)) -ne $item.sha256) { throw 'Fuzz admission source changed during execution.' } }
    $report.executions = [long]$runs.Groups[1].Value
    $report.coverage_edges = [long]$done.Groups[2].Value
    $report.features = [long]$done.Groups[3].Value
    $report.seed_replay_passed = $true
    $report.campaign_log_sha256 = Hash-File (@(Get-ChildItem -LiteralPath (Join-Path $root '.ci-work/logs') -Filter 'guard-fuzz-campaign_*_UTC.log' -File)[0].FullName)
    $report.cleanup_verified = $true
    $report.crash_artifacts = 0
    $report.passed = $true
    $report.failed = 0
    Write-FuzzLog 'INFO' "Completed executions=$($report.executions) coverage_edges=$($report.coverage_edges) features=$($report.features)"
} catch {
    $report.passed = $false
    $report.failed = 1
    $report.error = $_.Exception.Message
    Write-FuzzLog 'ERROR' $report.error
    throw
} finally {
    $report.elapsed_ms = [long]([DateTime]::UtcNow - $started).TotalMilliseconds
    try { Save-Report } finally { $writer.Dispose() }
}
