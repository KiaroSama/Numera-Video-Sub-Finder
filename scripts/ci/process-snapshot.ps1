#requires -Version 7.4
function ConvertFrom-MacProcessSnapshot([string]$Text, [int]$Group) {
    if ($Group -le 1 -or $Text.Length -gt 65536) { throw 'Invalid owned process snapshot bounds.' }
    $rows = @()
    foreach ($line in $Text -split "`n") {
        if (-not $line.Trim()) { continue }
        if ($rows.Count -ge 256 -or $line -notmatch '^\s*(\d+)\s+(\d+)\s+(\d+)\s+([A-Za-z+<>-]{1,16})\s+([0-9:.+-]{1,32})\s+(\d+)\s+([A-Za-z0-9_.+-]{1,64})\s*$') { throw 'Unrecognized owned process snapshot row.' }
        $processId = [int]$Matches[1]; $parentId = [int]$Matches[2]; $groupId = [int]$Matches[3]
        if ($processId -le 0 -or $parentId -lt 0 -or $groupId -ne $Group) { throw 'Process snapshot escaped the owned group.' }
        $rows += [ordered]@{ pid=$processId; ppid=$parentId; pgid=$groupId; state=$Matches[4]; cpu_time=$Matches[5]; rss_kib=[long]$Matches[6]; executable=$Matches[7] }
    }
    return $rows
}

function Write-MacProcessSnapshot([int]$Group, [string]$Destination) {
    if (-not $IsMacOS -or $env:GITHUB_ACTIONS -ne 'true' -or $Group -le 1) { throw 'Process snapshot requires an owned hosted macOS group.' }
    $probe = [Diagnostics.Process]::new()
    $probe.StartInfo = [Diagnostics.ProcessStartInfo]::new('/bin/ps')
    $probe.StartInfo.UseShellExecute = $false
    $probe.StartInfo.CreateNoWindow = $true
    $probe.StartInfo.RedirectStandardOutput = $true
    $probe.StartInfo.RedirectStandardError = $true
    $probe.StartInfo.RedirectStandardInput = $true
    $probe.StartInfo.StandardOutputEncoding = [Text.UTF8Encoding]::new($false, $true)
    $probe.StartInfo.StandardErrorEncoding = [Text.UTF8Encoding]::new($false, $true)
    $probe.StartInfo.Environment['LC_ALL'] = 'C'
    foreach ($arg in @('-g', [string]$Group, '-o', 'pid=,ppid=,pgid=,state=,time=,rss=,ucomm=')) { $probe.StartInfo.ArgumentList.Add($arg) }
    $started = $false
    try {
        if (-not $probe.Start()) { throw 'Owned process snapshot could not start.' }
        $started = $true
        $probe.StandardInput.Close()
        $buffers = @([char[]]::new(4096), [char[]]::new(4096))
        $readers = @($probe.StandardOutput, $probe.StandardError)
        $reads = @($readers[0].ReadAsync($buffers[0],0,4096), $readers[1].ReadAsync($buffers[1],0,4096))
        $text = [Text.StringBuilder]::new()
        $bytes = 0; $stderr = $false
        $clock = [Diagnostics.Stopwatch]::StartNew()
        while ($true) {
            for ($i=0; $i -lt 2; $i++) {
                if ($reads[$i] -and $reads[$i].IsCompleted) {
                    $count = $reads[$i].GetAwaiter().GetResult()
                    if ($count -gt 0) {
                        $bytes += [Text.Encoding]::UTF8.GetByteCount($buffers[$i],0,$count)
                        if ($bytes -gt 65536) { throw 'Owned process snapshot exceeded 64 KiB.' }
                        if ($i -eq 0) { $null = $text.Append($buffers[$i],0,$count) } else { $stderr = $true }
                        $reads[$i] = $readers[$i].ReadAsync($buffers[$i],0,4096)
                    } else { $reads[$i] = $null }
                }
            }
            if ($probe.HasExited -and -not $reads[0] -and -not $reads[1]) { break }
            if ($clock.ElapsedMilliseconds -ge 1000) { throw 'Owned process snapshot exceeded one second.' }
            [Threading.Thread]::Sleep(10)
        }
        if ($probe.ExitCode -ne 0 -or $stderr) { throw 'Owned process snapshot unavailable.' }
        $record = [ordered]@{ schema=1; owned_group=$Group; processes=@(ConvertFrom-MacProcessSnapshot $text.ToString() $Group); limitation='One failure-only snapshot; cumulative CPU time is not proof of progress or a linker root cause.' }
        [IO.File]::WriteAllText($Destination,($record|ConvertTo-Json -Depth 5),[Text.UTF8Encoding]::new($false))
    } finally {
        try {
            if ($started) {
                if (-not $probe.HasExited) { $probe.Kill() }
                if (-not $probe.WaitForExit(1000)) { throw 'Owned snapshot helper did not exit.' }
            }
        } finally { $probe.Dispose() }
    }
}
