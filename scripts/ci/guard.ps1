#requires -Version 7.4
[CmdletBinding()]
param(
    [Parameter(Mandatory)][ValidatePattern('^[a-zA-Z0-9._-]+$')][string]$Name,
    [Parameter(Mandatory)][ValidateRange(1, 10800)][int]$WallSeconds,
    [Parameter(Mandatory)][ValidateRange(1, 3600)][int]$IdleSeconds,
    [Parameter(Mandatory)][Alias('Executable')][string]$FilePath,
    [string[]]$Arguments = @(),
    [string]$WorkingDirectory = (Get-Location).Path,
    [string]$CaptureStdoutPath,
    [string]$ProgressDirectory
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
if ($env:GITHUB_ACTIONS -ne 'true' -or $env:RUNNER_ENVIRONMENT -ne 'github-hosted') {
    throw 'Command execution is restricted to GitHub-hosted Actions runners.'
}
$root = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../..'))
$logs = Join-Path $root '.ci-work/logs'
[IO.Directory]::CreateDirectory($logs) | Out-Null
$stamp = [DateTime]::UtcNow.ToString('yyyy-MM-dd_HH-mm-ss')
$logPath = Join-Path $logs "guard-$Name`_${stamp}_UTC.log"
if (Test-Path -LiteralPath $logPath) { $logPath = $logPath.Replace('.log', "_$([Guid]::NewGuid().ToString('N')).log") }
$utf8 = [Text.UTF8Encoding]::new($false)
$writer = $null
try { $writer = [IO.StreamWriter]::new([IO.FileStream]::new($logPath, [IO.FileMode]::CreateNew), $utf8); $writer.AutoFlush = $true }
catch { [Console]::Error.WriteLine('File logging initialization failed; using console diagnostics.'); throw }
$redactions = @(Get-ChildItem Env: | Where-Object { $_.Name -match '(TOKEN|PASSWORD|SECRET|API_KEY|ACCESS_KEY|CONNECTION_STRING)$' -and $_.Value.Length -ge 8 } | ForEach-Object Value)
function Write-GuardLog([string]$Level, [string]$Text) {
    foreach ($secret in $redactions) { $Text = $Text.Replace($secret, '[REDACTED]') }
    $Text = $Text -replace '(?i)(authorization\s*[:=]\s*|bearer\s+)[^\s]+', '$1[REDACTED]'
    $line = '[{0}] [{1}] [CI-GUARD] {2}' -f [DateTime]::UtcNow.ToString('yyyy-MM-dd HH:mm:ss UTC'), $Level, $Text
    if ($writer.BaseStream.Length -gt 67108864) { throw 'Guard diagnostic log exceeded 64 MiB; command must stop.' }
    $writer.WriteLine($line)
    [Console]::Error.WriteLine($line)
}
# The payload waits on an owned gate so it cannot spawn before its OS ownership boundary exists.
if (-not ('NvsfCiOwnership' -as [type])) {
    Add-Type -TypeDefinition @'
using System;
using System.ComponentModel;
using System.Diagnostics;
using System.Runtime.InteropServices;
public sealed class NvsfCiOwnership : IDisposable {
    IntPtr job;
    [StructLayout(LayoutKind.Sequential)] struct Basic {
        public long processUser, jobUser;
        public uint flags; public UIntPtr min, max; public uint count; public UIntPtr affinity;
        public uint priority, scheduling;
    }
    [StructLayout(LayoutKind.Sequential)] struct Io { public ulong a,b,c,d,e,f; }
    [StructLayout(LayoutKind.Sequential)] struct Extended {
        public Basic basic; public Io io; public UIntPtr processMemory, jobMemory, peakProcess, peakJob;
    }
    [DllImport("kernel32.dll", SetLastError=true)] static extern IntPtr CreateJobObject(IntPtr attrs, string name);
    [DllImport("kernel32.dll", SetLastError=true)] static extern bool SetInformationJobObject(IntPtr h, int type, ref Extended info, uint size);
    [DllImport("kernel32.dll", SetLastError=true)] static extern bool AssignProcessToJobObject(IntPtr h, IntPtr process);
    [DllImport("kernel32.dll", SetLastError=true)] static extern bool QueryInformationJobObject(IntPtr h, int type, IntPtr info, uint size, IntPtr returned);
    [DllImport("kernel32.dll", SetLastError=true)] static extern IntPtr OpenProcess(uint access, bool inherit, uint pid);
    [DllImport("kernel32.dll", SetLastError=true)] static extern bool IsProcessInJob(IntPtr process, IntPtr h, out bool inside);
    [DllImport("kernel32.dll", SetLastError=true)] static extern uint WaitForSingleObject(IntPtr h, uint milliseconds);
    [DllImport("kernel32.dll", SetLastError=true)] static extern bool TerminateJobObject(IntPtr h, uint code);
    [DllImport("kernel32.dll")] static extern bool CloseHandle(IntPtr h);
    [DllImport("libc", SetLastError=true)] static extern int kill(int pid, int sig);
    public static int SignalGroup(int pid, int signal) { return kill(-pid, signal); }
    public static int LastError() { return Marshal.GetLastWin32Error(); }
    public NvsfCiOwnership() {
        job = CreateJobObject(IntPtr.Zero, null);
        if (job == IntPtr.Zero) throw new Win32Exception();
        var settings = new Extended(); settings.basic.flags = 0x2000; // KILL_ON_JOB_CLOSE, no breakaway.
        if (!SetInformationJobObject(job, 9, ref settings, (uint)Marshal.SizeOf<Extended>())) {
            int error = Marshal.GetLastWin32Error(); CloseHandle(job); job = IntPtr.Zero;
            throw new Win32Exception(error);
        }
    }
    public void Assign(Process p) { if (!AssignProcessToJobObject(job, p.Handle)) throw new Win32Exception(); }
    public string LiveProcesses { get; private set; } = "";
    public uint Active(bool collectNames = false) {
        var live = collectNames ? new System.Collections.Generic.List<string>() : null;
        const int capacity = 4096;
        int bytes = 8 + capacity * IntPtr.Size;
        IntPtr buffer = Marshal.AllocHGlobal(bytes);
        try {
            if (!QueryInformationJobObject(job, 3, buffer, (uint)bytes, IntPtr.Zero)) throw new Win32Exception();
            int assigned = Marshal.ReadInt32(buffer), count = Marshal.ReadInt32(buffer, 4);
            if (assigned < 0 || assigned > capacity || count < 0 || count > capacity || count < assigned) throw new InvalidOperationException("Owned job process inventory exceeded its bound.");
            uint active = 0;
            for (int i = 0; i < count; i++) {
                uint pid = checked((uint)Marshal.ReadIntPtr(buffer, 8 + i * IntPtr.Size).ToInt64());
                IntPtr handle = OpenProcess(0x00101000, false, pid); // SYNCHRONIZE | QUERY_LIMITED_INFORMATION
                if (handle == IntPtr.Zero) {
                    if (Marshal.GetLastWin32Error() == 87) continue; // Exited before opening its handle.
                    throw new Win32Exception();
                }
                try {
                    if (!IsProcessInJob(handle, job, out bool inside)) throw new Win32Exception();
                    if (!inside) continue; // PID reused by an unrelated process: never count or terminate it.
                    uint state = WaitForSingleObject(handle, 0);
                    if (state == 0x102) {
                        active++;
                        if (!collectNames) continue;
                        string name;
                        try {
                            using var process = Process.GetProcessById(checked((int)pid));
                            name = process.ProcessName;
                        } catch (ArgumentException) { name = "exited-during-name-read"; }
                        catch (InvalidOperationException) { name = "unavailable"; }
                        catch (Win32Exception) { name = "unavailable"; }
                        live.Add($"{pid}:{name}");
                    } else if (state != 0) throw new Win32Exception();
                } finally { CloseHandle(handle); }
            }
            if (collectNames) LiveProcesses = string.Join(",", live);
            return active;
        } finally { Marshal.FreeHGlobal(buffer); }
    }
    public void Terminate() { if (job != IntPtr.Zero && !TerminateJobObject(job, 124)) throw new Win32Exception(); }
    public void Dispose() { if (job != IntPtr.Zero) { CloseHandle(job); job = IntPtr.Zero; } }
}
public sealed class NvsfCiFileProgress : IDisposable {
    readonly string root;
    readonly Stopwatch clock = Stopwatch.StartNew();
    System.Collections.Generic.Dictionary<string, (long size, long written)> previous = new(StringComparer.Ordinal);
    System.Collections.Generic.Dictionary<string, (long size, long written)> current = new(StringComparer.Ordinal);
    System.Collections.Generic.IEnumerator<System.IO.DirectoryInfo> ports;
    System.Collections.Generic.IEnumerator<System.IO.FileInfo> files;
    bool baseline = true;
    int directories;
    double lastSample = -250;
    public NvsfCiFileProgress(string path) {
        root = path;
        try { BeginCycle(); } catch { Dispose(); throw; }
    }
    void SelectDirectory(System.IO.DirectoryInfo directory) {
        if (!directory.Exists || (directory.Attributes & System.IO.FileAttributes.ReparsePoint) != 0)
            throw new InvalidOperationException("Owned build log directory is missing or linked.");
        files = directory.EnumerateFiles("*.log").GetEnumerator();
    }
    void BeginCycle() {
        var directory = new System.IO.DirectoryInfo(root);
        directories = 1;
        SelectDirectory(directory);
        ports = directory.EnumerateDirectories().GetEnumerator();
    }
    public bool Consume() {
        if (clock.Elapsed.TotalMilliseconds - lastSample < 250) return false;
        var elapsed = Stopwatch.StartNew();
        bool changed = false;
        // Advance only a small shallow page; a whole buildtree never blocks one guard tick.
        for (int entries = 0; entries < 128 && elapsed.ElapsedMilliseconds < 100; entries++) {
            if (files.MoveNext()) {
                var file = files.Current;
                if (current.Count >= 8192)
                    throw new InvalidOperationException("Owned build log file count exceeded maximum=8192.");
                // EnumerateFiles prepopulates fresh attributes, size and write time; Refresh repeats that I/O.
                if ((file.Attributes & System.IO.FileAttributes.ReparsePoint) != 0)
                    throw new InvalidOperationException("Owned build log may not be linked.");
                var value = (file.Length, file.LastWriteTimeUtc.Ticks);
                current.Add(file.FullName, value);
                if (!baseline)
                    changed |= previous.TryGetValue(file.FullName, out var old) ? old != value : value.Item1 > 0;
            } else {
                files.Dispose(); files = null;
                if (ports.MoveNext()) {
                    if (++directories > 513)
                        throw new InvalidOperationException("Owned build log directory count exceeded maximum=513.");
                    SelectDirectory(ports.Current);
                } else {
                    ports.Dispose(); ports = null;
                    previous = current;
                    current = new(StringComparer.Ordinal);
                    baseline = false;
                    BeginCycle();
                    break;
                }
            }
            if (elapsed.ElapsedMilliseconds >= 1000)
                throw new InvalidOperationException($"Owned build log metadata operation exceeded maximum_ms=1000 elapsed_ms={elapsed.ElapsedMilliseconds} directories={directories} files={current.Count}.");
        }
        if (elapsed.ElapsedMilliseconds >= 1000)
            throw new InvalidOperationException($"Owned build log metadata operation exceeded maximum_ms=1000 elapsed_ms={elapsed.ElapsedMilliseconds}.");
        lastSample = clock.Elapsed.TotalMilliseconds;
        return changed;
    }
    public void Dispose() { files?.Dispose(); ports?.Dispose(); }
}
'@
}
$process = [Diagnostics.Process]::new()
$stdoutWriter = $null
if ($CaptureStdoutPath) {
    $capture = [IO.Path]::GetFullPath($CaptureStdoutPath)
    if (-not $capture.StartsWith($root + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) { throw 'Output capture must remain inside this project.' }
    $stdoutWriter = [IO.StreamWriter]::new($capture, $false, $utf8)
    $stdoutWriter.AutoFlush = $true
}
$job = $null
$group = $null
$runId = [Guid]::NewGuid().ToString('N')
$gate = Join-Path $logs "$runId.gate"
$ready = Join-Path $logs "$runId.ready"
$code = 1
$started = $false
$assigned = $false
$rootExited = $false
$ownedPid = $null
$exitObserved = $null
$exitCode = $null
$fileProgress = $null
$lastFileNotice = 0.0
$stopwatch = [Diagnostics.Stopwatch]::StartNew()
try {
    if ($ProgressDirectory) {
        $progressRoot = [IO.Path]::GetFullPath($ProgressDirectory, $root)
        if (-not $progressRoot.StartsWith($root + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) { throw 'Build progress directory must remain inside this project.' }
        $cursor = $progressRoot
        while ($cursor.Length -gt $root.Length) {
            if ((Test-Path -LiteralPath $cursor) -and ((Get-Item -LiteralPath $cursor -Force).Attributes -band [IO.FileAttributes]::ReparsePoint)) { throw 'Build progress directory ancestors may not be links.' }
            $cursor = [IO.Path]::GetDirectoryName($cursor)
        }
        [IO.Directory]::CreateDirectory($progressRoot) | Out-Null
        $fileProgress = [NvsfCiFileProgress]::new($progressRoot)
    }
    $command = Get-Command $FilePath -CommandType Application -ErrorAction Stop | Select-Object -First 1
    $info = [Diagnostics.ProcessStartInfo]::new()
    $info.WorkingDirectory = [IO.Path]::GetFullPath($WorkingDirectory)
    if (-not (Test-Path -LiteralPath $info.WorkingDirectory -PathType Container)) { throw 'Working directory does not exist.' }
    $info.UseShellExecute = $false
    $info.CreateNoWindow = $true
    $info.RedirectStandardInput = $true
    $info.RedirectStandardOutput = $true
    $info.RedirectStandardError = $true
    $info.StandardOutputEncoding = $utf8
    $info.StandardErrorEncoding = $utf8
    if ($IsWindows) {
        $job = [NvsfCiOwnership]::new()
        $info.FileName = (Get-Command pwsh -CommandType Application -ErrorAction Stop | Select-Object -First 1).Source
        $payload = @'
$ErrorActionPreference='Stop'
[Console]::OutputEncoding=[Text.UTF8Encoding]::new($false)
[IO.File]::WriteAllText($env:NVSF_CI_READY,'ready',[Text.UTF8Encoding]::new($false))
$deadline=[DateTime]::UtcNow.AddSeconds(20)
while(-not [IO.File]::Exists($env:NVSF_CI_GATE)) {
    if([DateTime]::UtcNow -gt $deadline) { throw 'Ownership gate deadline exceeded' }
    [Threading.Thread]::Sleep(20)
}
$argv=@(ConvertFrom-Json -InputObject $env:NVSF_CI_ARGUMENTS)
& $env:NVSF_CI_EXECUTABLE @argv
exit $LASTEXITCODE
'@
        $info.Environment['NVSF_CI_GATE'] = $gate
        $info.Environment['NVSF_CI_READY'] = $ready
        $info.Environment['NVSF_CI_EXECUTABLE'] = $command.Source
        $info.Environment['NVSF_CI_ARGUMENTS'] = ConvertTo-Json -InputObject @($Arguments) -Compress
        foreach ($arg in @('-NoLogo', '-NoProfile', '-NonInteractive', '-EncodedCommand', [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes($payload)))) { $info.ArgumentList.Add($arg) }
    } else {
        # Only bootstrap/control glue uses the hosted Python stdlib; no Python application runtime.
        $info.FileName = (Get-Command python3 -CommandType Application -ErrorAction Stop | Select-Object -First 1).Source
        $payload = @'
import os, sys, time
os.setsid()
gate, ready, exe, *argv = sys.argv[1:]
with open(ready, "w", encoding="utf-8") as stream:
    stream.write("ready")
deadline = time.monotonic() + 20
while not os.path.isfile(gate):
    if time.monotonic() >= deadline:
        raise RuntimeError("Ownership gate deadline exceeded")
    time.sleep(0.02)
os.execv(exe, [exe, *argv])
'@
        foreach ($arg in @('-I', '-u', '-c', $payload, $gate, $ready, $command.Source) + $Arguments) { $info.ArgumentList.Add($arg) }
    }
    $process.StartInfo = $info
    Write-GuardLog 'INFO' "Starting name=$Name executable=$([IO.Path]::GetFileName($command.Source)) cwd=$($info.WorkingDirectory) wall_s=$WallSeconds idle_s=$IdleSeconds workers=2"
    if (-not $process.Start()) { throw 'Process start failed.' }
    $started = $true
    $ownedPid = $process.Id
    $startUtc = $process.StartTime.ToUniversalTime()
    Write-GuardLog 'INFO' "Owned process pid=$ownedPid start_utc=$($startUtc.ToString('o'))"
    if ($IsWindows) { $job.Assign($process); $assigned = $true }
    while (-not (Test-Path -LiteralPath $ready)) {
        if ($process.WaitForExit(20) -or $stopwatch.Elapsed.TotalSeconds -ge 20) { throw 'Process did not establish its ownership boundary.' }
    }
    if (-not $IsWindows) { $group = $process.Id }
    [IO.File]::WriteAllText($gate, 'go', $utf8)
    $process.StandardInput.Close()
    $buffers = @([char[]]::new(4096), [char[]]::new(4096))
    $readers = @($process.StandardOutput, $process.StandardError)
    $reads = @($readers[0].ReadAsync($buffers[0], 0, 4096), $readers[1].ReadAsync($buffers[1], 0, 4096))
    $lastProgress = $stopwatch.Elapsed.TotalSeconds
    while ($true) {
        if ($fileProgress -and $fileProgress.Consume()) {
            $lastProgress = $stopwatch.Elapsed.TotalSeconds
            if (($lastProgress - $lastFileNotice) -ge 30) {
                Write-GuardLog 'INFO' 'Owned native build log changed; file content remains in the build diagnostics.'
                $lastFileNotice = $lastProgress
            }
        }
        for ($i = 0; $i -lt 2; $i++) {
            if ($reads[$i] -and $reads[$i].IsCompleted) {
                $count = $reads[$i].GetAwaiter().GetResult()
                if ($count -gt 0) {
                    $lastProgress = $stopwatch.Elapsed.TotalSeconds
                    $chunk = [string]::new($buffers[$i], 0, $count)
                    if ($i -eq 0 -and $stdoutWriter) {
                        if ($stdoutWriter.BaseStream.Length -gt 8388608) { throw 'Structured output capture exceeded 8 MiB.' }
                        foreach ($secret in $redactions) { $chunk = $chunk.Replace($secret, '[REDACTED]') }
                        $stdoutWriter.Write($chunk)
                    }
                    Write-GuardLog $(if ($i -eq 0) { 'INFO' } else { 'WARNING' }) $chunk.TrimEnd()
                    $reads[$i] = $readers[$i].ReadAsync($buffers[$i], 0, 4096)
                } else { $reads[$i] = $null }
            }
        }
        if (-not $rootExited -and $process.HasExited) {
            $exitCode = $process.ExitCode
            $exitObserved = $stopwatch.Elapsed.TotalSeconds
            $rootExited = $true
            # An exited Windows root remains in job accounting while this Process holds its handle.
            $process.Close()
        }
        if ($rootExited) {
            $descendants = if ($IsWindows) { $job.Active() } else { [int]([NvsfCiOwnership]::SignalGroup($group, 0) -eq 0) }
            if ($descendants -eq 0 -and -not $reads[0] -and -not $reads[1]) { $code = $exitCode; break }
            if (($stopwatch.Elapsed.TotalSeconds - $exitObserved) -ge 2) {
                if ($IsWindows) { $job.Active($true) | Out-Null }
                throw "Owned root exited without bounded stream/tree completion; rejecting leaked process tree. root_exit_code=$exitCode live_tree=$descendants stdout_pending=$([bool]$reads[0]) stderr_pending=$([bool]$reads[1]) live_processes=$(if($IsWindows){$job.LiveProcesses}else{'group-present'})"
            }
        }
        if ($stopwatch.Elapsed.TotalSeconds -ge $WallSeconds -or ($stopwatch.Elapsed.TotalSeconds - $lastProgress) -ge $IdleSeconds) {
            $code = 124
            throw "Command exceeded wall/idle deadline: name=$Name pid=$ownedPid."
        }
        # Bounded scheduling wait, never a progress heartbeat that resets the idle deadline.
        [Threading.Thread]::Sleep(20)
    }
    Write-GuardLog $(if ($code -eq 0) { 'INFO' } else { 'ERROR' }) "Command finished name=$Name exit_code=$code elapsed_s=$([Math]::Round($stopwatch.Elapsed.TotalSeconds, 2))"
} catch {
    if ($code -eq 0) { $code = 1 }
    Write-GuardLog 'ERROR' "$($_.Exception.Message) $($_.ScriptStackTrace)"
} finally {
    try {
        if ($started) {
            if (-not $rootExited -and $process.HasExited) { $process.Close(); $rootExited = $true }
            if ($IsWindows -and $assigned) {
                $remaining = $job.Active()
                if ($remaining -gt 0) {
                    $job.Terminate()
                    if (-not $rootExited) {
                        if (-not $process.WaitForExit(3000)) { throw 'Owned root did not exit after job termination.' }
                        $process.Close(); $rootExited = $true
                    }
                    $cleanup = [Diagnostics.Stopwatch]::StartNew()
                    while ($job.Active() -gt 0 -and $cleanup.Elapsed.TotalSeconds -lt 10) { [Threading.Thread]::Sleep(20) }
                    if ($job.Active() -gt 0) { throw 'Owned Windows job still has live processes after cleanup.' }
                    if ($code -eq 0) { $code = 1; Write-GuardLog 'ERROR' 'Successful command left owned descendants; terminated and rejected.' }
                }
            } elseif ($group) {
                if ([NvsfCiOwnership]::SignalGroup($group, 0) -eq 0) {
                    [NvsfCiOwnership]::SignalGroup($group, 9) | Out-Null
                    $cleanup = [Diagnostics.Stopwatch]::StartNew()
                    while ([NvsfCiOwnership]::SignalGroup($group, 0) -eq 0 -and $cleanup.Elapsed.TotalSeconds -lt 10) { [Threading.Thread]::Sleep(20) }
                    if ([NvsfCiOwnership]::SignalGroup($group, 0) -eq 0) { throw 'Owned Unix process group still exists after cleanup.' }
                    if ($code -eq 0) { $code = 1; Write-GuardLog 'ERROR' 'Successful command left owned descendants; terminated and rejected.' }
                } elseif ([NvsfCiOwnership]::LastError() -ne 3) { throw 'Could not verify owned Unix process group cleanup.' }
            } elseif (-not $rootExited -and -not $process.HasExited) { $process.Kill($true) }
            if (-not $rootExited -and -not $process.WaitForExit(10000)) { throw 'Owned process did not exit after cleanup.' }
            Write-GuardLog 'INFO' "Cleanup verified pid=$ownedPid"
        }
    } catch { $code = 1; [Console]::Error.WriteLine("Cleanup failure: $($_.Exception.Message)") }
    if ($fileProgress) { $fileProgress.Dispose() }
    if ($job) { $job.Dispose() }
    $process.Dispose()
    foreach ($path in @($gate, $ready)) { if (Test-Path -LiteralPath $path) { Remove-Item -LiteralPath $path -Force } }
    if ($stdoutWriter) { $stdoutWriter.Dispose() }
    $writer.Dispose()
}
$global:LASTEXITCODE = $code
if ($code -ne 0) { throw "Guarded command failed: name=$Name exit_code=$code log=$logPath" }
return
