#requires -Version 7.4
[CmdletBinding()]
param([Parameter(Mandatory)][ValidatePattern('^(linux|windows|macos)-(x64|arm64)$')][string]$TargetId)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
[Console]::OutputEncoding = [Text.UTF8Encoding]::new($false)
if ($env:GITHUB_ACTIONS -ne 'true' -or $env:RUNNER_ENVIRONMENT -ne 'github-hosted') {
    throw 'Native dependency provisioning is restricted to GitHub-hosted Actions runners.'
}
$root = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../..'))
$targets = Get-Content -LiteralPath (Join-Path $root 'ci/targets.json') -Raw -Encoding utf8 | ConvertFrom-Json
$matches = @($targets.targets | Where-Object id -eq $TargetId)
if ($targets.scope -ne 'desktop-only' -or $matches.Count -ne 1) { throw 'Unknown or ambiguous required desktop target.' }
$target = $matches[0]
$hostOs = if ($IsWindows) { 'windows' } elseif ($IsMacOS) { 'macos' } else { 'linux' }
$hostArch = [Runtime.InteropServices.RuntimeInformation]::ProcessArchitecture.ToString().ToLowerInvariant()
if ($target.os -ne $hostOs -or $target.arch -ne $hostArch) { throw 'Native target does not match the hosted runner process OS/architecture.' }
$work = Join-Path $root '.ci-work/native'
if ($work.Contains(' ')) { throw 'Pinned FFmpeg port rejects spaces in its source build path; use a space-free hosted checkout.' }
$runtimes = Join-Path $root '.ci-work/runtimes'
$notices = Join-Path $root '.ci-work/native-notices'
$logs = Join-Path $root '.ci-work/logs'
$overlay = Join-Path $root 'ci/triplets'
$guard = Join-Path $PSScriptRoot 'guard.ps1'
foreach ($path in @($work, $runtimes, $notices, $logs, (Join-Path $work 'temp'), (Join-Path $work 'downloads'), (Join-Path $work 'binary-cache'))) {
    [IO.Directory]::CreateDirectory($path) | Out-Null
}
$utf8 = [Text.UTF8Encoding]::new($false)
$logPath = Join-Path $logs ("native_{0}_UTC.log" -f [DateTime]::UtcNow.ToString('yyyy-MM-dd_HH-mm-ss'))
if (Test-Path -LiteralPath $logPath) { $logPath = $logPath.Replace('.log', "_$([Guid]::NewGuid().ToString('N')).log") }
$logger = [IO.StreamWriter]::new([IO.FileStream]::new($logPath, [IO.FileMode]::CreateNew), $utf8)
$logger.AutoFlush = $true
function Write-NativeLog([string]$Level, [string]$Text) {
    $line = '[{0}] [{1}] [NATIVE-CI] {2}' -f [DateTime]::UtcNow.ToString('yyyy-MM-dd HH:mm:ss UTC'), $Level, $Text
    $logger.WriteLine($line)
    [Console]::Error.WriteLine($line)
}
$sequence = 0
function Invoke-NativeCommand {
    param([string]$Name, [string]$Executable, [string[]]$Arguments, [int]$Wall = 300, [int]$Idle = 120, [switch]$Capture, [string]$ProgressDirectory)
    $script:sequence++
    $parameters = @{ Name = "$Name-$sequence"; WallSeconds = $Wall; IdleSeconds = $Idle; Executable = $Executable; Arguments = $Arguments; WorkingDirectory = $root }
    $capturePath = Join-Path $logs "$Name-$sequence.stdout"
    if ($Capture) { $parameters.CaptureStdoutPath = $capturePath }
    if ($ProgressDirectory) { $parameters.ProgressDirectory = $ProgressDirectory }
    Write-NativeLog 'INFO' "Command stage=$Name wall_s=$Wall idle_s=$Idle"
    & $guard @parameters
    if ($LASTEXITCODE -ne 0) { throw "Native stage failed: $Name (exit=$LASTEXITCODE)." }
    if ($Capture) {
        try { return [IO.File]::ReadAllText($capturePath, $utf8).Trim() }
        finally { if (Test-Path -LiteralPath $capturePath) { Remove-Item -LiteralPath $capturePath -Force } }
    }
}
function Copy-NativeFile([IO.FileInfo]$File, [string]$Destination) {
    $source = $File.FullName
    if ($File.LinkTarget) {
        $resolved = $File.ResolveLinkTarget($true)
        if (-not $resolved -or -not $resolved.FullName.StartsWith($work + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) { throw 'Native archive link escapes its owned build/extraction root.' }
        $source = $resolved.FullName
    }
    $out = Join-Path $Destination $File.Name
    if (Test-Path -LiteralPath $out) {
        if ((Get-FileHash -LiteralPath $source -Algorithm SHA256).Hash -ne (Get-FileHash -LiteralPath $out -Algorithm SHA256).Hash) { throw "Conflicting native runtime name: $($File.Name)." }
    } else { Copy-Item -LiteralPath $source -Destination $out }
}
function Export-CiEnvironment([string]$Name, [string]$Value) {
    if ($Value -match '[\r\n]') { throw 'CI environment value contains an unexpected newline.' }
    [Environment]::SetEnvironmentVariable($Name, $Value, 'Process')
    [IO.File]::AppendAllText($env:GITHUB_ENV, "$Name=$Value`n", $utf8)
}
function Get-PeMachine([string]$Path) {
    $stream = [IO.File]::OpenRead($Path)
    $reader = [IO.BinaryReader]::new($stream)
    try {
        if ($reader.ReadUInt16() -ne 0x5A4D) { throw 'Native Windows file has no DOS header.' }
        $stream.Position = 0x3c
        $offset = $reader.ReadInt32()
        if ($offset -lt 0 -or $offset -gt $stream.Length - 6) { throw 'Invalid native PE header offset.' }
        $stream.Position = $offset
        if ($reader.ReadUInt32() -ne 0x00004550) { throw 'Native Windows file has no PE header.' }
        return $reader.ReadUInt16()
    } finally { $reader.Dispose() }
}
function Get-MacDependencies([string]$Path) {
    $output = Invoke-NativeCommand -Name 'macho-dependencies' -Executable 'otool' -Arguments @('-L', $Path) -Capture
    return @($output -split "`n" | Select-Object -Skip 1 | ForEach-Object { ($_ -split '\s+\(')[0].Trim() } | Where-Object { $_ })
}
$baseline = 'e456309491fd875487bf02b8a0dca76c1b1b00cc'
$ortCommit = '402979ac1bef89dcd55add41eaa7277c407599eb'
try {
    Write-NativeLog 'INFO' "Starting target=$TargetId root=$root runtime_root=$runtimes workers=2"
    $manifest = Get-Content -LiteralPath (Join-Path $root 'vcpkg.json') -Raw -Encoding utf8 | ConvertFrom-Json
    if ($manifest.'builtin-baseline' -ne $baseline) { throw 'Native manifest baseline differs from the reviewed source revision.' }
    if (-not (Test-Path -LiteralPath (Join-Path $overlay "$($target.triplet).cmake"))) { throw 'Required dynamic release overlay triplet is missing.' }
    foreach ($entry in @{ VCPKG_DISABLE_METRICS = '1'; VCPKG_MAX_CONCURRENCY = '2'; CMAKE_BUILD_PARALLEL_LEVEL = '2'; MSBUILDDISABLENODEREUSE = '1'; MSBUILD_TELEMETRY_OPTOUT = '1'; CARGO_BUILD_JOBS = '2'; OMP_NUM_THREADS = '2'; VCPKG_DOWNLOADS = (Join-Path $work 'downloads'); VCPKG_DEFAULT_BINARY_CACHE = (Join-Path $work 'binary-cache'); TEMP = (Join-Path $work 'temp'); TMP = (Join-Path $work 'temp'); TMPDIR = (Join-Path $work 'temp'); GIT_TERMINAL_PROMPT = '0' }.GetEnumerator()) {
        [Environment]::SetEnvironmentVariable($entry.Key, $entry.Value, 'Process')
    }
    $env:VCPKG_ROOT = Join-Path $work 'vcpkg'
    if (-not (Test-Path -LiteralPath $env:VCPKG_ROOT)) {
        Invoke-NativeCommand -Name 'vcpkg-checkout' -Executable 'git' -Arguments @('clone', '--filter=blob:none', '--no-checkout', 'https://github.com/microsoft/vcpkg.git', $env:VCPKG_ROOT) -Wall 600 -Idle 180
    }
    Invoke-NativeCommand -Name 'vcpkg-pin' -Executable 'git' -Arguments @('-C', $env:VCPKG_ROOT, 'checkout', '--detach', $baseline)
    $actual = Invoke-NativeCommand -Name 'vcpkg-revision' -Executable 'git' -Arguments @('-C', $env:VCPKG_ROOT, 'rev-parse', 'HEAD') -Capture
    if ($actual -ne $baseline) { throw 'Pinned native dependency checkout verification failed.' }
    if ($IsWindows) {
        $vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio/Installer/vswhere.exe'
        if (-not (Test-Path -LiteralPath $vswhere)) { throw 'Hosted Visual Studio discovery tool is missing.' }
        $vs = Invoke-NativeCommand -Name 'vs-discovery' -Executable $vswhere -Arguments @('-latest', '-products', '*', '-property', 'installationPath') -Capture
        if (-not $vs -or -not (Test-Path -LiteralPath $vs)) { throw 'Hosted Visual Studio installation was not found.' }
        $candidates = @($env:LIBCLANG_PATH, (Join-Path $env:ProgramFiles 'LLVM/bin'), (Join-Path $vs 'VC/Tools/LLVM/ARM64/bin'), (Join-Path $vs 'VC/Tools/LLVM/x64/bin'), (Join-Path $vs 'VC/Tools/LLVM/bin'))
        $clang = Get-Command clang -CommandType Application -ErrorAction SilentlyContinue | Select-Object -First 1
        if ($clang) { $candidates += Split-Path $clang.Source -Parent }
        $machine = if ($hostArch -eq 'arm64') { 0xAA64 } else { 0x8664 }
        $libclang = @($candidates | Where-Object { $_ -and (Test-Path -LiteralPath (Join-Path $_ 'libclang.dll')) } | Where-Object { (Get-PeMachine (Join-Path $_ 'libclang.dll')) -eq $machine }) | Select-Object -First 1
        if (-not $libclang) { throw "No actual host-$hostArch libclang.dll found; bindgen cannot load a target-only compiler DLL." }
        Export-CiEnvironment 'LIBCLANG_PATH' $libclang
        Export-CiEnvironment 'VCPKG_VISUAL_STUDIO_PATH' $vs
        Invoke-NativeCommand -Name 'vcpkg-bootstrap' -Executable (Join-Path $env:VCPKG_ROOT 'bootstrap-vcpkg.bat') -Arguments @('-disableMetrics') -Wall 600 -Idle 180
    } else {
        Invoke-NativeCommand -Name 'vcpkg-bootstrap' -Executable 'bash' -Arguments @((Join-Path $env:VCPKG_ROOT 'bootstrap-vcpkg.sh'), '-disableMetrics') -Wall 600 -Idle 180
    }
    $vcpkg = Join-Path $env:VCPKG_ROOT $(if ($IsWindows) { 'vcpkg.exe' } else { 'vcpkg' })
    Invoke-NativeCommand -Name 'native-ports' -Executable $vcpkg -Arguments @('install', "--triplet=$($target.triplet)", "--host-triplet=$($target.triplet)", "--overlay-triplets=$overlay", "--x-manifest-root=$root", "--x-install-root=$work/installed", '--disable-metrics') -Wall 6600 -Idle 900 -ProgressDirectory (Join-Path $env:VCPKG_ROOT 'buildtrees')
    $prefix = Join-Path (Join-Path $work 'installed') $target.triplet
    foreach ($relative in @('include/libavcodec/avcodec.h', 'include/libavformat/avformat.h', 'include/libavutil/avutil.h', 'include/libswscale/swscale.h', 'include/tesseract/capi.h', 'share/ffmpeg/copyright', 'share/tesseract/copyright')) {
        if (-not (Test-Path -LiteralPath (Join-Path $prefix $relative))) { throw "Native installation missing required file: $relative." }
    }
    $installed = Join-Path $work 'installed'
    foreach ($entry in @{ FFMPEG_DIR = $prefix; FFMPEG_INCLUDE_DIR = (Join-Path $prefix 'include'); FFMPEG_LIB_DIR = (Join-Path $prefix 'lib'); VCPKG_ROOT = $env:VCPKG_ROOT; VCPKG_INSTALLED_ROOT = $installed; VCPKG_TARGET_TRIPLET = $target.triplet; VCPKGRS_TRIPLET = $target.triplet; VCPKGRS_DYNAMIC = '1'; VCPKG_OVERLAY_TRIPLETS = $overlay; VCPKG_DISABLE_METRICS = '1'; VCPKG_MAX_CONCURRENCY = '2'; CMAKE_BUILD_PARALLEL_LEVEL = '2'; NVSF_RUNTIME_ROOT = $runtimes; NVSF_NATIVE_NOTICES_ROOT = $notices; ORT_SKIP_DOWNLOAD = '1'; ORT_OFFLINE = '1' }.GetEnumerator()) {
        Export-CiEnvironment $entry.Key $entry.Value
    }
    $separator = [IO.Path]::PathSeparator
    Export-CiEnvironment 'PKG_CONFIG_PATH' ((Join-Path $prefix 'lib/pkgconfig') + $separator + (Join-Path $prefix 'share/pkgconfig'))
    if ($IsWindows) {
        Export-CiEnvironment 'LIB' ((Join-Path $prefix 'lib') + ';' + $env:LIB)
        Export-CiEnvironment 'INCLUDE' ((Join-Path $prefix 'include') + ';' + $env:INCLUDE)
        [IO.File]::AppendAllText($env:GITHUB_PATH, "$(Join-Path $prefix 'bin')`n$runtimes`n", $utf8)
        $env:PATH = "$runtimes;$(Join-Path $prefix 'bin');$env:PATH"
    } else {
        $pkgConfig = Get-Command pkg-config -CommandType Application -ErrorAction SilentlyContinue | Select-Object -First 1
        if (-not $pkgConfig) {
            $pkgConfig = @(Get-ChildItem -LiteralPath (Join-Path $env:VCPKG_ROOT 'downloads/tools') -Recurse -File -ErrorAction SilentlyContinue | Where-Object Name -in @('pkgconf', 'pkg-config')) | Select-Object -First 1
        }
        if (-not $pkgConfig) { throw 'Native pkg-config/pkgconf prerequisite is unavailable after vcpkg bootstrap.' }
        $pkgPath = if ($pkgConfig -is [Management.Automation.CommandInfo]) { $pkgConfig.Source } else { $pkgConfig.FullName }
        Export-CiEnvironment 'PKG_CONFIG' $pkgPath
        Export-CiEnvironment 'LIBRARY_PATH' (Join-Path $prefix 'lib')
        Export-CiEnvironment $(if ($IsMacOS) { 'DYLD_LIBRARY_PATH' } else { 'LD_LIBRARY_PATH' }) ("$runtimes$separator" + (Join-Path $prefix 'lib'))
        $rustKey = 'CARGO_TARGET_' + $target.target.Replace('-', '_').ToUpperInvariant() + '_RUSTFLAGS'
        $rpath = if ($IsMacOS) { '-C link-arg=-Wl,-rpath,@executable_path/runtimes' } else { '-C link-arg=-Wl,-rpath,$ORIGIN/runtimes' }
        $previous = [Environment]::GetEnvironmentVariable($rustKey)
        Export-CiEnvironment $rustKey ("$previous $rpath".Trim())
    }
    foreach ($port in Get-ChildItem -LiteralPath (Join-Path $prefix 'share') -Directory) {
        $copyright = Join-Path $port.FullName 'copyright'
        if (Test-Path -LiteralPath $copyright) {
            $destination = Join-Path $notices $port.Name
            [IO.Directory]::CreateDirectory($destination) | Out-Null
            Copy-Item -LiteralPath $copyright -Destination (Join-Path $destination 'copyright') -Force
        }
    }
    $nativeDir = Join-Path $prefix $(if ($IsWindows) { 'bin' } else { 'lib' })
    foreach ($file in Get-ChildItem -LiteralPath $nativeDir -File | Where-Object { $_.Name -match '\.dll$|\.dylib$|\.so(\.[0-9.]+)?$' }) { Copy-NativeFile $file $runtimes }
    $ortSourceBuild = $target.ort.PSObject.Properties.Name -contains 'source_build' -and $target.ort.source_build
    if ($ortSourceBuild) {
        if (-not $IsMacOS -or $target.arch -ne 'x64' -or $target.ort.source_tag -ne 'v1.31.0') { throw 'Unexpected ONNX source-build target.' }
        $source = Join-Path $work 'ort-source'
        if (-not (Test-Path -LiteralPath $source)) {
            [IO.Directory]::CreateDirectory($source) | Out-Null
            Invoke-NativeCommand -Name 'ort-init' -Executable 'git' -Arguments @('-C', $source, 'init')
            Invoke-NativeCommand -Name 'ort-fetch' -Executable 'git' -Arguments @('-C', $source, 'fetch', '--depth=1', 'https://github.com/microsoft/onnxruntime.git', $ortCommit) -Wall 900 -Idle 180
            Invoke-NativeCommand -Name 'ort-checkout' -Executable 'git' -Arguments @('-C', $source, 'checkout', '--detach', 'FETCH_HEAD')
        }
        $actual = Invoke-NativeCommand -Name 'ort-revision' -Executable 'git' -Arguments @('-C', $source, 'rev-parse', 'HEAD') -Capture
        if ($actual -ne $ortCommit -or (Get-Content -LiteralPath (Join-Path $source 'VERSION_NUMBER') -Raw -Encoding utf8).Trim() -ne '1.31.0') { throw 'ONNX source revision/version verification failed.' }
        Invoke-NativeCommand -Name 'ort-submodules' -Executable 'git' -Arguments @('-C', $source, 'submodule', 'update', '--init', '--recursive', '--jobs', '2') -Wall 900 -Idle 180
        Invoke-NativeCommand -Name 'ort-source-build' -Executable 'python3' -Arguments @((Join-Path $source 'tools/ci_build/build.py'), '--build_dir', (Join-Path $work 'ort-build'), '--config', 'Release', '--update', '--build', '--build_shared_lib', '--parallel', '2', '--skip_tests', '--skip_submodule_sync', '--no_telemetry', '--cmake_extra_defines', 'CMAKE_OSX_DEPLOYMENT_TARGET=13.0', 'CMAKE_OSX_ARCHITECTURES=x86_64', 'onnxruntime_BUILD_UNIT_TESTS=OFF') -Wall 6600 -Idle 900
        $ortRoot = $source
        $ortLibraries = @(Get-ChildItem -LiteralPath (Join-Path $work 'ort-build/Release') -File | Where-Object Name -match '^libonnxruntime.*\.dylib$')
    } else {
        $uri = [Uri]$target.ort.url
        if ($uri.Scheme -ne 'https' -or $uri.Host -ne 'github.com' -or -not $uri.AbsolutePath.StartsWith('/microsoft/onnxruntime/releases/download/v1.31.0/')) { throw 'ONNX download is not an approved pinned official release URL.' }
        if ($target.ort.sha256 -notmatch '^[0-9a-f]{64}$' -or $target.ort.bytes -lt 1) { throw 'ONNX archive pin is invalid.' }
        $archive = Join-Path $work ([IO.Path]::GetFileName($uri.AbsolutePath))
        Invoke-NativeCommand -Name 'ort-download' -Executable 'curl' -Arguments @('--fail', '--location', '--proto', '=https', '--proto-redir', '=https', '--connect-timeout', '30', '--max-time', '180', '--output', $archive, $uri.AbsoluteUri) -Wall 210 -Idle 60
        if ((Get-Item -LiteralPath $archive).Length -ne $target.ort.bytes -or (Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash.ToLowerInvariant() -ne $target.ort.sha256) { throw 'ONNX archive bytes/SHA256 mismatch.' }
        $unpack = Join-Path $work 'ort-unpack'
        [IO.Directory]::CreateDirectory($unpack) | Out-Null
        if ($archive.EndsWith('.zip')) {
            $zip = [IO.Compression.ZipFile]::OpenRead($archive)
            try {
                foreach ($entry in $zip.Entries) { if ($entry.FullName -match '(^[/\\]|(^|[/\\])\.\.([/\\]|$)|^[A-Za-z]:)') { throw 'ONNX archive contains an unsafe entry path.' } }
            } finally { $zip.Dispose() }
            Expand-Archive -LiteralPath $archive -DestinationPath $unpack -Force
        } else {
            $entries = Invoke-NativeCommand -Name 'ort-archive-list' -Executable 'tar' -Arguments @('-tzf', $archive) -Capture
            foreach ($entry in $entries -split "`n") { if ($entry -match '(^/|(^|/)\.\.(/|$)|^[A-Za-z]:)') { throw 'ONNX archive contains an unsafe entry path.' } }
            Invoke-NativeCommand -Name 'ort-extract' -Executable 'tar' -Arguments @('-xzf', $archive, '-C', $unpack)
        }
        $ortRoots = @(Get-ChildItem -LiteralPath $unpack -Directory)
        if ($ortRoots.Count -ne 1) { throw 'ONNX archive must contain one exact release root.' }
        $ortRoot = $ortRoots[0].FullName
        $ortLibraries = @(Get-ChildItem -LiteralPath (Join-Path $ortRoot 'lib') -File | Where-Object Name -match '^onnxruntime.*\.dll$|^libonnxruntime.*\.(so|dylib)(\.[0-9.]+)?$')
    }
    if ($ortLibraries.Count -eq 0) { throw 'ONNX provision produced no actual native shared library.' }
    foreach ($file in $ortLibraries) { Copy-NativeFile $file $runtimes }
    $ortNotices = Join-Path $notices 'onnxruntime'
    [IO.Directory]::CreateDirectory($ortNotices) | Out-Null
    foreach ($pattern in @('LICENSE', 'ThirdPartyNotices.txt')) {
        $notice = @(Get-ChildItem -LiteralPath $ortRoot -Recurse -File -Filter $pattern | Sort-Object { $_.FullName.Length }) | Select-Object -First 1
        if (-not $notice) { throw "ONNX release/source is missing required legal notice: $pattern." }
        Copy-Item -LiteralPath $notice.FullName -Destination (Join-Path $ortNotices $pattern) -Force
    }
    if ($IsWindows) {
        $required = @('avcodec-63.dll', 'avformat-63.dll', 'avutil-61.dll', 'swscale-10.dll', 'tesseract55.dll', 'onnxruntime.dll')
        foreach ($file in Get-ChildItem -LiteralPath $runtimes -File -Filter '*.dll') {
            if ((Get-PeMachine $file.FullName) -ne $machine) { throw "Native runtime architecture mismatch: $($file.Name)." }
        }
    } else {
        $extension = if ($IsMacOS) { 'dylib' } else { 'so' }
        foreach ($base in @('avcodec', 'avformat', 'avutil', 'swscale', 'tesseract', 'onnxruntime')) {
            $canonical = Join-Path $runtimes "lib$base.$extension"
            if (-not (Test-Path -LiteralPath $canonical)) {
                $versioned = @(Get-ChildItem -LiteralPath $runtimes -File | Where-Object Name -match $(if ($IsMacOS) { "^lib$base\.[0-9.]+\.dylib$" } else { "^lib$base\.so\.[0-9.]+$" })) | Select-Object -First 1
                if (-not $versioned) { throw "Native library missing: $base." }
                Copy-Item -LiteralPath $versioned.FullName -Destination $canonical
            }
        }
        $required = @('avcodec', 'avformat', 'avutil', 'swscale', 'tesseract', 'onnxruntime') | ForEach-Object { "lib$_.$extension" }
        if ($IsMacOS) {
            $queue = [Collections.Generic.Queue[string]]::new()
            foreach ($file in Get-ChildItem -LiteralPath $runtimes -File -Filter '*.dylib') { $queue.Enqueue($file.FullName) }
            $visited = [Collections.Generic.HashSet[string]]::new()
            while ($queue.Count -gt 0) {
                if ($visited.Count -gt 512) { throw 'Native Mach-O dependency closure exceeded its file bound.' }
                $library = $queue.Dequeue()
                if (-not $visited.Add($library)) { continue }
                foreach ($dep in Get-MacDependencies $library) {
                    if ($dep.StartsWith('/usr/lib/') -or $dep.StartsWith('/System/Library/')) { continue }
                    $basename = [IO.Path]::GetFileName($dep)
                    $packaged = Join-Path $runtimes $basename
                    if (-not (Test-Path -LiteralPath $packaged)) {
                        $local = Join-Path (Join-Path $prefix 'lib') $basename
                        if (-not (Test-Path -LiteralPath $local)) { throw "Unresolved non-system Mach-O dependency: $dep." }
                        Copy-NativeFile (Get-Item -LiteralPath $local) $runtimes
                        $queue.Enqueue($packaged)
                    }
                    if ($basename -ne [IO.Path]::GetFileName($library)) {
                        Invoke-NativeCommand -Name 'macho-relocate' -Executable 'install_name_tool' -Arguments @('-change', $dep, "@loader_path/$basename", $library)
                    }
                }
                Invoke-NativeCommand -Name 'macho-id' -Executable 'install_name_tool' -Arguments @('-id', ("@rpath/" + [IO.Path]::GetFileName($library)), $library)
                Invoke-NativeCommand -Name 'macho-adhoc-sign' -Executable 'codesign' -Arguments @('--force', '--sign', '-', $library)
                $identity = Invoke-NativeCommand -Name 'macho-id-verify' -Executable 'otool' -Arguments @('-D', $library) -Capture
                if (($identity -split "`n" | Select-Object -Last 1).Trim() -ne ("@rpath/" + [IO.Path]::GetFileName($library))) { throw 'Relocated Mach-O install identity verification failed.' }
            }
        } else {
            foreach ($file in Get-ChildItem -LiteralPath $runtimes -File | Where-Object Name -match '\.so(\.[0-9.]+)?$') {
                Invoke-NativeCommand -Name 'elf-relocate' -Executable 'patchelf' -Arguments @('--set-rpath', '$ORIGIN', $file.FullName)
                $needed = Invoke-NativeCommand -Name 'elf-dependencies' -Executable 'patchelf' -Arguments @('--print-needed', $file.FullName) -Capture
                foreach ($dep in $needed -split "`n" | Where-Object { $_ }) {
                    if (Test-Path -LiteralPath (Join-Path $runtimes $dep)) { continue }
                    if ($dep -match '^(lib(c|m|dl|pthread|rt|resolv|util|gcc_s|stdc\+\+)\.so(\.[0-9]+)*|ld-linux[^/]*\.so(\.[0-9]+)*)$') { continue }
                    throw "Unresolved non-system ELF dependency: $dep ($($file.Name))."
                }
            }
        }
    }
    foreach ($name in $required) { if (-not (Test-Path -LiteralPath (Join-Path $runtimes $name) -PathType Leaf)) { throw "Expected native runtime absent: $name." } }
    $llamaTagObject = '69ecae922d2b60c1fa8c590279f4e2099af75111'
    $llamaCommit = 'd81235049384534c167caea52b85a694f6103d14'
    $llamaSource = Join-Path $work 'llama-source'
    if (-not (Test-Path -LiteralPath $llamaSource)) {
        [IO.Directory]::CreateDirectory($llamaSource) | Out-Null
        Invoke-NativeCommand -Name 'llama-init' -Executable 'git' -Arguments @('-C', $llamaSource, 'init')
        Invoke-NativeCommand -Name 'llama-fetch' -Executable 'git' -Arguments @('-C', $llamaSource, 'fetch', '--depth=1', 'https://github.com/ggml-org/llama.cpp.git', 'refs/tags/v0.6.0:refs/tags/v0.6.0') -Wall 600 -Idle 180
    }
    $tagObject = Invoke-NativeCommand -Name 'llama-tag' -Executable 'git' -Arguments @('-C', $llamaSource, 'rev-parse', 'refs/tags/v0.6.0') -Capture
    $peeled = Invoke-NativeCommand -Name 'llama-commit' -Executable 'git' -Arguments @('-C', $llamaSource, 'rev-parse', 'refs/tags/v0.6.0^{}') -Capture
    if ($tagObject -ne $llamaTagObject -or $peeled -ne $llamaCommit) { throw 'Stable llama.cpp tag/peeled-source revision changed or mismatched.' }
    Invoke-NativeCommand -Name 'llama-checkout' -Executable 'git' -Arguments @('-C', $llamaSource, 'checkout', '--detach', $llamaCommit)
    $llamaBuild = Join-Path $work 'llama-build'
    $configure = @('-S', $llamaSource, '-B', $llamaBuild, '-DCMAKE_BUILD_TYPE=Release', '-DBUILD_SHARED_LIBS=OFF', '-DLLAMA_BUILD_IS_DEV=OFF', '-DLLAMA_BUILD_TOOLS=ON', '-DLLAMA_BUILD_TESTS=OFF', '-DLLAMA_BUILD_EXAMPLES=OFF', '-DLLAMA_BUILD_SERVER=OFF', '-DLLAMA_BUILD_APP=OFF', '-DLLAMA_OPENSSL=OFF', '-DLLAMA_SUBPROCESS=OFF', '-DMTMD_VIDEO=OFF', '-DGGML_NATIVE=OFF', '-DGGML_METAL=OFF', '-DGGML_BLAS=OFF', '-DGGML_CUDA=OFF', '-DGGML_OPENMP=OFF', '-DGGML_CPU_KLEIDIAI=OFF')
    if ($IsWindows) {
        $configure += @('-A', $(if ($target.arch -eq 'arm64') { 'ARM64' } else { 'x64' }))
        if ($target.arch -eq 'arm64') { $configure += @('-T', 'ClangCL', '-DGGML_CPU_ARM_ARCH=armv8-a') }
    }
    elseif ($IsMacOS) { $configure += @('-DCMAKE_OSX_DEPLOYMENT_TARGET=13.0', $(if ($target.arch -eq 'arm64') { '-DCMAKE_OSX_ARCHITECTURES=arm64' } else { '-DCMAKE_OSX_ARCHITECTURES=x86_64' })) }
    Invoke-NativeCommand -Name 'llama-configure' -Executable 'cmake' -Arguments $configure -Wall 300 -Idle 120
    Invoke-NativeCommand -Name 'llama-build' -Executable 'cmake' -Arguments @('--build', $llamaBuild, '--config', 'Release', '--target', 'llama-mtmd-cli', '--parallel', '2') -Wall 3600 -Idle 600
    $llamaName = if ($IsWindows) { 'llama-mtmd-cli.exe' } else { 'llama-mtmd-cli' }
    $llamaBinaries = @(Get-ChildItem -LiteralPath (Join-Path $llamaBuild 'bin') -Recurse -File -Filter $llamaName)
    if ($llamaBinaries.Count -ne 1) { throw 'Native llama.cpp build did not produce one exact helper executable.' }
    Copy-Item -LiteralPath $llamaBinaries[0].FullName -Destination (Join-Path $runtimes $llamaName) -Force
    $llamaNotices = Join-Path $notices 'llama.cpp'
    [IO.Directory]::CreateDirectory($llamaNotices) | Out-Null
    Copy-Item -LiteralPath (Join-Path $llamaSource 'LICENSE') -Destination (Join-Path $llamaNotices 'LICENSE') -Force
    foreach ($licenseFile in Get-ChildItem -LiteralPath (Join-Path $llamaSource 'vendor') -Recurse -File | Where-Object Name -match '^(LICENSE|COPYING|NOTICE)([._-].*)?$') {
        $relative = [IO.Path]::GetRelativePath($llamaSource, $licenseFile.FullName)
        $destination = Join-Path $llamaNotices $relative
        [IO.Directory]::CreateDirectory((Split-Path $destination -Parent)) | Out-Null
        Copy-Item -LiteralPath $licenseFile.FullName -Destination $destination -Force
    }
    if ($IsWindows -and (Get-PeMachine (Join-Path $runtimes $llamaName)) -ne $machine) { throw 'Native visual-helper PE architecture mismatch.' }
    if ($IsMacOS) {
        foreach ($dependency in Get-MacDependencies (Join-Path $runtimes $llamaName)) {
            if (-not $dependency.StartsWith('/usr/lib/') -and -not $dependency.StartsWith('/System/Library/')) { throw "Unexpected dynamic visual-helper dependency: $dependency." }
        }
        Invoke-NativeCommand -Name 'llama-adhoc-sign' -Executable 'codesign' -Arguments @('--force', '--sign', '-', (Join-Path $runtimes $llamaName))
    }
    Invoke-NativeCommand -Name 'llama-help' -Executable (Join-Path $runtimes $llamaName) -Arguments @('--help') -Wall 30 -Idle 10
    $inventory = @(Get-ChildItem -LiteralPath $runtimes -File | Sort-Object Name | ForEach-Object { @{ file = $_.Name; bytes = $_.Length; sha256 = (Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash.ToLowerInvariant() } })
    $record = @{ schema = 1; target = $TargetId; vcpkg_commit = $baseline; ort_version = '1.31.0'; ort_source_commit = $(if ($ortSourceBuild) { $ortCommit } else { $null }); overlay_triplet = $target.triplet; linkage = 'dynamic'; build_type = 'release'; llama_tag_object = $llamaTagObject; llama_source_commit = $llamaCommit; llama_cli_status = 'EXPERIMENTAL_MODEL_NOT_LOADED'; runtime_files = $inventory; runtime_execution = 'NOT_RUN'; final_package_closure = 'PENDING_HOSTED_TEST' }
    [IO.File]::WriteAllText((Join-Path $notices 'native-inventory.json'), (ConvertTo-Json $record -Depth 6), $utf8)
    Write-NativeLog 'INFO' "Provisioning complete target=$TargetId runtime_files=$($inventory.Count); actual package/model inference remains a later required CI gate."
} catch {
    Write-NativeLog 'ERROR' "$($_.Exception.Message) $($_.ScriptStackTrace)"
    throw
} finally { $logger.Dispose() }
