$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = New-Object System.Text.UTF8Encoding $false
# Parse native argv so quoted paths, extended paths and Electron helpers cannot
# masquerade as the selected application or its original managed pairing.
Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class BroOrbitArguments {
    [DllImport("shell32.dll", SetLastError = true)]
    static extern IntPtr CommandLineToArgvW([MarshalAs(UnmanagedType.LPWStr)] string command, out int count);
    [DllImport("kernel32.dll")]
    static extern IntPtr LocalFree(IntPtr value);
    public static string[] Parse(string command) {
        int count;
        IntPtr argv = CommandLineToArgvW(command, out count);
        if (argv == IntPtr.Zero) throw new System.ComponentModel.Win32Exception();
        try {
            string[] result = new string[count];
            for (int i = 0; i < count; i++) result[i] = Marshal.PtrToStringUni(Marshal.ReadIntPtr(argv, i * IntPtr.Size));
            return result;
        } finally { LocalFree(argv); }
    }
}
'@
function Normalize-OrbitPath([string]$value) {
    if (!$value) { return '' }
    return $value.Replace('/', '\').Replace('\\?\', '').TrimEnd('\')
}
$exe = Normalize-OrbitPath $env:BRO_ORBIT_SCAN_EXE
$app = Normalize-OrbitPath $env:BRO_ORBIT_SCAN_APP
@(Get-CimInstance Win32_Process | ForEach-Object {
    $named = $_.Name -match '^orbit([ ._-].*)?\.exe$'
    $same = $exe -and (Normalize-OrbitPath $_.ExecutablePath) -eq $exe
    if (!$named -and !$same) { return }
    $arguments = if ($_.CommandLine) { @([BroOrbitArguments]::Parse($_.CommandLine)) } else { @() }
    if (@($arguments | Where-Object { $_ -like '--type=*' }).Count) { return }
    $selected = $same -and (!$app -or ($arguments.Count -gt 1 -and (Normalize-OrbitPath $arguments[1]) -eq $app))
    if (!$named -and !$selected) { return }
    $descriptor = $null
    $identity = $null
    if ($selected) {
        $managedFlags = @($arguments | Where-Object { $_ -ceq '--bro-managed' })
        $descriptorFlags = @($arguments | Where-Object { $_ -ceq '--external-host-file' })
        $index = [Array]::IndexOf($arguments, '--external-host-file')
        if ($managedFlags.Count -eq 1 -and $descriptorFlags.Count -eq 1 -and $index -ge 0 -and $index + 1 -lt $arguments.Count) {
            $descriptor = Normalize-OrbitPath $arguments[$index + 1]
        }
        try {
            $native = Get-Process -Id $_.ProcessId -ErrorAction Stop
            if ((Normalize-OrbitPath $native.Path) -eq $exe) {
                $identity = 'windows:{0:x16}' -f $native.StartTime.ToUniversalTime().ToFileTimeUtc()
            }
        } catch { } # An exiting or unreadable process cannot authorize recovery.
    }
    [pscustomobject]@{
        pid = $_.ProcessId
        executable = $_.ExecutablePath
        selected = [bool]$selected
        managed_descriptor = $descriptor
        process_identity = $identity
    }
}) | ConvertTo-Json -Compress
