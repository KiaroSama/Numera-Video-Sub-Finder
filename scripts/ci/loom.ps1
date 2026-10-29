#requires -Version 7.4
[CmdletBinding()]
param()
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
if ($env:GITHUB_ACTIONS -ne 'true' -or $env:RUNNER_ENVIRONMENT -ne 'github-hosted' -or -not $IsLinux -or [Runtime.InteropServices.RuntimeInformation]::ProcessArchitecture -ne 'X64') { throw 'Close-admission Loom requires a native Linux x64 hosted runner.' }
$root = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../..'))
Set-Location -LiteralPath $root
$utf8 = [Text.UTF8Encoding]::new($false, $true)
$work = Join-Path $root '.ci-work/loom'
if ((Get-Item -LiteralPath (Join-Path $root '.ci-work') -Force).LinkType -or (Test-Path -LiteralPath $work)) { throw 'Loom evidence requires a new unlinked owned directory.' }
[IO.Directory]::CreateDirectory($work) | Out-Null
$logPath = Join-Path $work ('loom_' + [DateTime]::UtcNow.ToString('yyyy-MM-dd_HH-mm-ss') + '_UTC.log')
try { $writer = [IO.StreamWriter]::new([IO.FileStream]::new($logPath,[IO.FileMode]::CreateNew),$utf8); $writer.AutoFlush = $true }
catch { [Console]::Error.WriteLine('Loom file logging failed; no model evidence was produced.'); throw }
$report = [ordered]@{ schema=1; source_sha=$env:GITHUB_SHA; target='linux-x64'; passed=$false; tests=0; failed=1; skipped=0; scope='production close-request lease only'; suites=@() }
$names = @('RUSTFLAGS','CARGO_ENCODED_RUSTFLAGS','CARGO_TARGET_DIR','RUST_TEST_THREADS','CARGO_BUILD_JOBS')
$environment = @{}
$original = [Environment]::GetEnvironmentVariables('Process')
foreach ($name in $names) { $environment[$name]=@{present=$original.Contains($name);value=$original[$name]} }
function Write-LoomLog([string]$Level,[string]$Message) {
    $line = '[{0}] [{1}] [LOOM] {2}' -f [DateTime]::UtcNow.ToString('yyyy-MM-dd HH:mm:ss UTC'),$Level,$Message
    $writer.WriteLine($line)
    [Console]::Error.WriteLine($line)
}
try {
    if ($env:GITHUB_SHA -notmatch '^[0-9a-f]{40}$') { throw 'Loom requires an exact source SHA.' }
    $head = & git rev-parse HEAD
    if ($LASTEXITCODE -ne 0 -or $head.Trim() -cne $env:GITHUB_SHA) { throw 'Loom checkout source mismatch.' }
    $source = Join-Path $root 'crates/core/src/close_admission.rs'
    $sourceHash = (Get-FileHash -LiteralPath $source -Algorithm SHA256).Hash.ToLowerInvariant()
    $report.production_source_sha256 = $sourceHash
    $report.loom_version = '0.7.2'
    $env:RUSTFLAGS = '--cfg=loom'
    Remove-Item -LiteralPath Env:CARGO_ENCODED_RUSTFLAGS -ErrorAction SilentlyContinue
    $env:CARGO_TARGET_DIR = Join-Path $work 'target'
    $env:RUST_TEST_THREADS = '1'
    $env:CARGO_BUILD_JOBS = '2'
    $capture = Join-Path $work 'model.stdout'
    Write-LoomLog 'INFO' 'Model-checking the same production close-admission source with two contenders; no native application execution.'
    & (Join-Path $PSScriptRoot 'guard.ps1') -Name close-loom -WallSeconds 180 -IdleSeconds 60 -Executable cargo -Arguments @('test','--locked','--manifest-path','ci/loom/Cargo.toml','--lib','--','--test-threads=1','--nocapture') -CaptureStdoutPath $capture
    if ($LASTEXITCODE -ne 0) { throw 'Production close-admission model failed.' }
    $text = [IO.File]::ReadAllText($capture,$utf8)
    $results = [regex]::Matches($text,'test result: ok\. (\d+) passed; (\d+) failed; (\d+) ignored; (\d+) measured; (\d+) filtered out')
    if ($results.Count -ne 1 -or $results[0].Groups[1].Value -ne '1') { throw 'Loom must execute exactly one production-source model.' }
    foreach ($index in 2..5) { if ($results[0].Groups[$index].Value -ne '0') { throw 'Failed, ignored, measured or filtered Loom model.' } }
    $executions = [regex]::Matches($text,'(?m)^close_admission_executions=([0-9]+)\r?$')
    $executionCount = [uint64]0
    if ($executions.Count -ne 1 -or -not [uint64]::TryParse($executions[0].Groups[1].Value,[ref]$executionCount) -or $executionCount -le 1) { throw 'Loom requires actual multiple model executions.' }
    $report.model_executions = $executionCount
    $guards = @(Get-ChildItem -LiteralPath (Join-Path $root '.ci-work/logs') -File -Filter 'guard-close-loom_*_UTC.log')
    if ($guards.Count -ne 1 -or $guards[0].Length -gt 8388608) { throw 'Missing unique bounded Loom guard evidence.' }
    $guardText = [IO.File]::ReadAllText($guards[0].FullName,$utf8)
    if ($guardText -notmatch 'Command finished name=close-loom exit_code=0 ' -or $guardText -notmatch 'Cleanup verified pid=\d+') { throw 'Loom requires successful owned cleanup.' }
    if ((Get-FileHash -LiteralPath $source -Algorithm SHA256).Hash.ToLowerInvariant() -cne $sourceHash) { throw 'Production model source changed during execution.' }
    $report.tests=1; $report.failed=0; $report.passed=$true; $report.cleanup_verified=$true
    $report.suites=@(@{name='actual_close_lease_excludes_duplicates_and_reopens_after_drop';tests=1;failed=0;skipped=0})
    Write-LoomLog 'INFO' 'Production close-admission model passed; full desktop operation/install/job concurrency remains separately qualified.'
} catch {
    Write-LoomLog 'ERROR' $_.Exception.Message
    throw
} finally {
    foreach ($name in $names) {
        if ($environment[$name].present) { Set-Item -LiteralPath "Env:$name" -Value $environment[$name].value }
        else { Remove-Item -LiteralPath "Env:$name" -ErrorAction SilentlyContinue }
    }
    try { [IO.File]::WriteAllText((Join-Path $work 'loom-report.json'),($report | ConvertTo-Json -Depth 8),$utf8) }
    finally { $writer.Dispose() }
}
