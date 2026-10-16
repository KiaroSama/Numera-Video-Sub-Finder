#requires -Version 7.5
[CmdletBinding()]
param()
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
if ($env:GITHUB_ACTIONS -ne 'true' -or $env:RUNNER_ENVIRONMENT -ne 'github-hosted') {
    throw 'Frontend environment contracts run only on GitHub-hosted Actions.'
}
$root = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../..'))
$work = Join-Path $root ".ci-work/frontend-contract-$([Guid]::NewGuid().ToString('N'))"
$scriptDirectory = Join-Path $work 'scripts/ci'
$utf8 = [Text.UTF8Encoding]::new($false, $true)
$isolatedNames = @('RUSTFLAGS','CARGO_ENCODED_RUSTFLAGS','RUSTC_WRAPPER','RUSTC_WORKSPACE_WRAPPER','LLVM_PROFILE_FILE','CARGO_LLVM_COV','CARGO_LLVM_COV_SHOW_ENV','TRUNK_TOOLS_WASM_BINDGEN','TRUNK_TOOLS_WASM_OPT','__CARGO_LLVM_COV_FRONTEND_CONTRACT')
$original = [Environment]::GetEnvironmentVariables('Process')
try {
    [IO.Directory]::CreateDirectory($scriptDirectory) | Out-Null
    [IO.Directory]::CreateDirectory((Join-Path $work 'crates/ui')) | Out-Null
    Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'frontend.ps1') -Destination $scriptDirectory
    # Only the expensive external build boundary is inert; the real frontend script runs unchanged.
    $boundary = @'
[CmdletBinding()]
param($Name, $WallSeconds, $IdleSeconds, $Executable, $Arguments, $WorkingDirectory)
$ErrorActionPreference = 'Stop'
if ($Executable -cne 'trunk' -or $WallSeconds -ne 1200 -or $IdleSeconds -ne 180) { throw 'Frontend build boundary changed.' }
$expected = @('build','--release','--locked')
if ($Name -ceq 'frontend-e2e') { $expected += @('--features','e2e') }
elseif ($Name -cne 'frontend-production') { throw 'Unexpected frontend profile.' }
if (($Arguments -join '|') -cne ($expected -join '|')) { throw 'Frontend arguments changed.' }
if ($WorkingDirectory -cne (Join-Path $work 'crates/ui')) { throw 'Frontend working directory changed.' }
$process = [Environment]::GetEnvironmentVariables('Process')
foreach ($key in $isolatedNames) {
    if ($IsWindows -and [Runtime.InteropServices.RuntimeInformation]::ProcessArchitecture -eq 'Arm64' -and $key -like 'TRUNK_TOOLS_*') {
        $version = if ($key -ceq 'TRUNK_TOOLS_WASM_BINDGEN') { '0.2.129' } else { 'version_133' }
        if (-not $process.Contains($key) -or $process[$key] -cne $version) { throw 'Native ARM helper version changed.' }
    } elseif ($process.Contains($key)) { throw "Frontend inherited an isolated variable: $key" }
}
[IO.File]::WriteAllText((Join-Path $work 'boundary-reached'), $Name, $utf8)
if ($reject) { throw 'Controlled frontend build failure.' }
$global:LASTEXITCODE = 0
'@
    [IO.File]::WriteAllText((Join-Path $scriptDirectory 'guard.ps1'), $boundary, $utf8)
    $cases = 0
    foreach ($e2e in @($false, $true)) {
        foreach ($reject in @($false, $true)) {
            foreach ($name in $isolatedNames) { Set-Item -LiteralPath "Env:$name" -Value $null }
            Set-Item -LiteralPath 'Env:CARGO_ENCODED_RUSTFLAGS' -Value ''
            Set-Item -LiteralPath 'Env:RUSTC_WRAPPER' -Value 'owned-wrapper-sentinel'
            Set-Item -LiteralPath 'Env:LLVM_PROFILE_FILE' -Value 'owned 日本 فارسی.profraw'
            Set-Item -LiteralPath 'Env:__CARGO_LLVM_COV_FRONTEND_CONTRACT' -Value 'owned-coverage-sentinel'
            if ($e2e) {
                Set-Item -LiteralPath 'Env:TRUNK_TOOLS_WASM_BINDGEN' -Value 'owned-helper-sentinel'
                Set-Item -LiteralPath 'Env:TRUNK_TOOLS_WASM_OPT' -Value ''
            }
            $before = [Environment]::GetEnvironmentVariables('Process')
            if (-not $before.Contains('CARGO_ENCODED_RUSTFLAGS') -or $before['CARGO_ENCODED_RUSTFLAGS'] -cne '' -or $before.Contains('RUSTFLAGS')) {
                throw 'Environment fixture did not create distinct absent and empty states.'
            }
            $receipt = Join-Path $work 'boundary-reached'
            if (Test-Path -LiteralPath $receipt) { Remove-Item -LiteralPath $receipt }
            $caught = $false
            try { & (Join-Path $scriptDirectory 'frontend.ps1') -E2e:$e2e }
            catch {
                if (-not $reject -or $_.Exception.Message -cne 'Controlled frontend build failure.') { throw }
                $caught = $true
            }
            if ($caught -ne $reject -or -not [IO.File]::Exists($receipt)) { throw 'Frontend did not reach and propagate the controlled build boundary.' }
            $profile = if ($e2e) { 'frontend-e2e' } else { 'frontend-production' }
            if ([IO.File]::ReadAllText($receipt, $utf8) -cne $profile) { throw 'Frontend profile receipt mismatch.' }
            $after = [Environment]::GetEnvironmentVariables('Process')
            foreach ($name in $isolatedNames) {
                if ($before.Contains($name) -ne $after.Contains($name) -or ($before.Contains($name) -and $before[$name] -cne $after[$name])) {
                    throw "Frontend did not restore exact environment state: $name"
                }
            }
            $cases++
        }
    }
    [Console]::WriteLine("Frontend environment contracts passed: tests=$cases failed=0 skipped=0; actual script, both profiles, absent/empty/Unicode values and failure restoration; no build executed.")
    $global:LASTEXITCODE = 0
} finally {
    foreach ($name in $isolatedNames) {
        $value = if ($original.Contains($name)) { [string]$original[$name] } else { $null }
        Set-Item -LiteralPath "Env:$name" -Value $value
    }
    if (Test-Path -LiteralPath $work) { Remove-Item -LiteralPath $work -Recurse -Force }
}
