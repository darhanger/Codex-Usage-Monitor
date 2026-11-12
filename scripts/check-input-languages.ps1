$ErrorActionPreference = 'Stop'

Add-Type @'
using System;
using System.Runtime.InteropServices;

public static class CodexMonitorInputSnapshot
{
    [DllImport("user32.dll")]
    public static extern int GetKeyboardLayoutList(int count, IntPtr[] layouts);

    [DllImport("user32.dll")]
    public static extern IntPtr GetKeyboardLayout(uint threadId);

    [DllImport("user32.dll")]
    public static extern IntPtr GetForegroundWindow();

    [DllImport("user32.dll")]
    public static extern uint GetWindowThreadProcessId(IntPtr window, IntPtr processId);
}
'@

Write-Output "Captured: $((Get-Date).ToString('o'))"
Write-Output 'Configured user languages:'
foreach ($language in (Get-WinUserLanguageList)) {
    Write-Output "  $($language.LanguageTag): $($language.InputMethodTips -join ', ')"
}

$count = [CodexMonitorInputSnapshot]::GetKeyboardLayoutList(0, $null)
$layouts = New-Object IntPtr[] $count
$returned = [CodexMonitorInputSnapshot]::GetKeyboardLayoutList($count, $layouts)
Write-Output "Loaded input locales ($returned):"
$layouts | Select-Object -First $returned | ForEach-Object {
    Write-Output "  0x$($_.ToInt64().ToString('X16'))"
}

$foreground = [CodexMonitorInputSnapshot]::GetForegroundWindow()
$threadId = [CodexMonitorInputSnapshot]::GetWindowThreadProcessId($foreground, [IntPtr]::Zero)
$active = [CodexMonitorInputSnapshot]::GetKeyboardLayout($threadId)
Write-Output "Foreground HWND: 0x$($foreground.ToInt64().ToString('X'))"
Write-Output "Foreground thread: $threadId"
Write-Output "Foreground input locale: 0x$($active.ToInt64().ToString('X16'))"
