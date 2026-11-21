# Compilation shares guard initialization, not the per-command deadline or observer startup.
function Get-BuildLogAssembly {
    $source=Join-Path $PSScriptRoot 'build_log_observer.rs'
    $sourceHash=(Get-FileHash -LiteralPath $source -Algorithm SHA256).Hash
    $rustc=(Get-Command rustc -CommandType Application -ErrorAction Stop | Select-Object -First 1).Source
    $rustcVer=(& $rustc --version 2>&1 | Out-String).Trim()
    if ($LASTEXITCODE -ne 0 -or [string]::IsNullOrWhiteSpace($rustcVer)) { throw 'rustc unavailable for observer compilation.' }
    $runtime="$rustcVer|$([Runtime.InteropServices.RuntimeInformation]::OSArchitecture)"
    $existing=Get-Variable -Scope Global -Name NvsfBuildLogAssembly -ErrorAction SilentlyContinue
    if ($existing) {
        $record=$existing.Value
        if ($record.root -cne $root -or $record.source -cne $sourceHash -or $record.runtime -cne $runtime -or (Get-Item -LiteralPath (Split-Path -Parent $record.path) -Force).LinkType -or (Get-Item -LiteralPath $record.path -Force).LinkType -or (Get-FileHash -LiteralPath $record.path -Algorithm SHA256).Hash -cne $record.hash) { throw 'Cached observer binary identity changed; start a fresh guard session.' }
        return $record.path
    }
    $parent=Join-Path $root '.ci-work'
    if ((Get-Item -LiteralPath $parent -Force).LinkType) { throw 'Observer binary state parent must not be linked.' }
    $directory=Join-Path $parent ('observer-assembly-' + [Guid]::NewGuid().ToString('N'))
    if (Test-Path -LiteralPath $directory) { throw 'Observer binary directory must be new.' }
    [IO.Directory]::CreateDirectory($directory)|Out-Null
    $binaryName=if ($IsWindows) { 'build-log-observer.exe' } else { 'build-log-observer' }
    $assembly=Join-Path $directory $binaryName
    if (Test-Path -LiteralPath $assembly) { throw 'Refusing to overwrite an observer binary.' }
    $elapsed=[Diagnostics.Stopwatch]::StartNew()
    & $rustc --edition 2024 -O --crate-name build_log_observer -o $assembly $source
    if ($LASTEXITCODE -ne 0 -or -not (Test-Path -LiteralPath $assembly -PathType Leaf)) { throw 'Observer Rust compilation failed.' }
    if ((Get-FileHash -LiteralPath $source -Algorithm SHA256).Hash -cne $sourceHash) { throw 'Observer source changed during compilation.' }
    $hash=(Get-FileHash -LiteralPath $assembly -Algorithm SHA256).Hash
    $global:NvsfBuildLogAssembly=@{root=$root;source=$sourceHash;runtime=$runtime;path=$assembly;hash=$hash}
    Write-GuardLog 'INFO' "Observer assembly initialized elapsed_ms=$($elapsed.ElapsedMilliseconds) source_sha256=$sourceHash"
    return $assembly
}
