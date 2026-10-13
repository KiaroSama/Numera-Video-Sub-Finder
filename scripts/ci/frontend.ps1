#requires -Version 7.4
[CmdletBinding()]
param([switch]$E2e)
$ErrorActionPreference='Stop'
$root=[IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../..'))
$saved=@{}
# Native LLVM instrumentation cannot be inherited by the separate WASM frontend build.
$names=@('RUSTFLAGS','CARGO_ENCODED_RUSTFLAGS','RUSTC_WRAPPER','RUSTC_WORKSPACE_WRAPPER','LLVM_PROFILE_FILE','CARGO_LLVM_COV','CARGO_LLVM_COV_SHOW_ENV')+@(Get-ChildItem Env: | Where-Object Name -Like '__CARGO_LLVM_COV_*' | ForEach-Object Name)
foreach($name in $names) {
    $saved[$name]=[Environment]::GetEnvironmentVariable($name,'Process')
    [Environment]::SetEnvironmentVariable($name,$null,'Process')
}
try {
    $arguments=@('build','--release')
    if($E2e){$arguments+=@('--features','e2e')}
    & "$PSScriptRoot/guard.ps1" -Name $(if($E2e){'frontend-e2e'}else{'frontend-production'}) -WallSeconds 1200 -IdleSeconds 180 -Executable trunk -Arguments $arguments -WorkingDirectory (Join-Path $root 'crates/ui')
} finally {
    foreach($name in $saved.Keys){[Environment]::SetEnvironmentVariable($name,$saved[$name],'Process')}
}
