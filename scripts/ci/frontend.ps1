#requires -Version 7.4
[CmdletBinding()]
param([switch]$E2e)
$ErrorActionPreference='Stop'
$root=[IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../..'))
$saved=@{}
$environment=[Environment]::GetEnvironmentVariables('Process')
# Native LLVM instrumentation cannot be inherited by the separate WASM frontend build.
$names=@('RUSTFLAGS','CARGO_ENCODED_RUSTFLAGS','RUSTC_WRAPPER','RUSTC_WORKSPACE_WRAPPER','LLVM_PROFILE_FILE','CARGO_LLVM_COV','CARGO_LLVM_COV_SHOW_ENV','TRUNK_TOOLS_WASM_BINDGEN','TRUNK_TOOLS_WASM_OPT')+@(Get-ChildItem Env: | Where-Object Name -Like '__CARGO_LLVM_COV_*' | ForEach-Object Name)
try {
    foreach($name in $names) {
        $saved[$name]=$environment[$name]
        Set-Item -LiteralPath "Env:$name" -Value $null
    }
    $arguments=@('build','--release','--locked')
    if($IsWindows -and [Runtime.InteropServices.RuntimeInformation]::ProcessArchitecture -eq 'Arm64') {
        $env:TRUNK_TOOLS_WASM_BINDGEN='0.2.129'
        $env:TRUNK_TOOLS_WASM_OPT='version_133'
    }
    if($E2e){$arguments+=@('--features','e2e')}
    & "$PSScriptRoot/guard.ps1" -Name $(if($E2e){'frontend-e2e'}else{'frontend-production'}) -WallSeconds 1200 -IdleSeconds 180 -Executable trunk -Arguments $arguments -WorkingDirectory (Join-Path $root 'crates/ui')
} finally {
    foreach($name in $saved.Keys){Set-Item -LiteralPath "Env:$name" -Value $saved[$name]}
}
