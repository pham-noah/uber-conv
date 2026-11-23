<#
.SYNOPSIS
    Removes the per-user uberconv install (no admin rights required).

.DESCRIPTION
    Deletes the Start-menu shortcut, the four HKCU registry keys the installer
    creates (AppUserModelId registration, the two context-menu verbs and the
    Apps & features entry), the user PATH entry, and finally the install
    directory itself.

    Safe to run when nothing is installed, and safe to run twice. The registry
    clean-up does not depend on the install directory, so the AUMID entry is
    still removed even if the files were deleted by hand.

.PARAMETER Quiet
    Print nothing (used by the Apps & features UninstallString).

.PARAMETER InstallDir
    Install directory to remove. Defaults to %LOCALAPPDATA%\Programs\uberconv.

.EXAMPLE
    powershell -NoProfile -ExecutionPolicy Bypass -File .\uninstall-windows.ps1

.EXAMPLE
    powershell -NoProfile -ExecutionPolicy Bypass -File .\uninstall-windows.ps1 -Quiet
#>
[CmdletBinding()]
param(
    [switch] $Quiet,
    [string] $InstallDir
)

$ErrorActionPreference = 'Stop'

$AppName     = 'uberconv'
$Aumid       = 'NoahPham.uberconv'
$ShortcutDir = Join-Path $env:APPDATA 'Microsoft\Windows\Start Menu\Programs'
$Shortcut    = Join-Path $ShortcutDir "$AppName.lnk"

if (-not $InstallDir) { $InstallDir = Join-Path $env:LOCALAPPDATA "Programs\$AppName" }
$InstallDir = $InstallDir.TrimEnd('\')

function Say([string] $Message) { if (-not $Quiet) { Write-Host "  $Message" } }
function SayError([string] $Message) { if (-not $Quiet) { Write-Host "  $Message" -ForegroundColor Yellow } }

# Registry keys created by install-windows.ps1 (all HKCU => no admin).
$RegKeys = @(
    "Software\Classes\AppUserModelId\$Aumid",
    "Software\Classes\*\shell\$AppName",
    "Software\Classes\Directory\shell\$AppName",
    "Software\Microsoft\Windows\CurrentVersion\Uninstall\$AppName"
)

if (-not $Quiet) {
    Write-Host ''
    Write-Host "uberconv - uninstall" -ForegroundColor Cyan
    Write-Host ''
}

# --------------------------------------------------------------------------
# 1. Close a running copy (its image stays locked otherwise)
# --------------------------------------------------------------------------
try {
    $running = @(Get-Process -Name "$AppName-app" -ErrorAction SilentlyContinue)
    if ($running.Count -gt 0) {
        $running | Stop-Process -Force -ErrorAction SilentlyContinue
        Start-Sleep -Milliseconds 700
        Say "Closed running uberconv-app.exe (was blocking file removal)"
    }
} catch {
    SayError "Could not close running uberconv-app.exe: $($_.Exception.Message)"
}

# --------------------------------------------------------------------------
# 2. Start-menu shortcut
# --------------------------------------------------------------------------
try {
    if (Test-Path -LiteralPath $Shortcut) {
        Remove-Item -LiteralPath $Shortcut -Force
        Say "Removed shortcut: $Shortcut"
    } else {
        Say "Shortcut already absent: $Shortcut"
    }
} catch {
    SayError "Could not remove shortcut '$Shortcut': $($_.Exception.Message)"
}

# --------------------------------------------------------------------------
# 3. Registry keys (independent of the install dir, so this still works when
#    the app files are already gone)
# --------------------------------------------------------------------------
foreach ($sub in $RegKeys) {
    try {
        $key = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey($sub)
        $exists = $null -ne $key
        if ($key) { $key.Close() }
        if ($exists) {
            [Microsoft.Win32.Registry]::CurrentUser.DeleteSubKeyTree($sub, $false)
            Say "Removed key    : HKCU\$sub"
        } else {
            Say "Key absent     : HKCU\$sub"
        }
    } catch {
        SayError "Could not remove HKCU\$sub : $($_.Exception.Message)"
    }
}

# --------------------------------------------------------------------------
# 4. User PATH
# --------------------------------------------------------------------------
function Send-EnvironmentChanged {
    try {
        if (-not ('WinEnvBroadcast' -as [type])) {
            Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class WinEnvBroadcast
{
    [DllImport("user32.dll", SetLastError = true, CharSet = CharSet.Unicode)]
    private static extern IntPtr SendMessageTimeout(IntPtr hWnd, uint Msg, UIntPtr wParam,
        string lParam, uint fuFlags, uint uTimeout, out UIntPtr lpdwResult);

    public static void Broadcast()
    {
        UIntPtr result;
        SendMessageTimeout(new IntPtr(0xffff), 0x001A, UIntPtr.Zero, "Environment", 0x0002, 2000, out result);
    }
}
'@
        }
        [WinEnvBroadcast]::Broadcast()
    } catch {
        # Cosmetic only.
    }
}

try {
    $envKey = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey('Environment', $true)
    if ($envKey) {
        try {
            $names = @($envKey.GetValueNames())
            if ($names -contains 'Path') {
                $current = [string]$envKey.GetValue('Path', '', [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
                $kind = $envKey.GetValueKind('Path')
                if ($kind -ne [Microsoft.Win32.RegistryValueKind]::ExpandString) {
                    $kind = [Microsoft.Win32.RegistryValueKind]::String
                }
                $wanted = $InstallDir.ToLowerInvariant()
                $kept = @()
                $removed = 0
                foreach ($part in $current.Split(';')) {
                    # Keep every other entry verbatim; only trim for the compare
                    # so an empty segment (which means "current dir") survives.
                    if (-not [string]::IsNullOrEmpty($part.Trim()) -and
                        $part.Trim().TrimEnd('\').ToLowerInvariant() -eq $wanted) {
                        $removed++
                    } else {
                        $kept += $part
                    }
                }
                if ($removed -gt 0) {
                    if (@($kept | Where-Object { -not [string]::IsNullOrEmpty($_.Trim()) }).Count -eq 0) {
                        $envKey.DeleteValue('Path', $false)
                    } else {
                        $envKey.SetValue('Path', ($kept -join ';'), $kind)
                    }
                    Send-EnvironmentChanged
                    Say "Removed $removed PATH entry/entries: $InstallDir"
                } else {
                    Say "PATH entry already absent: $InstallDir"
                }
            } else {
                Say "PATH entry already absent: $InstallDir (no user Path value)"
            }
        } finally {
            $envKey.Close()
        }
    } else {
        Say "PATH entry already absent: $InstallDir (no Environment key)"
    }
} catch {
    SayError "Could not edit the user PATH: $($_.Exception.Message)"
}

# --------------------------------------------------------------------------
# 5. Install directory
# --------------------------------------------------------------------------
if (Test-Path -LiteralPath $InstallDir) {
    $selfPath = $PSCommandPath
    $runningFromInstallDir = $false
    if ($selfPath) {
        $selfFull = [System.IO.Path]::GetFullPath($selfPath)
        $dirFull = [System.IO.Path]::GetFullPath($InstallDir) + [System.IO.Path]::DirectorySeparatorChar
        if ($selfFull.StartsWith($dirFull, [System.StringComparison]::OrdinalIgnoreCase)) {
            $runningFromInstallDir = $true
        }
    }

    try {
        if ($runningFromInstallDir) {
            # Cannot delete the script that is executing: wipe the payload,
            # then queue the (now tiny) directory for deletion after we exit.
            foreach ($entry in @(Get-ChildItem -LiteralPath $InstallDir -Force)) {
                if ($entry.FullName -ieq $selfPath) { continue }
                try {
                    Remove-Item -LiteralPath $entry.FullName -Recurse -Force -ErrorAction Stop
                } catch {
                    SayError "Could not remove '$($entry.FullName)': $($_.Exception.Message)"
                }
            }
            $cmd = 'ping -n 2 127.0.0.1 >nul & rd /s /q "' + $InstallDir + '"'
            Start-Process -FilePath $env:ComSpec -ArgumentList '/d','/c', $cmd -WindowStyle Hidden | Out-Null
            Say "Scheduled removal of $InstallDir (running from inside it)"
        } else {
            Remove-Item -LiteralPath $InstallDir -Recurse -Force
            Say "Removed install dir: $InstallDir"
        }
    } catch {
        # Fall back to a deferred delete if something is still holding a file.
        $cmd = 'ping -n 2 127.0.0.1 >nul & rd /s /q "' + $InstallDir + '"'
        Start-Process -FilePath $env:ComSpec -ArgumentList '/d','/c', $cmd -WindowStyle Hidden | Out-Null
        SayError "Install dir was in use; scheduled removal of $InstallDir"
    }
} else {
    Say "Install dir already absent: $InstallDir"
}

if (-not $Quiet) {
    Write-Host ''
    Write-Host "uberconv uninstalled. The context-menu, Apps & features and PATH entries are gone." -ForegroundColor Cyan
    Write-Host ''
}

exit 0
