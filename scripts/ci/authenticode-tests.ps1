# Run only through the existing bounded Windows Actions controller.
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
if ($env:GITHUB_ACTIONS -ne 'true' -or -not $IsWindows) {
    throw 'Authenticode tests require Windows GitHub Actions.'
}
. (Join-Path $PSScriptRoot 'authenticode.ps1')
function Expect-Rejection([scriptblock]$Action) {
    $rejected = $false
    try { & $Action | Out-Null } catch { $rejected = $true }
    if (-not $rejected) { throw 'Expected Authenticode rejection did not occur.' }
}
# Structural fixture is not a signed executable or mathematical signature proof.
function New-PeFixture([bool]$Signed) {
    $b = [byte[]]::new($(if ($Signed) { 528 } else { 512 }))
    $b[0] = 0x4d; $b[1] = 0x5a
    [BitConverter]::GetBytes([uint32]128).CopyTo($b, 60)
    $b[128] = 0x50; $b[129] = 0x45
    [BitConverter]::GetBytes([uint16]0x8664).CopyTo($b, 132)
    [BitConverter]::GetBytes([uint16]1).CopyTo($b, 134)
    [BitConverter]::GetBytes([uint16]240).CopyTo($b, 148)
    [BitConverter]::GetBytes([uint16]0x20b).CopyTo($b, 152)
    [BitConverter]::GetBytes([uint32]512).CopyTo($b, 212)
    [BitConverter]::GetBytes([uint32]16).CopyTo($b, 260)
    if ($Signed) {
        [BitConverter]::GetBytes([uint32]123).CopyTo($b, 216)
        [BitConverter]::GetBytes([uint32]512).CopyTo($b, 296)
        [BitConverter]::GetBytes([uint32]16).CopyTo($b, 300)
        [BitConverter]::GetBytes([uint32]10).CopyTo($b, 512)
        [BitConverter]::GetBytes([uint16]0x200).CopyTo($b, 516)
        [BitConverter]::GetBytes([uint16]2).CopyTo($b, 518)
        $b[520] = 0x30; $b[521] = 0
    }
    return ,$b
}
$root = Join-Path $env:GITHUB_WORKSPACE '.ci-work/authenticode-structural-tests'
if (Test-Path -LiteralPath $root) { throw 'Structural fixture directory already exists.' }
[IO.Directory]::CreateDirectory($root) | Out-Null
try {
    $plain = Join-Path $root 'plain 日本.exe'
    $signed = Join-Path $root 'signed 日本.exe'
    [IO.File]::WriteAllBytes($plain, (New-PeFixture $false))
    [IO.File]::WriteAllBytes($signed, (New-PeFixture $true))
    $a = Get-UnsignedPeBytes -Path $plain
    $b = Get-UnsignedPeBytes -Path $signed
    if ([Convert]::ToHexString($a) -cne [Convert]::ToHexString($b)) {
        throw 'Exact signature-only normalization differs.'
    }
    foreach ($offset in @(0, 60, 128, 152, 260, 296, 300, 512, 516, 518, 527)) {
        $bad = New-PeFixture $true
        $bad[$offset] = $bad[$offset] -bxor 0xff
        [IO.File]::WriteAllBytes($signed, $bad)
        Expect-Rejection { Get-UnsignedPeBytes -Path $signed }
    }
    $bad = New-PeFixture $true
    $bad[450] = 0x7f
    [IO.File]::WriteAllBytes($signed, $bad)
    if ([Convert]::ToHexString((Get-UnsignedPeBytes -Path $signed)) -ceq [Convert]::ToHexString($a)) {
        throw 'Normalization hid an unrelated byte mutation.'
    }
    Expect-Rejection { Assert-DevelopmentSignature -Path $plain -CertificateSha256 'invalid' }
    Expect-Rejection { Assert-DevelopmentSignature -Path $plain -CertificateSha256 ('0' * 64) }
    Write-Output 'Authenticode structural normalization and invalid-input checks passed; actual signed-file checks remain separate.'
} finally {
    # Only exact owned synthetic fixtures, never a caller-provided path.
    foreach ($name in @('plain 日本.exe', 'signed 日本.exe')) {
        $file = Join-Path $root $name
        if (Test-Path -LiteralPath $file) { [IO.File]::Delete($file) }
    }
    [IO.Directory]::Delete($root)
}
