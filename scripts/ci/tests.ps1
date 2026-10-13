#requires -Version 7.4
[CmdletBinding()]
param([Parameter(Mandatory)][string]$TargetId)
$ErrorActionPreference='Stop'
Set-StrictMode -Version Latest
$root=[IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../..'))
$utf8=[Text.UTF8Encoding]::new($false)
$reports=Join-Path $root '.ci-work/packages/reports'
[IO.Directory]::CreateDirectory($reports)|Out-Null
$rows=@()
foreach($entry in @(
    @{Name='core-app-xtask';Args=@('test','--locked','--no-fail-fast','-p','nvsf-core','-p','nvsf-app','-p','xtask','--','--test-threads=2')},
    @{Name='desktop';Args=@('test','--locked','-p','nvsf-desktop','--features','e2e','--','--test-threads=2')},
    @{Name='ui';Args=@('test','--locked','-p','nvsf-ui','--','--test-threads=2')}
)) {
    $capture=Join-Path $root ".ci-work/captures/tests-$($entry.Name).txt"
    & "$PSScriptRoot/guard.ps1" -Name "test-$($entry.Name)" -WallSeconds 1200 -IdleSeconds 180 -Executable cargo -Arguments $entry.Args -CaptureStdoutPath $capture
    if($LASTEXITCODE -ne 0){throw "Required suite failed: $($entry.Name)"}
    $text=[IO.File]::ReadAllText($capture,$utf8)
    $matches=[regex]::Matches($text,'test result: ok\. (\d+) passed; (\d+) failed; (\d+) ignored; (\d+) measured; (\d+) filtered out')
    if($matches.Count -eq 0){throw "No actual libtest result: $($entry.Name)"}
    $passed=0
    foreach($match in $matches){
        $passed += [int]$match.Groups[1].Value
        foreach($group in 2,3,5){if([int]$match.Groups[$group].Value -ne 0){throw "Failed/ignored/filtered required tests: $($entry.Name)"}}
    }
    if($passed -eq 0){throw "Zero-test suite: $($entry.Name)"}
    $rows+=@{id=$entry.Name;tests=$passed;failed=0;skipped=0;passed=$true}
}
$record=@{schema=1;source_sha=$env:GITHUB_SHA;target=$TargetId;passed=$true;tests=($rows|Measure-Object tests -Sum).Sum;failed=0;skipped=0;suites=$rows}
[IO.File]::WriteAllText((Join-Path $reports "$TargetId-suites.json"),($record|ConvertTo-Json -Depth 8),$utf8)
