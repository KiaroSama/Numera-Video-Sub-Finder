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
$claimed=@('crates/core','crates/app','crates/media-native','crates/cli','crates/ui','crates/desktop','xtask')
$workspace=[IO.File]::ReadAllText('Cargo.toml',$utf8)
foreach($manifest in $files|Where-Object {$_ -match '/Cargo.toml$'}) {
    $package=Split-Path $manifest -Parent
    if($package.Replace('\','/') -notin $claimed){throw 'Cargo package has no CI suite/build claim'}
}
[Console]::WriteLine('Tracked text/publication boundaries and package CI inventory validated; heuristic secret check is not an exhaustive credential scan.')
