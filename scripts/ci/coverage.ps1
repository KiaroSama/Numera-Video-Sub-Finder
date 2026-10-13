#requires -Version 7.4
[CmdletBinding()]
param([ValidateSet('start','report')][string]$Phase)
$ErrorActionPreference='Stop'
Set-StrictMode -Version Latest
if($env:GITHUB_ACTIONS -ne 'true'){throw 'Coverage only runs in GitHub Actions'}
$root=[IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../..'))
$utf8=[Text.UTF8Encoding]::new($false)
if($Phase -eq 'start') {
    & "$PSScriptRoot/guard.ps1" -Name llvm-cov-install -WallSeconds 1200 -IdleSeconds 180 -Executable cargo -Arguments @('install','cargo-llvm-cov','--version','0.9.1','--locked','--root','.ci-work/tools')
    if($LASTEXITCODE -ne 0){throw 'Coverage tool installation failed'}
    $env:PATH="$root/.ci-work/tools/bin$([IO.Path]::PathSeparator)$env:PATH"
    [IO.File]::AppendAllText($env:GITHUB_PATH,"$root/.ci-work/tools/bin`n",$utf8)
    $lines=& cargo llvm-cov show-env --pwsh
    if($LASTEXITCODE -ne 0){throw 'Coverage environment discovery failed'}
    foreach($line in $lines) {
        if($line -notmatch '^\$env:(?<key>[A-Z_0-9]+)="(?<value>(?:`u\{[0-9a-f]+\})*)"$'){throw 'Unexpected coverage environment output'}
        $key=$Matches.key
        $value=[regex]::Replace($Matches.value,'`u\{([0-9a-f]+)\}',{param($m)[char]::ConvertFromUtf32([Convert]::ToInt32($m.Groups[1].Value,16))})
        if($key -notmatch '^(RUSTFLAGS|CARGO_ENCODED_RUSTFLAGS|RUSTC_WRAPPER|RUSTC_WORKSPACE_WRAPPER|LLVM_PROFILE_FILE|CARGO_LLVM_COV(_[A-Z_]+)?|__CARGO_LLVM_COV_RUSTC_WRAPPER(_RUSTFLAGS|_CRATE_NAMES|_PRE_EXISTING|_COVERAGE_TARGET|_HOST)?)$'){throw "Unreviewed coverage variable: $key"}
        [Environment]::SetEnvironmentVariable($key,$value,'Process')
        [IO.File]::AppendAllText($env:GITHUB_ENV,"$key=$value`n",$utf8)
    }
    & cargo llvm-cov clean --workspace
    if($LASTEXITCODE -ne 0){throw 'Coverage clean failed'}
} else {
    & "$PSScriptRoot/guard.ps1" -Name llvm-cov-report -WallSeconds 180 -IdleSeconds 60 -Executable cargo -Arguments @('llvm-cov','report','--lcov','--output-path','.ci-work/coverage.lcov')
    if($LASTEXITCODE -ne 0){throw 'Coverage profile merge/report failed'}
    & "$root/target/release/xtask" coverage --input '.ci-work/coverage.lcov' --output '.ci-work/packages/reports/coverage.json'
    if($LASTEXITCODE -ne 0){throw 'Retained coverage floors failed'}
}
