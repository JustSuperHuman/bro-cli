# bro installer and updater for Windows.
#   irm https://raw.githubusercontent.com/JustSuperHuman/bro-cli/main/install.ps1 | iex
# With bro already installed it updates that copy in place, even while it's running; otherwise it
# installs to %LOCALAPPDATA%\Programs\bro and adds that to your user PATH.
# Env: BRO_VERSION (tag, default latest), BRO_INSTALL_DIR (default: the folder of the bro on PATH,
#      else %LOCALAPPDATA%\Programs\bro),
#      BRO_TOOLS (Quick Install too, e.g. "justtools git node claude", "recommended", "all" or "none";
#      default: ask on a fresh install, skip on an update).
# Wrapped in a script block so `irm | iex` leaves the caller's session untouched.
& {
    $ErrorActionPreference = 'Stop'
    $ProgressPreference = 'SilentlyContinue'
    [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12

    $repo = 'JustSuperHuman/bro-cli'
    $target = 'x86_64-pc-windows-msvc'

    # an existing bro is updated where it is
    $dir = $env:BRO_INSTALL_DIR
    if (-not $dir) {
        $found = Get-Command bro.exe -CommandType Application -ErrorAction SilentlyContinue | Select-Object -First 1
        $dir = if ($found) { Split-Path $found.Source } else { Join-Path $env:LOCALAPPDATA 'Programs\bro' }
    }
    $bro = Join-Path $dir 'bro.exe'
    $current = if (Test-Path $bro) { ("$(& $bro --version)" -replace '^bro\s+', '').Trim() } else { '' }

    $tag = $env:BRO_VERSION
    if (-not $tag) {
        $tag = (Invoke-RestMethod "https://api.github.com/repos/$repo/releases/latest" -Headers @{ 'User-Agent' = 'bro-installer' }).tag_name
    }
    $update = [bool]$current
    if ($current -and "v$current" -eq $tag) {
        Write-Host "bro $current in $dir is already the newest release."
    } else {
        if ($update) { Write-Host "Updating bro $current in $dir to $tag" } else { Write-Host "Installing bro $tag to $dir" }
        $name = "bro-$tag-$target"
        $base = "https://github.com/$repo/releases/download/$tag"
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
            if ($want -ne $got) { throw "Checksum mismatch for $name.zip; nothing was changed" }
            Expand-Archive $zip -DestinationPath $tmp -Force

            New-Item -ItemType Directory -Force $dir | Out-Null
            # only the executables: the README and LICENSE don't belong in a folder on PATH
            foreach ($file in Get-ChildItem (Join-Path $tmp $name) -File -Filter *.exe) {
                $dest = Join-Path $dir $file.Name
                $staged = Join-Path $dir ".$($file.Name).new"
                Copy-Item $file.FullName $staged -Force
                if (Test-Path $dest) {
                    # a running exe can't be overwritten but can be renamed; bro deletes the .old file when it next starts
                    $old = "$dest.old"
                    if (Test-Path $old) {
                        try { Remove-Item $old -Force } catch { $old = "$dest.$PID.old" }
                    }
                    Move-Item $dest $old -Force
                }
                Move-Item $staged $dest -Force
            }
        } finally {
            Remove-Item $tmp -Recurse -Force -ErrorAction SilentlyContinue
        }
        $now = "$(& $bro --version)".Trim()
        if ($update) { Write-Host "$now installed. Restart bro to use it." } else { Write-Host "$now installed." }
    }

    $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
    $onPath = ($env:Path -split ';') -contains $dir -or ($userPath -split ';') -contains $dir
    if (-not $onPath) {
        [Environment]::SetEnvironmentVariable('Path', ($(if ($userPath) { "$userPath;" }) + $dir), 'User')
        $env:Path = "$env:Path;$dir"
        Write-Host "Added $dir to your user PATH. Open a new terminal if 'bro' is not found."
    }

    # Offer Quick Install (JustTools, Git, Node.js, Bun, Python, .NET, the coding agents…) on a fresh
    # install; releases before `bro install` skip this.
    if ("$(& $bro --help)" -match 'bro install') {
        $tools = "$env:BRO_TOOLS".Trim()
        if ($tools -eq 'none') {
        } elseif ($tools) {
            & $bro install @($tools -split '[\s,]+' | Where-Object { $_ })
        } elseif (-not $update -and [Environment]::UserInteractive -and -not [Console]::IsInputRedirected) {
            Write-Host ""
            & $bro install
        }
    }
    if (-not $update) { Write-Host "Run: bro" }
}
