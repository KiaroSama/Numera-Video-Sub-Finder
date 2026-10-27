#requires -Version 7.4
$ErrorActionPreference='Stop'
Set-StrictMode -Version Latest
$utf8=[Text.UTF8Encoding]::new($false,$true)
$files=@(& git ls-files)
if($LASTEXITCODE -ne 0 -or $files.Count -eq 0){throw 'No tracked source inventory'}
foreach($file in $files) {
    if($file -match '(^|/)(\.ai|\.specify|specs|\.claude|\.kiro|\.codex|\.agents|\.cursor|\.cline|graphify-out|\.codebase-memory|plans|\.ignoreme)(/|$)' -or $file -match '(^|/)(secrets\.md|explain-AI\.md|CLAUDE\.md|AGENTS\.md|reference\.md|\.env(?:\..*)?)$') {throw 'Excluded local/private content is tracked'}
    if($file -match '\.(rs|toml|lock|json|ya?ml|ps1|md|html|css|cmake)$' -or $file -in @('LICENSE','.gitignore','.graphifyignore')) {
        $text=[IO.File]::ReadAllText((Join-Path (Get-Location) $file),$utf8)
        if($file.EndsWith('.rs') -and ($text.Split("`n").Count -gt 800)){throw "Owned source exceeds file ceiling: $file"}
        if($text -match '-----BEGIN (?:RSA |EC |OPENSSH )?PRIVATE KEY-----' -or $text -match '(?:ghp_|github_pat_)[A-Za-z0-9_]{30,}') {throw 'Potential real secret in tracked content; publication stops'}
    }
}
$claims=@{
    'crates/core'=@('.github/workflows/ci.yml','native')
    'crates/app'=@('.github/workflows/ci.yml','native')
    'crates/media-native'=@('.github/workflows/ci.yml','native')
    'crates/cli'=@('.github/workflows/ci.yml','native')
    'crates/ui'=@('.github/workflows/ci.yml','native')
    'crates/desktop'=@('.github/workflows/ci.yml','native')
    'xtask'=@('.github/workflows/ci.yml','native')
    # Standalone test=false binaries are built/replayed/fuzzed with ASan, not cargo test.
    'fuzz'=@('.github/workflows/quality.yml','fuzz')
}
foreach($manifest in $files|Where-Object {$_ -match '/Cargo.toml$'}) {
    $package=(Split-Path $manifest -Parent).Replace('\','/')
    if(-not $claims.ContainsKey($package)){throw 'Cargo package has no CI suite/build claim'}
    $claim=$claims[$package]
    if($claim[0] -notin $files){throw 'Cargo package CI claim references an untracked workflow'}
    $workflow=[IO.File]::ReadAllText($claim[0],$utf8)
    $job=[regex]::Match($workflow,('(?ms)^  '+[regex]::Escape($claim[1])+':\r?\n(?<body>.*?)(?=^  [A-Za-z0-9_-]+:\r?$|\z)'))
    if(-not $job.Success){throw 'Cargo package CI claim references a missing job'}
    if($package -eq 'fuzz') {
        if('scripts/ci/fuzz.ps1' -notin $files -or $job.Groups['body'].Value -notmatch '(?m)^        run: \./scripts/ci/fuzz\.ps1\r?$') {
            throw 'Standalone fuzz package requires its actual portable fuzz runner'
        }
    }
}
[Console]::WriteLine('Tracked text/publication boundaries and package CI inventory validated; heuristic secret check is not an exhaustive credential scan.')
