# bro installer for Windows.
#   irm https://raw.githubusercontent.com/JustSuperHuman/bro-cli/main/install.ps1 | iex
# Env: BRO_VERSION (tag, default latest), BRO_INSTALL_DIR (default %LOCALAPPDATA%\Programs\bro).
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12

$repo = 'JustSuperHuman/bro-cli'
$target = 'x86_64-pc-windows-msvc'
$dir = if ($env:BRO_INSTALL_DIR) { $env:BRO_INSTALL_DIR } else { Join-Path $env:LOCALAPPDATA 'Programs\bro' }

$tag = $env:BRO_VERSION
if (-not $tag) {
    $tag = (Invoke-RestMethod "https://api.github.com/repos/$repo/releases/latest" -Headers @{ 'User-Agent' = 'bro-installer' }).tag_name
}
$name = "bro-$tag-$target"
$base = "https://github.com/$repo/releases/download/$tag"
Write-Host "Installing bro $tag to $dir"

$tmp = Join-Path ([IO.Path]::GetTempPath()) ("bro-install-" + [guid]::NewGuid())
New-Item -ItemType Directory -Force $tmp | Out-Null
try {
    $zip = Join-Path $tmp "$name.zip"
    Invoke-WebRequest "$base/$name.zip" -OutFile $zip -UseBasicParsing
    $sumsFile = Join-Path $tmp 'SHA256SUMS.txt'
    Invoke-WebRequest "$base/SHA256SUMS.txt" -OutFile $sumsFile -UseBasicParsing
    $sums = Get-Content $sumsFile -Raw
    $line = ($sums -split "`n") | Where-Object { $_ -match "\s\*?$([regex]::Escape("$name.zip"))\s*$" } | Select-Object -First 1
    if (-not $line) { throw "No checksum for $name.zip in SHA256SUMS.txt" }
    $want = ($line -split '\s+')[0].ToLower()
    $got = (Get-FileHash $zip -Algorithm SHA256).Hash.ToLower()
    if ($want -ne $got) { throw "Checksum mismatch for $name.zip" }

    Expand-Archive $zip -DestinationPath $tmp -Force
    New-Item -ItemType Directory -Force $dir | Out-Null
    Get-Process bro, bro-app -ErrorAction SilentlyContinue | Where-Object { $_.Path -like "$dir\*" } |
        ForEach-Object { throw "bro is running from $dir; close it and rerun the installer." }
    Copy-Item (Join-Path $tmp "$name\*") $dir -Recurse -Force
} finally {
    Remove-Item $tmp -Recurse -Force -ErrorAction SilentlyContinue
}

$userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
if (-not (($userPath -split ';') -contains $dir)) {
    [Environment]::SetEnvironmentVariable('Path', ($(if ($userPath) { "$userPath;" }) + $dir), 'User')
    $env:Path = "$env:Path;$dir"
    Write-Host "Added $dir to your user PATH. Open a new terminal if 'bro' is not found."
}
Write-Host "bro $tag installed. Run: bro"
