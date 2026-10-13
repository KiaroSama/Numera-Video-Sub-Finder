#requires -Version 7.4
[CmdletBinding()]
param([Parameter(Mandatory)][string]$Executable,[Parameter(Mandatory)][string]$ArgumentsJson)
$ErrorActionPreference='Stop'
if(-not $IsLinux -or $env:GITHUB_ACTIONS -ne 'true'){throw 'Virtual desktop verification requires a hosted Linux runner'}
$process=[Diagnostics.Process]::new()
$process.StartInfo=[Diagnostics.ProcessStartInfo]::new('openbox')
$process.StartInfo.UseShellExecute=$false
$process.StartInfo.RedirectStandardOutput=$true
$process.StartInfo.RedirectStandardError=$true
$process.StartInfo.CreateNoWindow=$true
try {
    if(-not $process.Start()){throw 'Window manager failed startup'}
    $stdout=$process.StandardOutput.ReadToEndAsync();$stderr=$process.StandardError.ReadToEndAsync()
    $deadline=[DateTime]::UtcNow.AddSeconds(5)
    do {
        & wmctrl -m *> $null
        $ready=$LASTEXITCODE -eq 0
        if($process.HasExited){throw 'Window manager exited before readiness'}
        if(-not $ready){[Threading.Thread]::Sleep(20)}
    } while(-not $ready -and [DateTime]::UtcNow -lt $deadline)
    if(-not $ready){throw 'Window manager readiness deadline exceeded'}
    $argv=@(ConvertFrom-Json $ArgumentsJson)
    & $Executable @argv
    if($LASTEXITCODE -ne 0){throw 'Native production GUI verification failed'}
} finally {
    if(-not $process.HasExited){$process.Kill($true)}
    if(-not $process.WaitForExit(3000)){throw 'Owned window manager cleanup failed'}
    $process.Dispose()
}
