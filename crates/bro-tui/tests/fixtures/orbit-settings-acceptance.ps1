# Drives only fresh, isolated demo windows. No microphone/provider/desktop automation.
param(
    [Parameter(Mandatory = $true)][string]$Binary,
    [string]$Output = (Join-Path ([IO.Path]::GetTempPath()) ('bro-orbit-settings-' + [guid]::NewGuid().ToString('N')))
)
$ErrorActionPreference = 'Stop'
$Binary = (Resolve-Path -LiteralPath $Binary).Path
if (Test-Path -LiteralPath $Output) { throw 'Choose a new, empty evidence directory.' }
[void][IO.Directory]::CreateDirectory($Output)
$names = @('BRO_DIR','CLAUDE_POOL_DIR','BRO_CODEX_PROFILES_DIR','HOME','USERPROFILE','APPDATA','LOCALAPPDATA',
    'BRO_GUI_SCRIPT','BRO_GUI_SIZE','BRO_GUI_SCREENSHOT','BRO_GUI_EXIT','BRO_ORBIT_EXECUTABLE','BRO_ORBIT_APPLICATION_DIR')
$saved = @{}
foreach ($name in $names) { $saved[$name] = [Environment]::GetEnvironmentVariable($name, 'Process') }
$states = @('off','connecting','error','setup','ready','talking')
$captures = @()
try {
    foreach ($size in @('1120x820','760x700')) {
        foreach ($theme in @('graphite','paper')) {
            $label = "$size-$theme"
            $caseRoot = Join-Path $Output $label
            [void][IO.Directory]::CreateDirectory($caseRoot)
            foreach ($pair in @(@('BRO_DIR','bro'),@('CLAUDE_POOL_DIR','pool'),@('BRO_CODEX_PROFILES_DIR','codex'),
                @('HOME','home'),@('USERPROFILE','home'),@('APPDATA','appdata'),@('LOCALAPPDATA','local'))) {
                $path = Join-Path $caseRoot $pair[1]
                [void][IO.Directory]::CreateDirectory($path)
                [Environment]::SetEnvironmentVariable($pair[0], $path, 'Process')
            }
            $env:BRO_ORBIT_EXECUTABLE = Join-Path $caseRoot 'missing-electron.exe'
            $env:BRO_ORBIT_APPLICATION_DIR = Join-Path $caseRoot 'missing-preview'
            $env:BRO_GUI_SCREENSHOT = $null
            $env:BRO_GUI_EXIT = $null
            $env:BRO_GUI_SIZE = $size
            $steps = @('wait:1500', 'key:alt+,', 'wait:250', 'key:tab', 'key:tab', 'key:tab', "theme:$theme", 'wait:400')
            foreach ($state in $states) {
                $file = Join-Path $Output "$label-$state.png"
                $steps += @("orbit-fixture:$state", 'wait:500', "shot:$file")
                $captures += $file
            }
            $env:BRO_GUI_SCRIPT = ($steps + 'exit') -join ';'
            $child = Start-Process -FilePath $Binary -ArgumentList @('gui','--gui-child','--demo') -WindowStyle Hidden -PassThru
            if (-not $child.WaitForExit(60000)) {
                $child.Kill($true) # Only this exact fixture-owned process handle.
                throw "The isolated $label window timed out."
            }
            if ($child.ExitCode -ne 0) { throw "The isolated $label window exited with $($child.ExitCode)." }
        }
    }
    foreach ($capture in $captures) {
        if (-not (Test-Path -LiteralPath $capture) -or (Get-Item -LiteralPath $capture).Length -lt 1024) { throw "Missing settings capture: $capture" }
    }
    $result = @{ demoOnly = $true; deviceFree = $true; binary = $Binary; captures = $captures }
    [IO.File]::WriteAllText((Join-Path $Output 'captures.json'), ($result | ConvertTo-Json -Depth 4))
    "PASS $($captures.Count) isolated settings captures: $Output"
} finally {
    foreach ($name in $names) { [Environment]::SetEnvironmentVariable($name, $saved[$name], 'Process') }
}
