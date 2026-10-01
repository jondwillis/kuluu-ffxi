# Capture a hidden/occluded kuluu window to PNG via
# PrintWindow(PW_RENDERFULLCONTENT). The window never needs to be on screen:
# KULUU_WINDOW_HIDDEN=1 runs render into the buried surface and this reads it back.
# Usage: powershell -NoProfile -ExecutionPolicy Bypass -File scripts/cap-window.ps1 <process-name> <out.png> [wait-ms]
param(
    [Parameter(Mandatory=$true)][string]$Proc,
    [Parameter(Mandatory=$true)][string]$Out,
    [int]$WaitMs = 600
)
Add-Type @'
using System;
using System.Runtime.InteropServices;
public class W3 {
    [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr h, IntPtr after, int x, int y, int cx, int cy, uint flags);
    [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int cmdShow);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out Rect r);
    [StructLayout(LayoutKind.Sequential)] public struct Rect { public int l, t, r, b; }
    [DllImport("user32.dll")] public static extern bool GetClientRect(IntPtr h, out Rect r);
    [StructLayout(LayoutKind.Sequential)] public struct Pt { public int x, y; }
    [DllImport("user32.dll")] public static extern bool ClientToScreen(IntPtr h, ref Pt p);
    [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr h, IntPtr hdc, uint flags);
}
'@
Add-Type -AssemblyName System.Windows.Forms,System.Drawing
# Get-Process takes the bare image name; "kuluu.exe" is a wildcard pattern that matches nothing.
if ($Proc.EndsWith('.exe')) { $Proc = $Proc.Substring(0, $Proc.Length - 4) }
$w = Get-Process $Proc -ErrorAction SilentlyContinue | Where-Object { $_.MainWindowHandle -ne 0 } | Select-Object -First 1
if (-not $w) { Write-Output "no window for process '$Proc'"; exit 1 }
$hwnd = $w.MainWindowHandle
# Read the real rect: .NET's MainWindowBounds reports 0x0 for KULUU_WINDOW_HIDDEN windows
# even after they are sized, so GetWindowRect is the source of truth.
$rect = New-Object W3+Rect
[W3]::GetWindowRect($hwnd, [ref]$rect) | Out-Null
$cw = $rect.r - $rect.l; $ch = $rect.b - $rect.t
Write-Output "bounds=${cw}x${ch}"
if ($cw -le 0 -or $ch -le 0) {
    # Zero-sized (hidden until shown): place offscreen at the capture size —
    # SWP_NOZORDER|SWP_NOACTIVATE, no focus steal, never on screen — then show without
    # activation so winit sizes the swapchain; at -32000 it stays invisible.
    [W3]::SetWindowPos($hwnd, [IntPtr](0), -32000, -32000, 1280, 800, 0x14) | Out-Null
    [W3]::ShowWindow($hwnd, 4) | Out-Null   # SW_SHOWNOACTIVATE
    Start-Sleep -Milliseconds ($WaitMs + 600)
    $rect = New-Object W3+Rect
    [W3]::GetWindowRect($hwnd, [ref]$rect) | Out-Null
    $cw = $rect.r - $rect.l; $ch = $rect.b - $rect.t
    Write-Output "bounds=${cw}x${ch} (offscreen)"
    if ($cw -le 0 -or $ch -le 0) { Write-Output 'zero-size window'; exit 1 }
}
# PrintWindow draws the whole window (title bar + borders included); capture it all, then
# crop the non-client frame out of the bitmap.
$crect = New-Object W3+Rect
[W3]::GetClientRect($hwnd, [ref]$crect) | Out-Null
$cw2 = $crect.r - $crect.l; $ch2 = $crect.b - $crect.t
if ($cw2 -le 0 -or $ch2 -le 0) { Write-Output 'zero-size client'; exit 1 }
$origin = New-Object W3+Pt
[W3]::ClientToScreen($hwnd, [ref]$origin) | Out-Null
$bmp = New-Object System.Drawing.Bitmap($cw, $ch)
$g = [System.Drawing.Graphics]::FromImage($bmp)
$hdc = $g.GetHdc()
# PW_RENDERFULLCONTENT (2): renders the full window content even when hidden/occluded.
[W3]::PrintWindow($w.MainWindowHandle, $hdc, 2) | Out-Null
$g.ReleaseHdc($hdc)
$crop = New-Object System.Drawing.Rectangle(($origin.x - $rect.l), ($origin.y - $rect.t), $cw2, $ch2)
$bmp.Clone($crop, $bmp.PixelFormat).Save($Out)
Write-Output "captured pid $($w.Id) title '$($w.MainWindowTitle)' -> $Out (${cw2}x${ch2})"
