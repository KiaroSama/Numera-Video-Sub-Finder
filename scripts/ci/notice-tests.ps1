#requires -Version 7.5
[CmdletBinding()]
param()
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
if ($env:GITHUB_ACTIONS -ne 'true' -or $env:RUNNER_ENVIRONMENT -ne 'github-hosted') {
    throw 'Native notice contracts run only on GitHub-hosted Actions.'
}
. (Join-Path $PSScriptRoot 'notices.ps1')
$root = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../..'))
$work = Join-Path $root ".ci-work/notice-contract-$([Guid]::NewGuid().ToString('N'))"
try {
    $source = Join-Path $work 'source/1033'
    $destination = Join-Path $work 'destination'
    [IO.Directory]::CreateDirectory($source) | Out-Null
    $bytes = [byte[]]@(0x7B,0x5C,0x72,0x74,0x66,0x31,0x20,0xE9,0x7D)
    $original = Join-Path $source '.NET_Framework_EULA.rtf'
    [IO.File]::WriteAllBytes($original, $bytes)
    [IO.File]::WriteAllBytes((Join-Path $source 'ThirdPartyNotices.txt'), $bytes)
    $files = @(Copy-VisualStudioNotices (Join-Path $work 'source') $destination)
    if ($files.Count -ne 2 -or $files -cnotcontains '1033/DOTNET_Framework_EULA.rtf' -or $files -cnotcontains '1033/ThirdPartyNotices.txt') {
        throw 'Native notice destination inventory differs from its contract.'
    }
    foreach ($path in @($original, (Join-Path $destination '1033/DOTNET_Framework_EULA.rtf'))) {
        if ([Convert]::ToHexString([IO.File]::ReadAllBytes($path)) -cne [Convert]::ToHexString($bytes)) { throw 'Notice bytes changed.' }
    }
    $again = @(Copy-VisualStudioNotices (Join-Path $work 'source') $destination)
    if (($files -join '|') -cne ($again -join '|')) { throw 'Identical notice copying is not idempotent.' }
    [IO.File]::WriteAllBytes((Join-Path $destination '1033/DOTNET_Framework_EULA.rtf'), [byte[]]@(1))
    $rejected = $false
    try { $null = Copy-VisualStudioNotices (Join-Path $work 'source') $destination }
    catch { if ($_.Exception.Message -cne 'Visual Studio notice destination bytes collide.') { throw }; $rejected = $true }
    if (-not $rejected) { throw 'Conflicting notice bytes were accepted.' }
    [IO.File]::WriteAllBytes((Join-Path $destination '1033/DOTNET_Framework_EULA.rtf'), $bytes)
    [IO.File]::WriteAllBytes((Join-Path $source '.private-fixture.txt'), $bytes)
    $rejected = $false
    try { $null = Copy-VisualStudioNotices (Join-Path $work 'source') $destination }
    catch { if ($_.Exception.Message -cne 'Unexpected Visual Studio notice path.') { throw }; $rejected = $true }
    if (-not $rejected -or [IO.File]::Exists((Join-Path $destination '1033/.private-fixture.txt'))) { throw 'Unexpected hidden notice was copied.' }
    . (Join-Path $PSScriptRoot 'crt.ps1')
    $imports = @(Get-ReleaseCrtImports "    KERNEL32.dll`n    VCRUNTIME140.dll`n    msvcp140.dll`n    VCRUNTIME140.dll`n")
    if (($imports -join '|') -cne 'vcruntime140.dll|msvcp140.dll') { throw 'Imported release CRT selection differs.' }
    foreach ($listing in @('', '    vcruntime140d.dll', '    msvcp999.dll')) {
        $rejected = $false
        try { $null = Get-ReleaseCrtImports $listing } catch { $rejected = $true }
        if (-not $rejected) { throw 'Invalid or debug CRT import accepted.' }
    }
    $pe = [byte[]]::new(134)
    $pe[0] = 0x4D; $pe[1] = 0x5A; $pe[0x3C] = 128
    $pe[128] = 0x50; $pe[129] = 0x45; $pe[132] = 0x64; $pe[133] = 0xAA
    $crt = Join-Path $work 'fixture-crt.dll'
    [IO.File]::WriteAllBytes($crt, $pe)
    Assert-ReleaseCrtMachine $crt 'arm64'
    $rejected = $false
    try { Assert-ReleaseCrtMachine $crt 'x64' } catch {
        if ($_.Exception.Message -cne 'Redistributable DLL architecture differs: member=fixture-crt.dll expected=0x8664 actual=0xAA64.') { throw }
        $rejected = $true
    }
    if (-not $rejected) { throw 'Wrong CRT architecture accepted.' }
    $pe[0x3C] = 255; [IO.File]::WriteAllBytes($crt, $pe)
    $rejected = $false
    try { Assert-ReleaseCrtMachine $crt 'arm64' } catch { $rejected = $true }
    if (-not $rejected) { throw 'Invalid CRT PE offset accepted.' }
    [Console]::WriteLine('Native notice/CRT contracts passed: tests=7 failed=0 skipped=0; exact bytes, collision, hidden paths, import selection and strict PE admission.')
    $global:LASTEXITCODE = 0
} finally {
    if (Test-Path -LiteralPath $work) { Remove-Item -LiteralPath $work -Recurse -Force }
}
