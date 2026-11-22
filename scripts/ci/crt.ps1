#requires -Version 7.4
function Resolve-BuiltXtask {
    $root = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../..'))
    $ext = if ($IsWindows) { '.exe' } else { '' }
    $candidate = Join-Path $root "target/release/xtask$ext"
    if ([IO.File]::Exists($candidate)) { return $candidate }
    return $null
}
function Get-ReleaseCrtImports([string]$Dependencies) {
    $xtask = Resolve-BuiltXtask
    if ($null -ne $xtask) {
        $tmp = [IO.Path]::GetTempFileName()
        try {
            [IO.File]::WriteAllText($tmp, $Dependencies, [Text.UTF8Encoding]::new($false))
            $out = & $xtask ci-crt --mode imports --input $tmp
            if ($LASTEXITCODE -ne 0) { throw 'Rust ci-crt imports validation failed.' }
            foreach ($line in @($out)) { if (-not [string]::IsNullOrWhiteSpace($line)) { $line.Trim() } }
            return
        } finally { Remove-Item -LiteralPath $tmp -Force -ErrorAction SilentlyContinue }
    }
    if ($Dependencies.Length -gt 1048576) { throw 'PE import listing exceeds its byte budget.' }
    $imports = [regex]::Matches($Dependencies, '(?im)^\s+([A-Za-z0-9_.+-]+\.(?:dll|drv))\s*$')
    if ($imports.Count -lt 1 -or $imports.Count -gt 512) { throw 'PE import listing is empty or exceeds its member budget.' }
    $names = [Collections.Generic.HashSet[string]]::new([StringComparer]::OrdinalIgnoreCase)
    foreach ($match in $imports) {
        $name = $match.Groups[1].Value.ToLowerInvariant()
        if ($name -match '^(vcruntime|msvcp|concrt|vccorlib|ucrtbase).*d\.dll$') { throw "Nonredistributable debug CRT import: $name." }
        if ($name -match '^(concrt140|msvcp140(?:_1|_2|_atomic_wait|_codecvt_ids)?|vccorlib140|vcruntime140(?:_1|_threads)?)\.dll$') {
            if ($names.Add($name)) { $name }
        } elseif ($name -match '^(vcruntime|msvcp|concrt|vccorlib)') { throw "Unknown release CRT import: $name." }
    }
}
function Assert-ReleaseCrtMachine([string]$Path, [string]$Architecture) {
    $xtask = Resolve-BuiltXtask
    if ($null -ne $xtask) {
        & $xtask ci-crt --mode machine --input $Path --arch $Architecture
        if ($LASTEXITCODE -ne 0) { throw 'Rust ci-crt machine validation failed.' }
        return
    }
    if ($Architecture -notin @('arm64', 'x64')) { throw 'Unsupported native CRT architecture.' }
    $file = Get-Item -LiteralPath $Path -Force
    if ($file.PSIsContainer -or ($file.Attributes -band [IO.FileAttributes]::ReparsePoint) -or $file.Length -lt 64 -or $file.Length -gt 16777216) { throw 'CRT input must be a bounded regular PE image.' }
    $reader = [IO.BinaryReader]::new([IO.File]::OpenRead($file.FullName))
    try {
        if ($reader.ReadUInt16() -ne 0x5A4D) { throw 'Invalid redistributable DOS header.' }
        $reader.BaseStream.Position = 0x3C
        $offset = $reader.ReadInt32()
        if ($offset -lt 64 -or $offset -gt $reader.BaseStream.Length - 6) { throw 'Invalid redistributable PE offset.' }
        $reader.BaseStream.Position = $offset
        if ($reader.ReadUInt32() -ne 0x4550) { throw 'Invalid redistributable PE signature.' }
        $machine = $reader.ReadUInt16()
        $expected = if ($Architecture -eq 'arm64') { 0xAA64 } else { 0x8664 }
        if ($machine -ne $expected) { throw ('Redistributable DLL architecture differs: member={0} expected=0x{1:X4} actual=0x{2:X4}.' -f $file.Name, $expected, $machine) }
    } finally { $reader.Dispose() }
}
