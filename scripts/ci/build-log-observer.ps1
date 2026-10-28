#requires -Version 7.4
[CmdletBinding()]
param([Parameter(Mandatory)][string]$Directory,[Parameter(Mandatory)][string]$Checkout,[ValidateSet('none','stall')][string]$Contract='none')
$ErrorActionPreference='Stop'
Set-StrictMode -Version Latest
if ($env:GITHUB_ACTIONS -ne 'true' -or $env:RUNNER_ENVIRONMENT -ne 'github-hosted') { throw 'Build log observation is hosted-only.' }
[Console]::OutputEncoding=[Text.UTF8Encoding]::new($false)
# No filesystem access, compilation or descendants until the parent establishes OS ownership.
[Console]::WriteLine('READY')
if ([Console]::ReadLine() -cne 'GO') { exit 2 }
try {
    if ($Contract -ne 'none' -and -not $Directory.StartsWith((Join-Path $Checkout '.ci-work/guard-contract-'),[StringComparison]::Ordinal)) { throw 'Invalid observer contract scope.' }
    Add-Type -Path (Join-Path $PSScriptRoot 'build-log-progress.cs')
    if ($Contract -eq 'stall') { [Threading.Thread]::Sleep(60000); exit 3 }
    $sampler=[NvsfCiFileProgress]::new($Directory,$Checkout)
    try {
        $initialized=$false
        while ($true) {
            $changed=$sampler.Consume()
            if (-not $initialized -and $sampler.Initialized) { [Console]::WriteLine('BASELINE'); $initialized=$true }
            if ($changed) { [Console]::WriteLine('CHANGE') }
            # Cooperative scheduling only: never a heartbeat or parent deadline reset.
            [Threading.Thread]::Sleep(20)
        }
    } finally { $sampler.Dispose() }
} catch {
    # Do not send file names/content/private exceptions through the bounded control protocol.
    [Console]::WriteLine('ERROR')
    exit 1
}
