<#
.SYNOPSIS
    Per-user installer for uberconv (no admin rights required).

.DESCRIPTION
    Copies uberconv-app.exe + uberconv.exe (and ffmpeg.exe when it sits next to
    them) into %LOCALAPPDATA%\Programs\uberconv, creates a Start-menu shortcut
    whose AppUserModelID is NoahPham.uberconv, registers that AUMID under
    HKCU\Software\Classes\AppUserModelId, adds "Convert with uberconv..." verbs
    to the file and folder context menus, registers an Apps & features entry,
    and puts the install dir on the user PATH.

    Everything is per-user (HKCU / %LOCALAPPDATA% / %APPDATA%) so it works
    without elevation. Re-running is safe: files are overwritten, the shortcut
    and registry keys are rewritten in place, and the PATH entry is only added
    once.

.PARAMETER Source
    Directory containing uberconv-app.exe and uberconv.exe.
    Defaults to <repo>\target\release, where <repo> is the parent of this
    script's folder.

.PARAMETER NoPath
    Skip adding the install dir to the user PATH.

.EXAMPLE
    powershell -NoProfile -ExecutionPolicy Bypass -File .\install-windows.ps1

.EXAMPLE
    powershell -NoProfile -ExecutionPolicy Bypass -File .\install-windows.ps1 `
        -Source 'F:\App Projects\Uberconv\target\release' -NoPath
#>
[CmdletBinding()]
param(
    [string] $Source,
    [switch] $NoPath
)

$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

# --------------------------------------------------------------------------
# Constants
# --------------------------------------------------------------------------
$AppName     = 'uberconv'
$Aumid       = 'NoahPham.uberconv'
$Publisher   = 'Noah Pham'
$Summary     = 'uberconv - convert anything to anything'
$GuiExeName  = 'uberconv-app.exe'
$CliExeName  = 'uberconv.exe'
$InstallDir  = Join-Path $env:LOCALAPPDATA "Programs\$AppName"
$GuiExe      = Join-Path $InstallDir $GuiExeName
$CliExe      = Join-Path $InstallDir $CliExeName
$ShortcutDir = Join-Path $env:APPDATA 'Microsoft\Windows\Start Menu\Programs'
$Shortcut    = Join-Path $ShortcutDir "$AppName.lnk"

# Registry locations (all HKCU => no admin).
$RegAumid     = "HKCU:\Software\Classes\AppUserModelId\$Aumid"
$RegVerbFile  = "HKCU:\Software\Classes\*\shell\$AppName"
$RegVerbDir   = "HKCU:\Software\Classes\Directory\shell\$AppName"
$RegUninstall = "HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\$AppName"

function Write-Step([string] $Message) { Write-Host "  $Message" }

# Fatal setup error: message on stderr, non-zero exit, no stack trace noise.
function Stop-Install([string[]] $Lines) {
    Write-Host ''
    foreach ($line in $Lines) { $Host.UI.WriteErrorLine($line) }
    Write-Host ''
    exit 1
}

# --------------------------------------------------------------------------
# 1. Locate the payload
# --------------------------------------------------------------------------
Write-Host ''
Write-Host "uberconv - per-user install" -ForegroundColor Cyan
Write-Host ''

if (-not $Source) {
    $repoRoot = Split-Path -Parent $PSScriptRoot
    $Source = Join-Path $repoRoot 'target\release'
}

$SourceItem = Get-Item -LiteralPath $Source -ErrorAction SilentlyContinue
if (-not $SourceItem -or -not $SourceItem.PSIsContainer) {
    Stop-Install @(
        "Source directory not found: '$Source'.",
        "Pass -Source <dir> pointing at the folder that holds $GuiExeName and $CliExeName."
    )
}
$Source = $SourceItem.FullName

$missing = @()
foreach ($n in @($GuiExeName, $CliExeName)) {
    if (-not (Test-Path -LiteralPath (Join-Path $Source $n) -PathType Leaf)) { $missing += $n }
}
if ($missing.Count -gt 0) {
    Stop-Install @(
        "Missing $($missing -join ' and ') in '$Source'.",
        'Build them first:  cargo build --release   (from the repo root),',
        'or pass -Source <dir> pointing at the release folder.'
    )
}

Write-Step "Source      : $Source"
Write-Step "Install dir : $InstallDir"

# --------------------------------------------------------------------------
# 2. Copy the binaries
# --------------------------------------------------------------------------
if (-not (Test-Path -LiteralPath $InstallDir)) {
    New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
}

# A running GUI copy holds its image file open; close it so the overwrite works.
$procName = [System.IO.Path]::GetFileNameWithoutExtension($GuiExeName)
$running = @(Get-Process -Name $procName -ErrorAction SilentlyContinue)
if ($running.Count -gt 0) {
    Write-Step "Closing the running $GuiExeName instance(s) so the files can be replaced."
    $running | Stop-Process -Force -ErrorAction SilentlyContinue
    Start-Sleep -Milliseconds 700
}

foreach ($n in @($GuiExeName, $CliExeName)) {
    $src = Join-Path $Source $n
    $dst = Join-Path $InstallDir $n
    try {
        Copy-Item -LiteralPath $src -Destination $dst -Force
    } catch {
        Stop-Install @(
            "Could not copy '$src' to '$dst': $($_.Exception.Message)",
            'Close uberconv if it is running, then try again.'
        )
    }
}

# ffmpeg is optional but required for audio/video conversion.
$ffmpegSrc = Join-Path $Source 'ffmpeg.exe'
$ffmpegDst = Join-Path $InstallDir 'ffmpeg.exe'
$hasFfmpeg = Test-Path -LiteralPath $ffmpegSrc -PathType Leaf
if ($hasFfmpeg) {
    Copy-Item -LiteralPath $ffmpegSrc -Destination $ffmpegDst -Force
    Write-Step "Copied      : $GuiExeName, $CliExeName, ffmpeg.exe"
} else {
    Write-Step "Copied      : $GuiExeName, $CliExeName (no ffmpeg.exe in source - audio/video conversion will be unavailable)"
}

# --------------------------------------------------------------------------
# 3. Version metadata from the exe's file resource
# --------------------------------------------------------------------------
$version = '0.0.0'
try {
    $vi = (Get-Item -LiteralPath $GuiExe).VersionInfo
    $candidate = $vi.FileVersion
    if ([string]::IsNullOrWhiteSpace($candidate)) { $candidate = $vi.ProductVersion }
    if (-not [string]::IsNullOrWhiteSpace($candidate)) {
        $first = ($candidate.Trim() -split '\s+')[0]
        if ($first -match '^\d+(\.\d+){1,3}$') { $version = $first }
    }
} catch {
    $version = '0.0.0'
}
Write-Step "Version     : $version"

# --------------------------------------------------------------------------
# 4. Start-menu shortcut with a custom AppUserModelID
#    WScript.Shell / Shell.Application cannot write PKEY_AppUserModel_ID, so a
#    tiny IShellLinkW + IPropertyStore helper does it.
# --------------------------------------------------------------------------
$shellLinkSource = @'
using System;
using System.Text;
using System.Runtime.InteropServices;

public static class UberconvShellLink
{
    [StructLayout(LayoutKind.Sequential, Pack = 4)]
    public struct PROPERTYKEY
    {
        public Guid fmtid;
        public int pid;
    }

    // 24 bytes on x64: 2-byte VT + 6 bytes padding + 16-byte union.
    [StructLayout(LayoutKind.Sequential, Pack = 8)]
    public struct PROPVARIANT
    {
        public ushort vt;
        public ushort r1;
        public ushort r2;
        public ushort r3;
        public IntPtr p;
        public IntPtr p2;
    }

    [ComImport, Guid("000214F9-0000-0000-C000-000000000046"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
    private interface IShellLinkW
    {
        void GetPath([MarshalAs(UnmanagedType.LPWStr)] StringBuilder pszFile, int cch, IntPtr pfd, uint fFlags);
        void GetIDList(out IntPtr ppidl);
        void SetIDList(IntPtr pidl);
        void GetDescription([MarshalAs(UnmanagedType.LPWStr)] StringBuilder pszName, int cch);
        void SetDescription([MarshalAs(UnmanagedType.LPWStr)] string pszName);
        void GetWorkingDirectory([MarshalAs(UnmanagedType.LPWStr)] StringBuilder pszDir, int cch);
        void SetWorkingDirectory([MarshalAs(UnmanagedType.LPWStr)] string pszDir);
        void GetArguments([MarshalAs(UnmanagedType.LPWStr)] StringBuilder pszArgs, int cch);
        void SetArguments([MarshalAs(UnmanagedType.LPWStr)] string pszArgs);
        void GetHotkey(out short pwHotkey);
        void SetHotkey(short wHotkey);
        void GetShowCmd(out int piShowCmd);
        void SetShowCmd(int iShowCmd);
        void GetIconLocation([MarshalAs(UnmanagedType.LPWStr)] StringBuilder pszIconPath, int cch, out int piIcon);
        void SetIconLocation([MarshalAs(UnmanagedType.LPWStr)] string pszIconPath, int iIcon);
        void SetRelativePath([MarshalAs(UnmanagedType.LPWStr)] string pszPathRel, uint dwReserved);
        void Resolve(IntPtr hwnd, uint fFlags);
        void SetPath([MarshalAs(UnmanagedType.LPWStr)] string pszFile);
    }

    [ComImport, Guid("0000010b-0000-0000-C000-000000000046"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
    private interface IPersistFile
    {
        void GetClassID(out Guid pClassID);
        [PreserveSig] int IsDirty();
        void Load([MarshalAs(UnmanagedType.LPWStr)] string pszFileName, uint dwMode);
        void Save([MarshalAs(UnmanagedType.LPWStr)] string pszFileName, [MarshalAs(UnmanagedType.Bool)] bool fRemember);
        void SaveCompleted([MarshalAs(UnmanagedType.LPWStr)] string pszFileName);
        void GetCurFile([MarshalAs(UnmanagedType.LPWStr)] out string ppszFileName);
    }

    [ComImport, Guid("886D8EEB-8CF2-4446-8D02-CDBA1DBDCF99"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
    private interface IPropertyStore
    {
        void GetCount(out uint cProps);
        void GetAt(uint iProp, out PROPERTYKEY pkey);
        void GetValue(ref PROPERTYKEY key, out PROPVARIANT pv);
        void SetValue(ref PROPERTYKEY key, ref PROPVARIANT pv);
        void Commit();
    }

    [ComImport, Guid("00021401-0000-0000-C000-000000000046")]
    private class ShellLinkCoClass { }

    // PKEY_AppUserModel_ID = {9F4C2855-9F79-4B39-A8D0-E1D42DE1D5F3}, 5
    private static readonly Guid FmtIdAppUserModel = new Guid("9F4C2855-9F79-4B39-A8D0-E1D42DE1D5F3");
    private const int PidAppUserModelId = 5;
    private const ushort VtLpwstr = 31;

    public static void Create(string lnkPath, string targetPath, string arguments,
                              string workingDirectory, string description,
                              string iconPath, string appUserModelId)
    {
        object o = new ShellLinkCoClass();
        try
        {
            IShellLinkW link = (IShellLinkW)o;
            link.SetPath(targetPath);
            if (!string.IsNullOrEmpty(arguments)) link.SetArguments(arguments);
            link.SetWorkingDirectory(workingDirectory);
            link.SetDescription(description);
            link.SetIconLocation(iconPath, 0);
            link.SetShowCmd(1); // SW_SHOWNORMAL

            IPropertyStore store = (IPropertyStore)o;
            PROPERTYKEY key = new PROPERTYKEY();
            key.fmtid = FmtIdAppUserModel;
            key.pid = PidAppUserModelId;

            PROPVARIANT pv = new PROPVARIANT();
            pv.vt = VtLpwstr;
            pv.p = Marshal.StringToCoTaskMemUni(appUserModelId);
            try
            {
                store.SetValue(ref key, ref pv);
            }
            finally
            {
                Marshal.FreeCoTaskMem(pv.p);
            }
            store.Commit();

            IPersistFile file = (IPersistFile)o;
            file.Save(lnkPath, true);
        }
        finally
        {
            if (Marshal.IsComObject(o)) Marshal.FinalReleaseComObject(o);
        }
    }
}
'@

Add-Type -TypeDefinition $shellLinkSource -Language CSharp

if (-not (Test-Path -LiteralPath $ShortcutDir)) {
    New-Item -ItemType Directory -Path $ShortcutDir -Force | Out-Null
}
# Remove first so the COM save cannot fail on a stale/read-only file.
if (Test-Path -LiteralPath $Shortcut) { Remove-Item -LiteralPath $Shortcut -Force }

[UberconvShellLink]::Create($Shortcut, $GuiExe, '', $InstallDir, $Summary, $GuiExe, $Aumid)
Write-Step "Shortcut    : $Shortcut"

# Read the AUMID back through the shell property system and report PASS/FAIL.
$readBack = $null
try {
    $shell = New-Object -ComObject Shell.Application
    $folder = $shell.Namespace((Split-Path -Parent $Shortcut))
    if ($folder) {
        $item = $folder.ParseName((Split-Path -Leaf $Shortcut))
        if ($item) { $readBack = [string]$item.ExtendedProperty('System.AppUserModel.ID') }
    }
} catch {
    $readBack = $null
} finally {
    if ($shell) { [void][System.Runtime.InteropServices.Marshal]::ReleaseComObject($shell) }
}

$aumidOk = ($readBack -eq $Aumid)
if ($aumidOk) {
    Write-Host "PASS  shortcut System.AppUserModel.ID = '$readBack'" -ForegroundColor Green
} else {
    Write-Host "FAIL  shortcut System.AppUserModel.ID read back as '$readBack' (expected '$Aumid')" -ForegroundColor Red
}

# --------------------------------------------------------------------------
# Registry helpers. The file-verb key lives under "Classes\*\shell", and "*"
# is a PowerShell wildcard, so provider cmdlets are unreliable there: talk to
# the registry API directly instead.
# --------------------------------------------------------------------------
function Set-RegValue {
    param(
        [Parameter(Mandatory)] [string] $SubKey,
        [Parameter(Mandatory)] [AllowEmptyString()] [string] $Name,
        [Parameter(Mandatory)] [AllowEmptyString()] [string] $Value,
        [Microsoft.Win32.RegistryValueKind] $Kind = [Microsoft.Win32.RegistryValueKind]::String
    )
    $key = [Microsoft.Win32.Registry]::CurrentUser.CreateSubKey($SubKey)
    try {
        # '' is the key's default value.
        $key.SetValue($Name, $Value, $Kind)
    } finally {
        $key.Close()
    }
}

# --------------------------------------------------------------------------
# 5. Register the AUMID so the taskbar groups/uses our icon + name
# --------------------------------------------------------------------------
Set-RegValue -SubKey "Software\Classes\AppUserModelId\$Aumid" -Name 'DisplayName' -Value $AppName
Set-RegValue -SubKey "Software\Classes\AppUserModelId\$Aumid" -Name 'IconUri'     -Value $GuiExe
Write-Step "Registered  : $RegAumid"

# --------------------------------------------------------------------------
# 6. Per-user context-menu verbs ("Convert with uberconv...")
#    NOTE: these only do something because uberconv-app.exe accepts file and
#    folder paths as argv[1..] and queues them at startup.
#    A shell verb can only substitute %1: "%*" is a cmd.exe construct Explorer
#    does not expand, so one command line cannot carry a whole selection.
#    MultiSelectModel='Player' is what keeps Explorer from silently dropping
#    the other files - it offers the verb for any selection size and invokes it
#    separately per item, so N files become N launches instead of one.
# --------------------------------------------------------------------------
$verbLabel = 'Convert with uberconv...'
$verbCmd   = '"' + $GuiExe + '" "%1"'

$verbKeys = @(
    "Software\Classes\*\shell\$AppName",
    "Software\Classes\Directory\shell\$AppName"
)
foreach ($verbKey in $verbKeys) {
    Set-RegValue -SubKey $verbKey          -Name ''     -Value $verbLabel
    Set-RegValue -SubKey $verbKey          -Name 'Icon' -Value ($GuiExe + ',0')
    # Player: offered for any selection, invoked once per selected item.
    Set-RegValue -SubKey $verbKey          -Name 'MultiSelectModel' -Value 'Player'
    Set-RegValue -SubKey "$verbKey\command" -Name ''    -Value $verbCmd
}
Write-Step "Context menu: 'Convert with uberconv...' for files and folders"

# --------------------------------------------------------------------------
# 7. Apps & features entry
# --------------------------------------------------------------------------
$sizeKb = 0
foreach ($n in @($GuiExeName, $CliExeName, 'ffmpeg.exe')) {
    $p = Join-Path $InstallDir $n
    if (Test-Path -LiteralPath $p -PathType Leaf) {
        $sizeKb += [int][math]::Ceiling((Get-Item -LiteralPath $p).Length / 1KB)
    }
}

# The uninstaller is copied next to the app so Add/Remove Programs keeps
# working even if the repo checkout goes away.
$uninstInInstallDir = Join-Path $InstallDir 'uninstall-windows.ps1'
$uninstSource = Join-Path $PSScriptRoot 'uninstall-windows.ps1'
if (Test-Path -LiteralPath $uninstSource -PathType Leaf) {
    Copy-Item -LiteralPath $uninstSource -Destination $uninstInInstallDir -Force
}
$uninstallCmd = 'powershell.exe -NoProfile -ExecutionPolicy Bypass -File "' + $uninstInInstallDir + '" -Quiet'

if (-not (Test-Path -LiteralPath $RegUninstall)) {
    New-Item -Path $RegUninstall -Force | Out-Null
}
$uninstallSubKey = "Software\Microsoft\Windows\CurrentVersion\Uninstall\$AppName"
Set-RegValue -SubKey $uninstallSubKey -Name 'DisplayName'     -Value $AppName
Set-RegValue -SubKey $uninstallSubKey -Name 'DisplayIcon'     -Value ($GuiExe + ',0')
Set-RegValue -SubKey $uninstallSubKey -Name 'Publisher'       -Value $Publisher
Set-RegValue -SubKey $uninstallSubKey -Name 'DisplayVersion'  -Value $version
Set-RegValue -SubKey $uninstallSubKey -Name 'InstallLocation' -Value $InstallDir
Set-RegValue -SubKey $uninstallSubKey -Name 'EstimatedSize'   -Value ([int]$sizeKb) -Kind DWord
Set-RegValue -SubKey $uninstallSubKey -Name 'NoModify'        -Value 1 -Kind DWord
Set-RegValue -SubKey $uninstallSubKey -Name 'NoRepair'        -Value 1 -Kind DWord
Set-RegValue -SubKey $uninstallSubKey -Name 'UninstallString' -Value $uninstallCmd
Write-Step "Uninstall   : $RegUninstall ($sizeKb KB)"

# --------------------------------------------------------------------------
# 8. User PATH (idempotent)
# --------------------------------------------------------------------------
function Test-PathHasEntry([string] $PathValue, [string] $Entry) {
    if ([string]::IsNullOrWhiteSpace($PathValue)) { return $false }
    $wanted = $Entry.TrimEnd('\').ToLowerInvariant()
    foreach ($part in $PathValue.Split(';')) {
        if ($part.Trim().TrimEnd('\').ToLowerInvariant() -eq $wanted) { return $true }
    }
    return $false
}

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
        # Cosmetic only: new shells pick the change up on their own.
    }
}

$pathEntry = $InstallDir.TrimEnd('\')

if ($NoPath) {
    Write-Step "PATH        : skipped (-NoPath)"
} else {
    $envKey = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey('Environment', $true)
    if (-not $envKey) {
        $envKey = [Microsoft.Win32.Registry]::CurrentUser.CreateSubKey('Environment')
    }
    try {
        $current = $envKey.GetValue('Path', '', [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
        $kind = [Microsoft.Win32.RegistryValueKind]::ExpandString
        if (@($envKey.GetValueNames()) -contains 'Path') { $kind = $envKey.GetValueKind('Path') }
        if ($kind -ne [Microsoft.Win32.RegistryValueKind]::ExpandString) {
            $kind = [Microsoft.Win32.RegistryValueKind]::String
        }

        if (Test-PathHasEntry $current $pathEntry) {
            Write-Step "PATH        : already present (left untouched)"
        } else {
            if ([string]::IsNullOrWhiteSpace($current)) {
                $newPath = $pathEntry
            } else {
                $newPath = $current.TrimEnd(';') + ';' + $pathEntry
            }
            $envKey.SetValue('Path', $newPath, $kind)
            Send-EnvironmentChanged
            Write-Step "PATH        : added $pathEntry (user scope)"
        }
    } finally {
        $envKey.Close()
    }
}

# --------------------------------------------------------------------------
# 9. Summary
# --------------------------------------------------------------------------
Write-Host ''
Write-Host "uberconv $version installed for $env:USERNAME" -ForegroundColor Cyan
Write-Host "  app      : $GuiExe"
Write-Host "  cli      : $CliExe"
Write-Host "  shortcut : $Shortcut"
Write-Host "  aumid    : $Aumid (shortcut + HKCU\Software\Classes\AppUserModelId)"
Write-Host "  verbs    : 'Convert with uberconv...' on files and folders"
Write-Host "  uninstall: $RegUninstall"
Write-Host ''
Write-Host 'next: launch uberconv from the Start menu'
Write-Host ''

if (-not $aumidOk) { exit 1 }
