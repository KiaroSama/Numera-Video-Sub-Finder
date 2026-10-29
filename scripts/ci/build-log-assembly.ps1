# Compilation shares guard type initialization, not the per-command deadline or observer startup.
function Get-BuildLogAssembly {
    $source=Join-Path $PSScriptRoot 'build-log-progress.cs'
    $sourceHash=(Get-FileHash -LiteralPath $source -Algorithm SHA256).Hash
    $runtime="$($PSVersionTable.PSVersion)|$($PSVersionTable.PSEdition)|$([Runtime.InteropServices.RuntimeInformation]::FrameworkDescription)"
    $existing=Get-Variable -Scope Global -Name NvsfBuildLogAssembly -ErrorAction SilentlyContinue
    if ($existing) {
        $record=$existing.Value
        if ($record.root -cne $root -or $record.source -cne $sourceHash -or $record.runtime -cne $runtime -or (Get-Item -LiteralPath (Split-Path -Parent $record.path) -Force).LinkType -or (Get-Item -LiteralPath $record.path -Force).LinkType -or (Get-FileHash -LiteralPath $record.path -Algorithm SHA256).Hash -cne $record.hash) { throw 'Cached observer assembly identity changed; start a fresh guard session.' }
        return $record.path
    }
    $parent=Join-Path $root '.ci-work'
    if ((Get-Item -LiteralPath $parent -Force).LinkType) { throw 'Observer assembly state parent must not be linked.' }
    $directory=Join-Path $parent ('observer-assembly-' + [Guid]::NewGuid().ToString('N'))
    if (Test-Path -LiteralPath $directory) { throw 'Observer assembly directory must be new.' }
    [IO.Directory]::CreateDirectory($directory)|Out-Null
    $assembly=Join-Path $directory 'build-log-progress.dll'
    if (Test-Path -LiteralPath $assembly) { throw 'Refusing to overwrite an observer assembly.' }
    $elapsed=[Diagnostics.Stopwatch]::StartNew()
    Add-Type -LiteralPath $source -OutputAssembly $assembly
    if ((Get-FileHash -LiteralPath $source -Algorithm SHA256).Hash -cne $sourceHash) { throw 'Observer source changed during compilation.' }
    $hash=(Get-FileHash -LiteralPath $assembly -Algorithm SHA256).Hash
    $global:NvsfBuildLogAssembly=@{root=$root;source=$sourceHash;runtime=$runtime;path=$assembly;hash=$hash}
    Write-GuardLog 'INFO' "Observer assembly initialized elapsed_ms=$($elapsed.ElapsedMilliseconds) source_sha256=$sourceHash"
    return $assembly
}
