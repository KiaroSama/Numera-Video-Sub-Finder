#requires -Version 7.4
[CmdletBinding()]
param()
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
if ($env:GITHUB_ACTIONS -ne 'true' -or $env:RUNNER_ENVIRONMENT -ne 'github-hosted' -or -not $IsLinux -or [Runtime.InteropServices.RuntimeInformation]::OSArchitecture -ne 'X64' -or [Runtime.InteropServices.RuntimeInformation]::ProcessArchitecture -ne 'X64') {
    throw 'Media sanitizer admission requires a native Linux x64 GitHub-hosted runner.'
}
$root = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../..'))
Set-Location -LiteralPath $root
if ($env:GITHUB_SHA -notmatch '^[0-9a-f]{40}$') { throw 'Media sanitizer admission requires an exact source SHA.' }
$utf8 = [Text.UTF8Encoding]::new($false)
$nightly = 'nightly-2026-10-27'
$target = 'x86_64-unknown-linux-gnu'
$work = Join-Path $root '.ci-work/media-sanitizer'
if ((Get-Item -Force -LiteralPath (Join-Path $root '.ci-work')).LinkType -or (Test-Path -LiteralPath $work)) { throw 'Media sanitizer state must be a new unlinked directory.' }
[IO.Directory]::CreateDirectory($work) | Out-Null
$stage = Join-Path $work 'runtimes'
[IO.Directory]::CreateDirectory($stage) | Out-Null
$reports = Join-Path $root '.ci-work/packages/reports'
[IO.Directory]::CreateDirectory($reports) | Out-Null
$reportPath = Join-Path $reports 'linux-x64-media-sanitizer.json'
$logPath = Join-Path $root ('.ci-work/logs/media-sanitizer_' + [DateTime]::UtcNow.ToString('yyyy-MM-dd_HH-mm-ss') + '_UTC.log')
try {
    $writer = [IO.StreamWriter]::new([IO.FileStream]::new($logPath,[IO.FileMode]::CreateNew),$utf8)
    $writer.AutoFlush = $true
} catch { [Console]::Error.WriteLine('Media sanitizer file logging failed; no admission result was produced.'); throw }
function Write-Log([string]$Level,[string]$Message) {
    $line = '[{0}] [{1}] [MEDIA-ASAN] {2}' -f [DateTime]::UtcNow.ToString('yyyy-MM-dd HH:mm:ss UTC'),$Level,$Message
    $writer.WriteLine($line)
    [Console]::Error.WriteLine($line)
}
function Hash-File([string]$Path) { (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant() }
function Save-Report { [IO.File]::WriteAllText($reportPath,($report | ConvertTo-Json -Depth 12),$utf8) }
function Assert-Regular([string]$Path) {
    $item = Get-Item -Force -LiteralPath $Path
    if ($item.PSIsContainer -or $item.Length -lt 1 -or ($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -or ($item.LinkType -and $item.LinkType -ne 'HardLink')) { throw "Expected a nonempty regular compiler/runtime file: $Path" }
    $cursor = $item.Directory
    while ($cursor -and $cursor.FullName -ne $root) {
        if ($cursor.LinkType -or ($cursor.Attributes -band [IO.FileAttributes]::ReparsePoint)) { throw 'Sanitizer artifact ancestor must not be linked.' }
        $cursor = $cursor.Parent
    }
    if (-not $cursor) { throw 'Sanitizer artifact escaped the checkout.' }
}
function Invoke-Guard([string]$Name,[int]$Wall,[int]$Idle,[string]$Executable,[string[]]$ArgumentList,[string]$Capture = '') {
    $parameters = @{Name="media-asan-$Name";WallSeconds=$Wall;IdleSeconds=$Idle;Executable=$Executable;Arguments=$ArgumentList;WorkingDirectory=$root}
    if ($Capture) { $parameters.CaptureStdoutPath = $Capture }
    & (Join-Path $PSScriptRoot 'guard.ps1') @parameters
    if ($LASTEXITCODE -ne 0) { throw "Media sanitizer stage failed: $Name exit=$LASTEXITCODE" }
    $logs = @(Get-ChildItem -LiteralPath (Join-Path $root '.ci-work/logs') -Filter "guard-media-asan-$Name`_*_UTC.log" -File)
    if ($logs.Count -ne 1 -or $logs[0].Length -gt 33554432) { throw "Missing, duplicate or oversized sanitizer stage evidence: $Name" }
    $text = [IO.File]::ReadAllText($logs[0].FullName,$utf8)
    if ($text -notmatch "Command finished name=media-asan-$Name exit_code=0 " -or $text -notmatch 'Cleanup verified pid=\d+') { throw "Sanitizer stage lacks successful owned cleanup: $Name" }
    if ($text -match 'ERROR: (AddressSanitizer|LeakSanitizer)|AddressSanitizer:DEADLYSIGNAL|SUMMARY: (AddressSanitizer|LeakSanitizer)') { throw "Sanitizer diagnostic in stage: $Name" }
    $report.commands += @{name=$Name;log_sha256=(Hash-File $logs[0].FullName);cleanup_verified=$true}
    return $text
}
function Invoke-Build([string]$Name,[string]$Executable,[string[]]$ArgumentList,[string]$Capture = '') {
    $remaining = [int][Math]::Floor(600 - $buildClock.Elapsed.TotalSeconds)
    if ($remaining -lt 1) { throw 'Aggregate media sanitizer build budget exhausted (600 seconds).' }
    return Invoke-Guard $Name $remaining ([Math]::Min(180,$remaining)) $Executable $ArgumentList $Capture
}
function Read-Artifacts([string]$Path) {
    $rows = @()
    $completed = 0
    foreach ($line in [IO.File]::ReadAllLines($Path,$utf8)) {
        # Cargo's JSON mode does not control arbitrary output from other build tools.
        if (-not $line.StartsWith('{', [StringComparison]::Ordinal)) { continue }
        $message = $line | ConvertFrom-Json -AsHashtable
        if ($message.reason -eq 'compiler-artifact') { $rows += $message }
        if ($message.reason -eq 'build-finished') {
            if ($message.success -ne $true) { throw 'Cargo reported an unsuccessful sanitizer build.' }
            $completed++
        }
    }
    if (-not $rows.Count -or $completed -ne 1) { throw 'Cargo requires actual compiler artifacts and one successful completion record.' }
    return $rows
}
function Run-Tests([string]$Name,[string]$Executable,[string[]]$ArgumentList,[int]$Expected = 0) {
    $capture = Join-Path $root ".ci-work/logs/media-asan-$Name.stdout"
    $null = Invoke-Guard $Name 120 60 $Executable ($ArgumentList + @('--test-threads=1','--nocapture')) $capture
    $text = [IO.File]::ReadAllText($capture,$utf8)
    $result = [regex]::Matches($text,'test result: ok\. (\d+) passed; (\d+) failed; (\d+) ignored; (\d+) measured; (\d+) filtered out')
    if ($result.Count -ne 1) { throw "Missing unique actual libtest completion: $Name" }
    $row = $result[0]
    $passed = [int]$row.Groups[1].Value
    if ($passed -lt 1 -or ($Expected -gt 0 -and $passed -ne $Expected)) { throw "Zero or changed selected sanitizer test inventory: $Name" }
    foreach ($index in 2,3,4) { if ([int]$row.Groups[$index].Value -ne 0) { throw "Failed, ignored or measured-only sanitizer tests: $Name" } }
    if ($Name -ne 'callbacks' -and [int]$row.Groups[5].Value -ne 0) { throw "Unexpected filtered sanitizer tests: $Name" }
    $report.suites += @{name=$Name;tests=$passed;failed=0;skipped=0;filtered=[int]$row.Groups[5].Value;binary_sha256=(Hash-File $Executable)}
    $report.tests += $passed
    return $text
}
$started = [DateTime]::UtcNow
$report = [ordered]@{
    schema=1;source_sha=$env:GITHUB_SHA;target='linux-x64';rust_target=$target;profile='actual-media-rust-ffi-asan';nightly=$nightly;
    passed=$false;tests=0;failed=1;skipped=0;sanitizer='address';instrumented_std=$true;build_workers=2;test_threads=1;
    build_wall_seconds=600;build_idle_seconds=180;test_wall_seconds=120;test_idle_seconds=60;commands=@();suites=@();
    qualification='Rust caller/cdylib and Rust std only; existing release FFmpeg/C closure uninstrumented; not UBSan, whole-program or cross-platform qualification'
}
$environment = @{}
$names = @('RUSTFLAGS','RUSTDOCFLAGS','CARGO_ENCODED_RUSTFLAGS','RUSTC_WRAPPER','RUSTC_WORKSPACE_WRAPPER','LLVM_PROFILE_FILE','CARGO_TARGET_DIR','CARGO_BUILD_TARGET','CARGO_BUILD_JOBS','RUST_TEST_THREADS','NVSF_RUNTIME_ROOT','NVSF_MEDIA_SANITIZER_ROOT','NVSF_MEDIA_SANITIZER','ASAN_OPTIONS','LSAN_OPTIONS','LD_LIBRARY_PATH','LD_PRELOAD','RUSTUP_AUTO_INSTALL')
$names += @(Get-ChildItem Env: | Where-Object { $_.Name -match '^(CARGO_LLVM_COV|__CARGO_LLVM_COV|CARGO_TARGET_.*_RUSTFLAGS)' } | ForEach-Object Name)
$originalEnvironment = [Environment]::GetEnvironmentVariables('Process')
foreach ($name in $names | Sort-Object -Unique) { $environment[$name] = @{present=$originalEnvironment.Contains($name);value=$originalEnvironment[$name]} }
Save-Report
try {
    Write-Log 'INFO' 'Starting actual media caller/cdylib ASan admission; release C closure remains uninstrumented'
    $head = & git -C $root rev-parse HEAD
    if ($LASTEXITCODE -ne 0 -or $head.Trim() -ne $env:GITHUB_SHA) { throw 'Sanitizer checkout SHA mismatch.' }
    $inputs = @('Cargo.toml','Cargo.lock','crates/app/Cargo.toml','crates/media-native/Cargo.toml','crates/app/src/media.rs','crates/core/src/media_abi.rs','crates/media-native/src/lib.rs','crates/app/tests/media_display_geometry.rs','crates/app/tests/support/media_sanitizer_admission.rs','scripts/ci/media-sanitizer.ps1')
    $report.inputs = @($inputs | ForEach-Object { & git -C $root diff --quiet HEAD -- $_; if ($LASTEXITCODE -ne 0) { throw 'Sanitizer input differs from checked-out source.' }; @{file=$_;sha256=(Hash-File (Join-Path $root $_))} })
    $readerFixture = Join-Path $work 'cargo-records-regression.jsonl'
    $artifactRecord = '{"reason":"compiler-artifact","target":{"name":"literal"}}'
    $finishedRecord = '{"reason":"build-finished","success":true}'
    [IO.File]::WriteAllText($readerFixture, ("[compiler_builtins 0.1.160] cargo:rerun-if-env-changed=LIBM_BUILD_VERBOSE`n" + $artifactRecord + "`n" + $finishedRecord), $utf8)
    if (@(Read-Artifacts $readerFixture).Count -ne 1) { throw 'Mixed Cargo stdout regression failed.' }
    foreach ($invalid in @(
        ('{' + "`n" + $artifactRecord + "`n" + $finishedRecord),
        $finishedRecord,
        $artifactRecord,
        ($artifactRecord + "`n" + '{"reason":"build-finished","success":false}'),
        ($artifactRecord + "`n" + $finishedRecord + "`n" + $finishedRecord)
    )) {
        [IO.File]::WriteAllText($readerFixture, $invalid, $utf8)
        $rejected = $false
        try { $null = Read-Artifacts $readerFixture } catch { $rejected = $true }
        if (-not $rejected) { throw 'Invalid Cargo record regression was accepted.' }
    }
    Write-Log 'INFO' 'Cargo record reader mixed-output and five rejection regressions passed'
    $ordinary = Join-Path $root '.ci-work/runtimes'
    $inventory = [IO.File]::ReadAllText((Join-Path $root '.ci-work/native-notices/native-inventory.json'),$utf8) | ConvertFrom-Json -AsHashtable
    if ($inventory.schema -ne 1 -or $inventory.source_sha -ne $env:GITHUB_SHA -or $inventory.target -ne 'linux-x64' -or $inventory.rust_target -ne $target -or $inventory.linkage -ne 'dynamic' -or $inventory.cargo_lock_sha256 -ne (Hash-File (Join-Path $root 'Cargo.lock')) -or -not $inventory.media_adapter) { throw 'Sanitizer requires the current same-source staged native closure.' }
    if ($inventory.runtime_files.Count -lt 1 -or $inventory.runtime_files.Count -gt 512) { throw 'Invalid native closure inventory bound.' }
    $known = [Collections.Generic.HashSet[string]]::new([StringComparer]::Ordinal)
    $staged = @{}
    foreach ($entry in $inventory.runtime_files) {
        if ($entry.file -match '[/\\:]|^\.{1,2}$' -or -not $known.Add([string]$entry.file)) { throw 'Unsafe or duplicate native inventory member.' }
        $file = Join-Path $ordinary $entry.file
        Assert-Regular $file
        if ((Get-Item -LiteralPath $file).Length -ne $entry.bytes -or (Hash-File $file) -ne $entry.sha256) { throw 'Native closure bytes differ from current inventory.' }
        if ($entry.file -match '\.so(\.[0-9.]+)?$' -and $entry.file -ne 'libnvsf_media_native.so') {
            Copy-Item -LiteralPath $file -Destination (Join-Path $stage $entry.file)
            $staged[$entry.file] = $entry.sha256
        }
    }
    if (@(Get-ChildItem -Force -LiteralPath $ordinary).Count -ne $known.Count) { throw 'Uninventoried ordinary runtime files.' }
    foreach ($base in 'libavcodec.so','libavformat.so','libavutil.so','libswscale.so') { if (-not $staged.ContainsKey($base)) { throw "Missing admitted FFmpeg member: $base" } }
    foreach ($name in $environment.Keys) { Remove-Item -LiteralPath "Env:$name" -ErrorAction SilentlyContinue }
    # Linux Rust cdylibs borrow the executable's compiler-rt; export it for actual dlopen resolution.
    $env:RUSTFLAGS = '-Zsanitizer=address -Cforce-frame-pointers=yes -Cdebuginfo=2 -Clink-arg=-Wl,--export-dynamic'
    $env:CARGO_BUILD_JOBS = '2'
    $env:RUST_TEST_THREADS = '1'
    $env:RUSTUP_AUTO_INSTALL = '0'
    $env:CARGO_TARGET_DIR = Join-Path $work 'target'
    $env:NVSF_RUNTIME_ROOT = $stage
    $env:NVSF_MEDIA_SANITIZER_ROOT = $stage
    $env:NVSF_MEDIA_SANITIZER = 'address'
    $env:LD_LIBRARY_PATH = $stage
    $env:ASAN_OPTIONS = 'halt_on_error=1:abort_on_error=1:detect_leaks=1:detect_stack_use_after_return=1:strict_string_checks=1'
    $buildClock = [Diagnostics.Stopwatch]::StartNew()
    $null = Invoke-Build 'nightly-install' 'rustup' @('toolchain','install',$nightly,'--profile','minimal','--component','rust-src')
    $versionPath = Join-Path $work 'rustc-version.txt'
    $null = Invoke-Build 'version' 'rustup' @('run',$nightly,'rustc','--version','--verbose') $versionPath
    $version = [IO.File]::ReadAllText($versionPath,$utf8)
    if ($version -notmatch 'rustc 1\.101\.0-nightly' -or $version -notmatch 'commit-hash: 16d1ffac17ad8b739e32483ac2e65fe7e1804977' -or $version -notmatch 'host: x86_64-unknown-linux-gnu') { throw 'Pinned sanitizer nightly compiler/host identity mismatch.' }
    $report.rustc = $version.Trim()
    $common = @('--locked','--target',$target,'--target-dir',$env:CARGO_TARGET_DIR,'-Zbuild-std','-vv','--message-format=json-render-diagnostics')
    $nativeCapture = Join-Path $root '.ci-work/logs/media-asan-native-build.jsonl'
    $nativeLog = Invoke-Build 'native-build' 'cargo' (@("+$nightly",'build','-p','nvsf-media-native') + $common) $nativeCapture
    $nativeTests = Join-Path $root '.ci-work/logs/media-asan-native-tests-build.jsonl'
    $null = Invoke-Build 'native-tests-build' 'cargo' (@("+$nightly",'test','--no-run','-p','nvsf-media-native','--lib') + $common) $nativeTests
    $appCapture = Join-Path $root '.ci-work/logs/media-asan-app-build.jsonl'
    $appLog = Invoke-Build 'app-build' 'cargo' (@("+$nightly",'test','--no-run','-p','nvsf-app','--lib','--test','media_display_geometry') + $common) $appCapture
    $report.build_elapsed_ms = [long]$buildClock.Elapsed.TotalMilliseconds
    foreach ($pair in @(@{text=$nativeLog;crate='nvsf_media_native'},@{text=$appLog;crate='nvsf_app'})) {
        # Guard chunks may split one long rustc command; ELF symbol checks below bind instrumentation to each artifact.
        if ($pair.text -notmatch ('--crate-name ' + $pair.crate + '\b') -or $pair.text -notmatch '-Zsanitizer=address') { throw 'Missing actual rustc sanitizer instrumentation evidence for caller/callee.' }
    }
    $artifacts = @(Read-Artifacts $nativeCapture)
    foreach ($standard in 'core','alloc','std') {
        if (-not @($artifacts | Where-Object { $_.target.name -eq $standard -and $_.package_id -match 'rust/library/' }).Count) { throw "Missing rebuilt Rust standard library artifact: $standard" }
    }
    $libraries = @($artifacts | Where-Object { $_.target.name -eq 'nvsf_media_native' -and 'cdylib' -in $_.target.crate_types } | ForEach-Object { $_.filenames | Where-Object { [IO.Path]::GetFileName($_) -eq 'libnvsf_media_native.so' } })
    if ($libraries.Count -ne 1) { throw 'Expected exactly one actual instrumented media cdylib compiler artifact.' }
    $library = [IO.Path]::GetFullPath($libraries[0])
    if (-not $library.StartsWith($env:CARGO_TARGET_DIR + '/', [StringComparison]::Ordinal)) { throw 'Native sanitizer compiler artifact escaped its isolated target root.' }
    Assert-Regular $library
    $report.compiler_adapter_sha256 = Hash-File $library
    $adapter = Join-Path $stage 'libnvsf_media_native.so'
    Copy-Item -LiteralPath $library -Destination $adapter
    $null = Invoke-Guard 'rpath' 15 5 'patchelf' @('--set-rpath','$ORIGIN',$adapter)
    $symbols = Invoke-Guard 'adapter-symbols' 15 5 'nm' @('-D',$adapter)
    foreach ($symbol in 'nvsf_media_probe_v1','nvsf_media_decode_v1','nvsf_media_health_v1','__asan_init') { if ($symbols -notmatch "(?m)\b$symbol\s*$") { throw "Instrumented media ELF symbol missing: $symbol" } }
    $staged['libnvsf_media_native.so'] = Hash-File $adapter
    $report.adapter_sha256 = $staged['libnvsf_media_native.so']
    [IO.File]::WriteAllText((Join-Path $work 'staged-libraries.json'),($staged | ConvertTo-Json),$utf8)
    $testArtifacts = @((Read-Artifacts $nativeTests)) + @((Read-Artifacts $appCapture))
    foreach ($row in $testArtifacts | Where-Object { $_.profile.test -and $_.executable }) { Assert-Regular $row.executable; if (-not $row.executable.StartsWith($env:CARGO_TARGET_DIR + '/', [StringComparison]::Ordinal)) { throw 'Sanitizer test executable escaped isolated compiler output.' } }
    $callee = @($testArtifacts | Where-Object { $_.profile.test -and $_.target.name -eq 'nvsf_media_native' -and $_.executable })
    $caller = @($testArtifacts | Where-Object { $_.profile.test -and $_.target.name -eq 'nvsf_app' -and $_.executable })
    $geometry = @($testArtifacts | Where-Object { $_.profile.test -and $_.target.name -eq 'media_display_geometry' -and $_.executable })
    if ($callee.Count -ne 1 -or $caller.Count -ne 1 -or $geometry.Count -ne 1) { throw 'Missing or ambiguous actual native/caller/geometry libtest executables.' }
    $symbols = Invoke-Guard 'caller-symbols' 15 5 'nm' @('-D','--defined-only',$geometry[0].executable)
    if ($symbols -notmatch '(?m)\b__asan_init\s*$') { throw 'Actual caller executable does not export its Rust ASan runtime.' }
    $null = Run-Tests 'native-abi-geometry' $callee[0].executable @()
    $null = Run-Tests 'callbacks' $caller[0].executable @('media::callback_tests::') 3
    $geometryOutput = Run-Tests 'display-geometry' $geometry[0].executable @() 2
    $loaded = @([regex]::Matches($geometryOutput,'NVSF_MEDIA_ASAN_LOADED=(\{[^\r\n]+\})'))
    if ($loaded.Count -ne 2) { throw 'Both actual Y4M tests must report their live loaded media/FFmpeg closure.' }
    $report.loaded = @($loaded | ForEach-Object { $evidence = $_.Groups[1].Value | ConvertFrom-Json -AsHashtable; if ($evidence.adapter_sha256 -ne $report.adapter_sha256 -or $evidence.libraries.Count -lt 5) { throw 'Loaded adapter identity/evidence mismatch.' }; $evidence })
    foreach ($name in $staged.Keys) { if ((Hash-File (Join-Path $stage $name)) -ne $staged[$name]) { throw 'Sanitizer staged runtime changed during execution.' } }
    foreach ($entry in $inventory.runtime_files) { if ((Hash-File (Join-Path $ordinary $entry.file)) -ne $entry.sha256) { throw 'Ordinary production runtime changed during sanitizer admission.' } }
    foreach ($entry in $report.inputs) { if ((Hash-File (Join-Path $root $entry.file)) -ne $entry.sha256) { throw 'Sanitizer source/lock input changed during admission.' } }
    $report.cleanup_verified = $true
    $report.passed = $true
    $report.failed = 0
    Write-Log 'INFO' "Completed actual instrumented media tests=$($report.tests); loaded staged adapter and unchanged release C closure verified"
} catch {
    $report.passed = $false
    $report.failed = 1
    $report.error = $_.Exception.Message
    Write-Log 'ERROR' $report.error
    throw
} finally {
    foreach ($name in $environment.Keys) {
        if ($environment[$name].present) { Set-Item -LiteralPath "Env:$name" -Value $environment[$name].value }
        else { Remove-Item -LiteralPath "Env:$name" -ErrorAction SilentlyContinue }
    }
    $report.elapsed_ms = [long]([DateTime]::UtcNow - $started).TotalMilliseconds
    try { Save-Report } finally { $writer.Dispose() }
}
