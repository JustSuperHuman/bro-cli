# Fresh demo windows only: no capture, input injection, downloads or session changes.
param(
    [Parameter(Mandatory = $true)][string]$Binary,
    [Parameter(Mandatory = $true)][string]$Output
)
$ErrorActionPreference = 'Stop'
$Binary = (Resolve-Path -LiteralPath $Binary).Path
if (Test-Path -LiteralPath $Output) { throw 'Choose a new evidence directory.' }
[void][IO.Directory]::CreateDirectory($Output)
$Output = (Resolve-Path -LiteralPath $Output).Path
$names = @('BRO_DIR','BRO_GUI_SCRIPT','BRO_GUI_SIZE','BRO_GUI_SCREENSHOT','BRO_GUI_EXIT')
$saved = @{}
foreach ($name in $names) { $saved[$name] = [Environment]::GetEnvironmentVariable($name, 'Process') }
try {
    foreach ($size in @('1120x820','760x700')) {
        foreach ($theme in @('graphite','paper')) {
            $label = "$size-$theme"
            $env:BRO_DIR = Join-Path $Output "$label-profile"
            $env:BRO_GUI_SIZE = $size
            $env:BRO_GUI_SCREENSHOT = $null
            $env:BRO_GUI_EXIT = $null
            $off = Join-Path $Output "$label-off.png"
            $on = Join-Path $Output "$label-on.png"
            $watch = Join-Path $Output "$label-watch.png"
            $env:BRO_GUI_SCRIPT = @('wait:1400','key:alt+,','wait:300','key:end',"theme:$theme",'wait:300',"shot:$off",
                'key:enter','wait:300',"shot:$on",'key:down','key:enter','wait:300',"shot:$watch",'exit') -join ';'
            $child = Start-Process -FilePath $Binary -ArgumentList @('gui','--gui-child','--demo') -WindowStyle Hidden -PassThru
            if (-not $child.WaitForExit(45000)) {
                $child.Kill($true) # This exact fixture-owned process handle only.
                throw "The isolated $label settings window timed out."
            }
            if ($child.ExitCode -ne 0) { throw "The isolated $label settings window failed: $($child.ExitCode)." }
            foreach ($capture in @($off,$on,$watch)) {
                if (-not (Test-Path -LiteralPath $capture) -or (Get-Item -LiteralPath $capture).Length -lt 1024) { throw "Missing capture: $capture" }
            }
        }
    }
    "PASS 12 isolated Mobile Companion settings captures: $Output"
} finally {
    foreach ($name in $names) { [Environment]::SetEnvironmentVariable($name, $saved[$name], 'Process') }
}
