# Dot-sourced by guard.ps1 after its shared OS ownership type is loaded.
function Start-BuildLogObserver([string]$Directory,[string]$Contract,[string]$Assembly) {
    $process=[Diagnostics.Process]::new()
    $state=@{process=$process;job=$null;group=$null;started=$false;assigned=$false;read=$null;error=$null;ready=$false;baseline=$false;phase='starting';pid=0}
    try {
        $info=[Diagnostics.ProcessStartInfo]::new()
        $pwsh=(Get-Command pwsh -CommandType Application -ErrorAction Stop | Select-Object -First 1).Source
        $args=@('-NoLogo','-NoProfile','-NonInteractive','-File',(Join-Path $PSScriptRoot 'build-log-observer.ps1'),'-Directory',$Directory,'-Checkout',$root,'-Assembly',$Assembly,'-Contract',$Contract)
        $info.WorkingDirectory=$root
        $info.UseShellExecute=$false
        $info.CreateNoWindow=$true
        $info.RedirectStandardInput=$true
        $info.RedirectStandardOutput=$true
        $info.RedirectStandardError=$true
        $info.StandardOutputEncoding=$utf8
        $info.StandardErrorEncoding=$utf8
        if ($IsWindows) {
            $state.job=[NvsfCiOwnership]::new()
            $info.FileName=$pwsh
            foreach ($arg in $args) { $info.ArgumentList.Add($arg) }
        } else {
            $info.FileName=(Get-Command python3 -CommandType Application -ErrorAction Stop | Select-Object -First 1).Source
            # setsid before READY; exec preserves the same owned PID/group, with no bootstrap file polling.
            foreach ($arg in @('-I','-u','-c','import os,sys; os.setsid(); os.execv(sys.argv[1],sys.argv[1:])',$pwsh)+$args) { $info.ArgumentList.Add($arg) }
        }
        $process.StartInfo=$info
        if (-not $process.Start()) { throw 'Observer start failed.' }
        $state.started=$true
        $state.pid=$process.Id
        if ($IsWindows) { $state.job.Assign($process); $state.assigned=$true }
        $state.read=$process.StandardOutput.ReadLineAsync()
        $state.error=$process.StandardError.ReadLineAsync()
        $startup=[Diagnostics.Stopwatch]::StartNew()
        while (-not $state.read.IsCompleted) {
            if ($startup.Elapsed.TotalSeconds -ge [Math]::Min(20,[Math]::Min($WallSeconds,$IdleSeconds)) -or $process.HasExited) { throw 'Observer ownership readiness failed.' }
            [Threading.Thread]::Sleep(20)
        }
        if ($state.read.GetAwaiter().GetResult() -cne 'READY') { throw 'Invalid observer ownership readiness.' }
        if (-not $IsWindows) { $state.group=$process.Id }
        $state.phase="ready_ms=$($startup.ElapsedMilliseconds)"
        Write-GuardLog 'INFO' "Owned build log observer pid=$($state.pid) $($state.phase)"
        $process.StandardInput.WriteLine('GO')
        $process.StandardInput.Close()
        $state.ready=$true
        $state.read=$process.StandardOutput.ReadLineAsync()
        if ($Contract -ne 'stall') {
            while (-not $state.baseline) {
                $null=Read-BuildLogObserver $state
                if ($startup.Elapsed.TotalSeconds -ge [Math]::Min(20,[Math]::Min($WallSeconds,$IdleSeconds))) { throw "Observer baseline exceeded startup deadline elapsed_ms=$($startup.ElapsedMilliseconds) last_phase=$($state.phase)." }
                [Threading.Thread]::Sleep(20)
            }
        }
        return $state
    } catch {
        try { Stop-BuildLogObserver $state } catch { Write-GuardLog 'ERROR' 'Observer startup cleanup failed.' }
        throw
    }
}
function Read-BuildLogObserver($State) {
    if ($State.error.IsCompleted) {
        $line=$State.error.GetAwaiter().GetResult()
        if ($null -ne $line) { throw 'Build log observer emitted an unexpected diagnostic.' }
    }
    $changed=$false
    # Bounded in-memory pipe work; never filesystem I/O or a wait on the deadline owner.
    for ($i=0;$i -lt 8 -and $State.read.IsCompleted;$i++) {
        $line=$State.read.GetAwaiter().GetResult()
        if ($line -cmatch '^PHASE (loaded_ms|baseline_ms)=([0-9]{1,10})$' -and -not $State.baseline) {
            $State.phase=$line.Substring(6)
            Write-GuardLog 'INFO' "Observer phase $($State.phase)"
        }
        elseif ($line -ceq 'BASELINE' -and -not $State.baseline) { $State.baseline=$true }
        elseif ($line -ceq 'CHANGE' -and $State.baseline) { $changed=$true }
        else { throw 'Build log observer failed or its control pipe closed.' }
        $State.read=$State.process.StandardOutput.ReadLineAsync()
    }
    if ($State.process.HasExited) { throw 'Build log observer exited before its owner.' }
    return $changed
}
function Stop-BuildLogObserver($State) {
    if (-not $State) { return }
    $p=$State.process
    $failure=$null
    if ($State.ready) {
        try { $null=Read-BuildLogObserver $State } catch { $failure=$_.Exception }
    }
    try {
        if ($State.started) {
            if ($IsWindows -and $State.assigned) {
                $State.job.Terminate()
            } elseif ($State.group) {
                if ([NvsfCiOwnership]::SignalGroup($State.group,0) -eq 0) {
                    if ([NvsfCiOwnership]::SignalGroup($State.group,9) -ne 0 -and [NvsfCiOwnership]::LastError() -ne 3) { throw 'Observer group termination failed.' }
                } elseif ([NvsfCiOwnership]::LastError() -ne 3) { throw 'Observer group state unavailable.' }
            } elseif (-not $p.HasExited) { $p.Kill($true) }
            if (-not $p.WaitForExit(3000)) { throw 'Observer root was not reaped within three seconds.' }
            $p.Close()
            $cleanup=[Diagnostics.Stopwatch]::StartNew()
            if ($IsWindows -and $State.assigned) {
                while ($State.job.Active() -gt 0 -and $cleanup.Elapsed.TotalSeconds -lt 3) { [Threading.Thread]::Sleep(20) }
                if ($State.job.Active() -gt 0) { throw 'Observer job still has live processes.' }
            } elseif ($State.group) {
                while ([NvsfCiOwnership]::SignalGroup($State.group,0) -eq 0 -and $cleanup.Elapsed.TotalSeconds -lt 3) { [Threading.Thread]::Sleep(20) }
                if ([NvsfCiOwnership]::SignalGroup($State.group,0) -eq 0 -or [NvsfCiOwnership]::LastError() -ne 3) { throw 'Observer process group cleanup could not be verified.' }
            }
            Write-GuardLog 'INFO' "Observer cleanup verified pid=$($State.pid)"
        }
        if ($failure) { throw $failure }
    } finally {
        if ($State.job) { $State.job.Dispose() }
        $p.Dispose()
    }
}
