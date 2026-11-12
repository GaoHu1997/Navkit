param([string]$Root = 'D:\github\tauri-navicat')
$ErrorActionPreference = 'Stop'
$src = Join-Path $Root 'src-tauri\target\release\navkit.exe'
$dst = Join-Path $Root 'dist-portable\navkit.exe'
$data = Join-Path $Root 'dist-portable\data'
$before = @{}
Get-ChildItem -LiteralPath $data -Recurse -File | ForEach-Object {
    $before[$_.FullName] = (Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash
}
Copy-Item -LiteralPath $src -Destination $dst -Force
$readme = Get-ChildItem -LiteralPath (Join-Path $Root 'assets') -Filter '*.txt' | Select-Object -First 1
Copy-Item -LiteralPath $readme.FullName -Destination (Join-Path $Root 'dist-portable\README-portable.txt') -Force
$after = @{}
Get-ChildItem -LiteralPath $data -Recurse -File | ForEach-Object {
    $after[$_.FullName] = (Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash
}
if ($after.Count -ne $before.Count) { throw 'Portable data file set changed' }
foreach ($p in $before.Keys) {
    if (-not $after.ContainsKey($p) -or $after[$p] -ne $before[$p]) {
        throw "Portable data changed: $p"
    }
}
$sourceHash = (Get-FileHash -LiteralPath $src -Algorithm SHA256).Hash
$targetHash = (Get-FileHash -LiteralPath $dst -Algorithm SHA256).Hash
if ($sourceHash -ne $targetHash) { throw 'Executable checksum mismatch' }
[pscustomobject]@{
    executable = $dst
    sha256 = $targetHash
    preservedFiles = $before.Count
    dataUnchanged = $true
} | ConvertTo-Json
