#requires -Version 7.4
[CmdletBinding()]
param()
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
if ($env:GITHUB_ACTIONS -ne 'true' -or $env:RUNNER_ENVIRONMENT -ne 'github-hosted') {
    throw 'Guard regressions run only on GitHub-hosted Actions runners.'
}
$root = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../..'))
$work = Join-Path $root ".ci-work/guard-contract-$([Guid]::NewGuid().ToString('N'))"
[IO.Directory]::CreateDirectory($work) | Out-Null
$utf8 = [Text.UTF8Encoding]::new($false)
$guard = Join-Path $PSScriptRoot 'guard.ps1'
$pwsh = (Get-Command pwsh -CommandType Application -ErrorAction Stop | Select-Object -First 1).Source
$originalPath = $env:PATH
$ownedChild = $null
function Invoke-Contract([string]$Name, [string]$Code, [string]$Capture, [string]$Progress) {
    $encoded = [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes($Code))
    $parameters = @{ Name = "contract-$Name"; WallSeconds = 15; IdleSeconds = 5; FilePath = $pwsh; Arguments = @('-NoLogo','-NoProfile','-NonInteractive','-EncodedCommand',$encoded); WorkingDirectory = $work; CaptureStdoutPath = $Capture }
    if ($Progress) { $parameters.ProgressDirectory = $Progress }
    & $guard @parameters
}
try {
    . (Join-Path $PSScriptRoot 'process-snapshot.ps1')
    $snapshot = @(ConvertFrom-MacProcessSnapshot "42 1 42 S 0:01.23 128 cargo`n43 42 42 R+ 1:20.00 4096 rustc" 42)
    if ($snapshot.Count -ne 2 -or $snapshot[1].executable -cne 'rustc' -or $snapshot[1].rss_kib -ne 4096 -or $snapshot[1].cpu_time -cne '1:20.00') { throw 'Owned process snapshot lost literal resource evidence.' }
    foreach ($invalid in @('43 42 99 R 0:00.00 1 rustc','43 42 42 R 0:00.00 1 /private/rustc','43 42 42 R 0:00.00 1 rustc --private-argument','malformed')) {
        $rejected = $false
        try { $null = ConvertFrom-MacProcessSnapshot $invalid 42 } catch { $rejected = $true }
        if (-not $rejected) { throw 'Process snapshot accepted an unrelated group or private text.' }
    }
    # Duplicate real executable directories reproduce multi-candidate PATH resolution without mocks.
    $env:PATH = (Split-Path $pwsh -Parent) + [IO.Path]::PathSeparator + $originalPath
    if (-not $IsWindows) {
        $python = (Get-Command python3 -CommandType Application -ErrorAction Stop | Select-Object -First 1).Source
        $env:PATH = (Split-Path $python -Parent) + [IO.Path]::PathSeparator + $env:PATH
    }
    $capture = Join-Path $work 'unicode.stdout'
    Invoke-Contract 'unicode' '[Console]::OutputEncoding=[Text.UTF8Encoding]::new($false); [Console]::WriteLine("guard 日本 فارسی"); exit 0' $capture
    if ([IO.File]::ReadAllText($capture, $utf8).Trim() -cne 'guard 日本 فارسی') { throw 'Guard did not preserve exact Unicode stdout.' }
    $rejected = $false
    try { Invoke-Contract 'failure' 'exit 7' (Join-Path $work 'failure.stdout') }
    catch { $rejected = $_.Exception.Message -match 'exit_code=7' }
    if (-not $rejected) { throw 'Guard did not preserve a real nonzero exit code.' }

    $leakCode = @'
$info=[Diagnostics.ProcessStartInfo]::new()
$info.FileName=(Get-Command pwsh -CommandType Application -ErrorAction Stop | Select-Object -First 1).Source
$info.UseShellExecute=$false
$info.CreateNoWindow=$true
foreach($arg in @('-NoLogo','-NoProfile','-NonInteractive','-Command','[Threading.Thread]::Sleep(60000)')) { $info.ArgumentList.Add($arg) }
$child=[Diagnostics.Process]::Start($info)
[IO.File]::WriteAllText('child.json',(@{pid=$child.Id;start=$child.StartTime.ToUniversalTime().ToString('o')}|ConvertTo-Json -Compress),[Text.UTF8Encoding]::new($false))
$child.Dispose()
exit 0
'@
    $rejected = $false
    try { Invoke-Contract 'leak' $leakCode (Join-Path $work 'leak.stdout') }
    catch { $rejected = $_.Exception.Message -match 'Guarded command failed' }
    if (-not (Test-Path -LiteralPath (Join-Path $work 'child.json'))) { throw 'Leak regression did not reach actual child creation.' }
    $ownedChild = [IO.File]::ReadAllText((Join-Path $work 'child.json'), $utf8) | ConvertFrom-Json
    if (-not $rejected) { throw 'Guard accepted an actual surviving descendant.' }
    $survivor = Get-Process -Id $ownedChild.pid -ErrorAction SilentlyContinue
    try {
        if ($survivor -and $survivor.StartTime.ToUniversalTime().ToString('o') -eq $ownedChild.start -and -not $survivor.HasExited) { throw 'Guard left its real regression child alive.' }
    } finally { if ($survivor) { $survivor.Dispose() } }
    $cases = 6
    if ($IsWindows) {
        Add-Type -TypeDefinition @'
using System;
using System.Diagnostics;
using System.IO;
using System.Text;
using System.Threading;
using System.Threading.Tasks;
public sealed class NvsfRetainedProcess : IDisposable {
    readonly CancellationTokenSource stop = new CancellationTokenSource();
    readonly Task monitor;
    Process child;
    public NvsfRetainedProcess(string path) {
        monitor = Task.Run(() => {
            var elapsed = Stopwatch.StartNew();
            while (!File.Exists(path)) {
                stop.Token.ThrowIfCancellationRequested();
                if (elapsed.Elapsed.TotalSeconds >= 10) throw new TimeoutException("Retained-handle child did not appear.");
                Thread.Sleep(20);
            }
            var record = File.ReadAllLines(path, new UTF8Encoding(false));
            if (record.Length != 2 || !int.TryParse(record[0], out int pid) || pid <= 0)
                throw new InvalidOperationException("Retained-handle child identity is invalid.");
            child = Process.GetProcessById(pid);
            if (child.StartTime.ToUniversalTime().ToString("o") != record[1])
                throw new InvalidOperationException("Retained-handle PID identity changed.");
            if (child.Handle == IntPtr.Zero) throw new InvalidOperationException("Process handle was not opened.");
            File.WriteAllText(Path.Combine(Path.GetDirectoryName(path), "held.ready"), "held", new UTF8Encoding(false));
        });
    }
    public void Verify() {
        if (!monitor.Wait(10000)) throw new TimeoutException("Retained-handle monitor did not finish.");
        if (child == null || !child.HasExited || child.ExitCode != 0)
            throw new InvalidOperationException("Retained child did not exit successfully while its real handle stayed open.");
    }
    public void Dispose() {
        stop.Cancel();
        try {
            if (!monitor.Wait(1000)) throw new TimeoutException("Retained-handle monitor cleanup failed.");
        } finally { child?.Dispose(); stop.Dispose(); }
    }
}
'@
        $heldCode = @'
$ErrorActionPreference='Stop'
$info=[Diagnostics.ProcessStartInfo]::new()
$info.FileName=(Get-Command pwsh -CommandType Application -ErrorAction Stop | Select-Object -First 1).Source
$info.UseShellExecute=$false
$info.CreateNoWindow=$true
$childCode='$deadline=[DateTime]::UtcNow.AddSeconds(10); while(-not [IO.File]::Exists("held.ready")) { if([DateTime]::UtcNow -ge $deadline) { exit 2 }; [Threading.Thread]::Sleep(20) }; exit 0'
foreach($arg in @('-NoLogo','-NoProfile','-NonInteractive','-Command',$childCode)) { $info.ArgumentList.Add($arg) }
$child=[Diagnostics.Process]::Start($info)
try {
    [IO.File]::WriteAllLines('held.tmp',@([string]$child.Id,$child.StartTime.ToUniversalTime().ToString('o')),[Text.UTF8Encoding]::new($false))
    [IO.File]::Move('held.tmp','held.json')
    if(-not $child.WaitForExit(3000)) { throw 'Retained regression child did not exit within its deadline.' }
    if($child.ExitCode -ne 0) { throw 'Retained regression synchronization failed.' }
} finally { $child.Dispose() }
exit 0
'@
        $retained = [NvsfRetainedProcess]::new((Join-Path $work 'held.json'))
        try {
            Invoke-Contract 'retained-exited-handle' $heldCode (Join-Path $work 'held.stdout')
            $retained.Verify()
        } finally { $retained.Dispose() }
        $cases++
    }
    $progress = Join-Path $work 'buildlogs'
    [IO.Directory]::CreateDirectory($progress) | Out-Null
    # An actual multi-port inventory exceeds one 128-entry sampler page without creating source-tree work.
    for ($port = 0; $port -lt 43; $port++) {
        $directory = Join-Path $progress "port-$port"
        [IO.Directory]::CreateDirectory($directory) | Out-Null
        for ($stage = 0; $stage -lt 4; $stage++) {
            [IO.File]::WriteAllText((Join-Path $directory "stage-$stage.log"), 'owned baseline', $utf8)
        }
    }
    $quietCode = @'
[IO.Directory]::CreateDirectory('buildlogs/ffmpeg/source/deep')|Out-Null
for($i=0;$i -lt 1000;$i++) {
    [IO.File]::WriteAllText("buildlogs/ffmpeg/source/deep/unit-$i.c",'owned harmless source noise',[Text.UTF8Encoding]::new($false))
}
for($i=0;$i -lt 12;$i++) {
    [IO.File]::AppendAllText('buildlogs/ffmpeg/build.log',"completed-unit=$i`n",[Text.UTF8Encoding]::new($false))
    # Controlled quiet-build cadence exceeds the guard idle limit in total, not between writes.
    [Threading.Thread]::Sleep(500)
}
exit 0
'@
    Invoke-Contract 'file-progress' $quietCode (Join-Path $work 'quiet.stdout') $progress
    if ([IO.File]::ReadAllLines((Join-Path $progress 'ffmpeg/build.log'), $utf8).Count -ne 12) { throw 'Quiet build did not complete every actual log write.' }
    $rejected = $false
    $frozenCode = @'
$deadline=[DateTime]::UtcNow.AddSeconds(12)
while([DateTime]::UtcNow -lt $deadline) {
    [IO.File]::AppendAllText('buildlogs/ffmpeg/source/deep/noise.log',"source-not-build-progress`n",[Text.UTF8Encoding]::new($false))
    [Threading.Thread]::Sleep(20)
}
exit 0
'@
    $failureLog = $null
    try { Invoke-Contract 'frozen-progress' $frozenCode (Join-Path $work 'frozen.stdout') $progress }
    catch {
        $rejected = $_.Exception.Message -match 'exit_code=124'
        if ($_.Exception.Message -match ' log=(.+)$') { $failureLog = $Matches[1] }
    }
    if (-not $rejected) { throw 'Existing unchanged build logs incorrectly reset the idle deadline.' }
    if ($IsMacOS) {
        if (-not $failureLog) { throw 'Frozen timeout did not retain its exact log identity.' }
        $snapshotPath = $failureLog + '.processes.json'
        $record = [IO.File]::ReadAllText($snapshotPath,$utf8) | ConvertFrom-Json
        if ($record.schema -ne 1 -or $record.owned_group -le 1 -or @($record.processes).Count -eq 0) { throw 'Actual macOS timeout snapshot is missing or empty.' }
        if (@($record.processes | Where-Object { $_.pgid -ne $record.owned_group }).Count -ne 0 -or @($record.processes | Where-Object { $_.pid -eq $record.owned_group -and $_.executable -eq 'pwsh' }).Count -ne 1) { throw 'Actual macOS snapshot did not identify the owned root/group.' }
    }
    [Console]::WriteLine("Guard contracts passed: tests=$cases failed=0 skipped=0; Unicode, exit status, child cleanup, quiet log progress, frozen log timeout and platform process-handle semantics.")
    $global:LASTEXITCODE = 0
} finally {
    $env:PATH = $originalPath
    if ($ownedChild) {
        $child = Get-Process -Id $ownedChild.pid -ErrorAction SilentlyContinue
        try {
            if ($child -and $child.StartTime.ToUniversalTime().ToString('o') -eq $ownedChild.start -and -not $child.HasExited) {
                $child.Kill($true)
                if (-not $child.WaitForExit(3000)) { throw 'Regression child cleanup failed.' }
            }
        } finally { if ($child) { $child.Dispose() } }
    }
    Remove-Item -LiteralPath $work -Recurse -Force
}
