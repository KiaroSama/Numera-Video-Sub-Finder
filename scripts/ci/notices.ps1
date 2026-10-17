#requires -Version 7.4
function Copy-VisualStudioNotices([string]$Source, [string]$Destination) {
    $root = [IO.Path]::GetFullPath($Source)
    if (-not [IO.Directory]::Exists($root)) { throw 'Visual Studio distribution license terms are absent.' }
    $pending = [Collections.Generic.Queue[string]]::new()
    $pending.Enqueue($root)
    $files = [Collections.Generic.List[object]]::new()
    $names = [Collections.Generic.HashSet[string]]::new([StringComparer]::OrdinalIgnoreCase)
    $deadline = [DateTime]::UtcNow.AddSeconds(30)
    $total = 0L
    $count = 0
    while ($pending.Count) {
        $directory = Get-Item -LiteralPath $pending.Dequeue() -Force
        if ($directory.Attributes -band [IO.FileAttributes]::ReparsePoint) { throw 'Visual Studio notices may not contain links.' }
        foreach ($item in Get-ChildItem -LiteralPath $directory.FullName -Force) {
            $count++
            if ($count -gt 4096 -or [DateTime]::UtcNow -ge $deadline) { throw 'Visual Studio notice discovery exceeds its budget.' }
            if ($item.Attributes -band [IO.FileAttributes]::ReparsePoint) { throw 'Visual Studio notices may not contain links.' }
            $relative = [IO.Path]::GetRelativePath($root, $item.FullName).Replace('\', '/')
            if ($relative.Split('/').Count -gt 16 -or $relative.Length -gt 512) { throw 'Unexpected Visual Studio notice path.' }
            if ($item.PSIsContainer) { $pending.Enqueue($item.FullName); continue }
            if ($item.Extension -notin @('.txt', '.rtf', '.html', '.htm', '.pdf')) { continue }
            # Preserve the vendor's exact bytes without weakening package dotfile rejection.
            if ($item.Name -ceq '.NET_Framework_EULA.rtf') {
                $relative = $relative.Substring(0, $relative.Length - $item.Name.Length) + 'DOTNET_Framework_EULA.rtf'
            }
            if ($relative -match '(^|/)(\.[^/]*|secrets\.md|explain-AI\.md|CLAUDE\.md|AGENTS\.md|reference\.md)(/|$)|[\\:]' -or -not $names.Add($relative)) {
                throw 'Unexpected Visual Studio notice path.'
            }
            $total += $item.Length
            if ($total -gt 33554432) { throw 'Visual Studio notice bytes exceed their budget.' }
            $files.Add(@{ source = $item.FullName; relative = $relative; bytes = $item.Length; hash = (Get-FileHash -LiteralPath $item.FullName -Algorithm SHA256).Hash })
        }
    }
    if (-not $files.Count) { throw 'Visual Studio distribution license terms are absent.' }
    foreach ($file in $files | Sort-Object relative) {
        if ([DateTime]::UtcNow -ge $deadline) { throw 'Visual Studio notice copying exceeds its budget.' }
        $out = Join-Path $Destination $file.relative
        [IO.Directory]::CreateDirectory([IO.Path]::GetDirectoryName($out)) | Out-Null
        if (Test-Path -LiteralPath $out) {
            $prior = Get-Item -LiteralPath $out -Force
            if ($prior.PSIsContainer -or ($prior.Attributes -band [IO.FileAttributes]::ReparsePoint) -or $prior.Length -ne $file.bytes -or (Get-FileHash -LiteralPath $out -Algorithm SHA256).Hash -ne $file.hash) {
                throw 'Visual Studio notice destination bytes collide.'
            }
        } else { [IO.File]::Copy($file.source, $out, $false) }
        if (([IO.FileInfo]::new($out)).Length -ne $file.bytes -or (Get-FileHash -LiteralPath $out -Algorithm SHA256).Hash -ne $file.hash) {
            throw 'Copied Visual Studio notice bytes differ.'
        }
        $file.relative
    }
}
