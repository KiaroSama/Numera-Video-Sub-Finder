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
function Invoke-Contract([string]$Name, [string]$Code, [string]$Capture) {
    $encoded = [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes($Code))
    & $guard -Name "contract-$Name" -WallSeconds 15 -IdleSeconds 5 -FilePath $pwsh -Arguments @('-NoLogo','-NoProfile','-NonInteractive','-EncodedCommand',$encoded) -WorkingDirectory $work -CaptureStdoutPath $Capture
}
try {
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
    [Console]::WriteLine('Guard contracts passed: tests=3 failed=0 skipped=0; Unicode success, nonzero exit and owned descendant rejection/cleanup.')
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
