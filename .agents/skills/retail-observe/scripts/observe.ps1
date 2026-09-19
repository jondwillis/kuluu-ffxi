<#
observe.ps1 - drive and observe a retail FFXI client running natively on
Windows. Same verbs and the same lib/keys.tsv as observe.sh on macOS/Linux, so
a drive recipe or an observation record written on one host reads on all of
them. Run it from PowerShell:

  .\observe.ps1 doctor              check prerequisites and what is missing
  .\observe.ps1 targets             every top-level window (find yours here)
  .\observe.ps1 status              which window resolves as the client
  .\observe.ps1 window              JSON of the matched window
  .\observe.ps1 show                bring it forward (input needs this)
  .\observe.ps1 capture [out.png]   screenshot the client window
  .\observe.ps1 ocr                 capture + OCR: TEXT<TAB>x<TAB>y
  .\observe.ps1 keys                logical key names and their in-game meaning
  .\observe.ps1 key <name> [secs]   press or hold a key
  .\observe.ps1 type <text>         type a string
  .\observe.ps1 click <x> <y> [right|double]   window-relative coordinates
  .\observe.ps1 move <x> <y>
  .\observe.ps1 click-text <regex> [right|double]
  .\observe.ps1 launch [-Server HOST]

Configuration mirrors observe.sh: FFXI_OBSERVE_WINDOW_TITLE (default
"FINAL FANTASY"), FFXI_OBSERVE_INSTALL, FFXI_OBSERVE_RUNNER (empty on native
Windows), FFXI_OBSERVE_LOADER, FFXI_OBSERVE_LOADER_ARGS, FFXI_OBSERVE_SERVER,
FFXI_OBSERVE_ARTIFACTS, FFXI_OBSERVE_PROFILE (%APPDATA%\ffxi-observe\NAME.conf).

Verification status: written against documented Win32 behavior, not yet
exercised against a running client. Run `doctor` first and fix what it reports.

Native Windows has no VM layer and no Wine translation, which removes most of
the failure modes the other hosts carry - but it adds one: UAC. An elevated
client cannot be driven by a non-elevated script at all (Windows blocks input
across integrity levels), and clicking through a UAC prompt is a human's
decision, never this script's.
#>

[CmdletBinding()]
param(
  [Parameter(Position = 0)][string]$Command,
  [Parameter(Position = 1, ValueFromRemainingArguments = $true)][string[]]$Rest,
  [string]$Server
)

$ErrorActionPreference = 'Stop'
$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$KeysTsv = Join-Path $ScriptDir 'lib\keys.tsv'

Add-Type -AssemblyName System.Drawing, System.Windows.Forms
Add-Type @'
using System;
using System.Text;
using System.Runtime.InteropServices;
public struct RECT { public int Left, Top, Right, Bottom; }
public static class Win {
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int cmd);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
  [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
  [DllImport("user32.dll")] public static extern uint MapVirtualKey(uint code, uint mapType);
  [DllImport("user32.dll")] public static extern void keybd_event(byte vk, byte scan, uint flags, IntPtr extra);
  [DllImport("user32.dll")] public static extern void mouse_event(uint flags, uint dx, uint dy, uint data, IntPtr extra);
}
'@

# Windows virtualizes coordinates for a non-DPI-aware process, which silently
# shifts every click on a scaled display. Declaring awareness up front means the
# rectangles this script reads are the ones the user sees.
[void][Win]::SetProcessDPIAware()

$KEYEVENTF_KEYUP = 0x2
$KEYEVENTF_SCANCODE = 0x8
$MOUSEEVENTF = @{ LeftDown = 0x2; LeftUp = 0x4; RightDown = 0x8; RightUp = 0x10 }

function Die($msg) { Write-Error "observe: $msg"; exit 1 }

function Get-Conf {
  $conf = @{
    window_title = 'FINAL FANTASY'; install = ''; runner = ''
    loader = ''; loader_args = ''; server = ''
    artifacts = 'artifacts\retail'
  }
  $name = $env:FFXI_OBSERVE_PROFILE
  if ($name) {
    $path = Join-Path $env:APPDATA "ffxi-observe\$name.conf"
    if (-not (Test-Path $path)) { Die "profile '$name' has no config at $path" }
    foreach ($line in Get-Content $path) {
      if ($line -match '^\s*#' -or $line -notmatch '=') { continue }
      $k, $v = $line -split '=', 2
      $conf[$k.Trim()] = $v.Trim()
    }
    $script:ProfileName = $name
  }
  foreach ($k in @($conf.Keys)) {
    $env_val = [Environment]::GetEnvironmentVariable("FFXI_OBSERVE_" + $k.ToUpper())
    if ($env_val) { $conf[$k] = $env_val }
  }
  if ($Server) { $conf.server = $Server }
  return $conf
}

function Get-KeyCode($name) {
  foreach ($line in Get-Content $KeysTsv) {
    if ($line -match '^#' -or $line -notmatch "`t") { continue }
    $f = $line -split "`t"
    if ($f[0] -eq $name) { return [Convert]::ToInt32($f[3], 16) }
  }
  if ($name -match '^0x[0-9a-fA-F]+$') { return [Convert]::ToInt32($name, 16) }
  Die "unknown key '$name' - run: .\observe.ps1 keys"
}

function Show-Keys {
  '{0,-17} {1,-14} {2}' -f 'logical key', 'win32 VK', 'in-game meaning'
  foreach ($line in Get-Content $KeysTsv) {
    if ($line -match '^#' -or $line -notmatch "`t") { continue }
    $f = $line -split "`t"
    '{0,-17} {1,-14} {2}' -f $f[0], $f[3], $f[4]
  }
}

function Get-Windows {
  Get-Process | Where-Object { $_.MainWindowHandle -ne 0 -and $_.MainWindowTitle } | ForEach-Object {
    $r = New-Object RECT
    [void][Win]::GetWindowRect($_.MainWindowHandle, [ref]$r)
    [pscustomobject]@{
      id = [int64]$_.MainWindowHandle; name = $_.MainWindowTitle; owner = $_.ProcessName
      pid = $_.Id; x = $r.Left; y = $r.Top; w = $r.Right - $r.Left; h = $r.Bottom - $r.Top
    }
  } | Where-Object { $_.w -gt 200 -and $_.h -gt 200 }
}

function Resolve-Window($conf) {
  $hit = Get-Windows | Where-Object { $_.name -match $conf.window_title } | Select-Object -First 1
  if (-not $hit) {
    Die @"
no window matching title /$($conf.window_title)/.
  - the client is not running        -> .\observe.ps1 launch
  - a launcher window, not the game  -> set FFXI_OBSERVE_WINDOW_TITLE
  - the client runs elevated          -> Windows blocks input across integrity
    levels; run this PowerShell with the same elevation, or start the client
    unelevated
Run  .\observe.ps1 targets  to see every window.
"@
  }
  return $hit
}

# Focus, then act, in the same call: the client ignores input aimed at a window
# that is not foreground, and nothing about a previous call's focus survives.
function Set-Foreground($win) {
  [void][Win]::ShowWindow([IntPtr]$win.id, 9)   # SW_RESTORE
  [void][Win]::SetForegroundWindow([IntPtr]$win.id)
  Start-Sleep -Milliseconds 300
}

function Save-Capture($win, $out) {
  if (-not $out) {
    $dir = (Get-Conf).artifacts
    New-Item -ItemType Directory -Force -Path $dir | Out-Null
    $out = Join-Path $dir ((Get-Date -Format 'yyyyMMdd-HHmmss') + '.png')
  }
  New-Item -ItemType Directory -Force -Path (Split-Path -Parent $out) | Out-Null
  # CopyFromScreen reads what is actually composited, which is what a DirectX
  # client shows; it therefore needs the window unobstructed and not minimized.
  $bmp = New-Object System.Drawing.Bitmap($win.w, $win.h)
  $g = [System.Drawing.Graphics]::FromImage($bmp)
  $g.CopyFromScreen($win.x, $win.y, 0, 0, (New-Object System.Drawing.Size($win.w, $win.h)))
  $bmp.Save($out, [System.Drawing.Imaging.ImageFormat]::Png)
  $g.Dispose(); $bmp.Dispose()
  return $out
}

function Invoke-Ocr($img) {
  if (-not (Get-Command tesseract -ErrorAction SilentlyContinue)) {
    Die "tesseract not installed (needed for ocr/click-text). Install it, or read the capture yourself."
  }
  $rows = & tesseract $img stdout tsv 2>$null | Select-Object -Skip 1
  $lines = @{}
  foreach ($row in $rows) {
    $f = $row -split "`t"
    if ($f.Count -lt 12 -or $f[0] -ne '5' -or -not $f[11]) { continue }
    $key = "$($f[2])-$($f[3])-$($f[4])-$($f[5])"
    $l = [int]$f[6]; $t = [int]$f[7]; $r = $l + [int]$f[8]; $b = $t + [int]$f[9]
    if ($lines.ContainsKey($key)) {
      $e = $lines[$key]
      $e.text += " $($f[11])"
      $e.l = [Math]::Min($e.l, $l); $e.t = [Math]::Min($e.t, $t)
      $e.r = [Math]::Max($e.r, $r); $e.b = [Math]::Max($e.b, $b)
    } else {
      $lines[$key] = [pscustomobject]@{ text = $f[11]; l = $l; t = $t; r = $r; b = $b }
    }
  }
  $lines.Values | ForEach-Object {
    "{0}`t{1}`t{2}" -f $_.text, [int](($_.l + $_.r) / 2), [int](($_.t + $_.b) / 2)
  }
}

# Elevation prompts are the human's to answer. Refusing at the click layer
# means no drive loop can talk itself into clicking one.
$ConsentRe = 'User Account Control|make changes to your device|Verified publisher'

function Send-Key($vk, $seconds) {
  # DirectInput clients read scan codes rather than virtual keys, so inject the
  # scan code: a VK-only press is the usual reason a game "ignores" automation.
  $scan = [byte][Win]::MapVirtualKey([uint32]$vk, 0)
  [Win]::keybd_event([byte]$vk, $scan, $KEYEVENTF_SCANCODE, [IntPtr]::Zero)
  Start-Sleep -Seconds $seconds
  [Win]::keybd_event([byte]$vk, $scan, $KEYEVENTF_SCANCODE -bor $KEYEVENTF_KEYUP, [IntPtr]::Zero)
}

function Send-Click($gx, $gy, $kind, $press) {
  [void][Win]::SetCursorPos($gx, $gy)
  if ($press -ne 'click') { return }
  Start-Sleep -Milliseconds 50
  $down = if ($kind -eq 'right') { $MOUSEEVENTF.RightDown } else { $MOUSEEVENTF.LeftDown }
  $up = if ($kind -eq 'right') { $MOUSEEVENTF.RightUp } else { $MOUSEEVENTF.LeftUp }
  [Win]::mouse_event($down, 0, 0, 0, [IntPtr]::Zero)
  Start-Sleep -Milliseconds 50
  [Win]::mouse_event($up, 0, 0, 0, [IntPtr]::Zero)
  if ($kind -eq 'double') {
    Start-Sleep -Milliseconds 80
    [Win]::mouse_event($down, 0, 0, 0, [IntPtr]::Zero)
    [Win]::mouse_event($up, 0, 0, 0, [IntPtr]::Zero)
  }
}

$conf = Get-Conf

switch ($Command) {

  'doctor' {
    "profile: $(if ($ProfileName) { $ProfileName } else { 'none (zero-config: matching window title only)' })"
    "window match: title /$($conf.window_title)/"
    "host: Windows $([Environment]::OSVersion.Version)"
    "  [ok]   .NET drawing + forms available"
    $elevated = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
    "  [info] this PowerShell is $(if ($elevated) { 'elevated' } else { 'not elevated' }); input only crosses to a client at the same integrity level"
    if (Get-Command tesseract -ErrorAction SilentlyContinue) { "  [ok]   tesseract" }
    else { "  [warn] no tesseract -> ocr/click-text unavailable; you can still read captures yourself" }
    $win = Get-Windows | Where-Object { $_.name -match $conf.window_title } | Select-Object -First 1
    if ($win) { "  [ok]   client window found: $($win | ConvertTo-Json -Compress)" }
    else { "  [warn] no client window matches right now (fine if it is not running)" }
  }

  'targets' { Get-Windows | ForEach-Object { "{0}`t{1}`t{2}x{3}`t{4}" -f $_.id, $_.owner, $_.w, $_.h, $_.name } }

  'window' { Resolve-Window $conf | ConvertTo-Json }

  'status' {
    $win = Get-Windows | Where-Object { $_.name -match $conf.window_title } | Select-Object -First 1
    if ($win) { "client window: $($win | ConvertTo-Json -Compress)" }
    else { "client window: none matching title /$($conf.window_title)/" }
  }

  'show' { $win = Resolve-Window $conf; Set-Foreground $win; $win | ConvertTo-Json -Compress }

  'capture' {
    $win = Resolve-Window $conf
    Set-Foreground $win
    $out = Save-Capture $win $Rest[0]
    "$out  window:$($win.w)x$($win.h)  image:$($win.w)px-wide  scale:1x (DPI-aware: window units are pixels)"
  }

  'keys' { Show-Keys }

  { $_ -in 'ocr', 'click-text' } {
    $win = Resolve-Window $conf
    Set-Foreground $win
    $tmp = Join-Path $env:TEMP ("observe-ocr-" + [guid]::NewGuid().ToString('N').Substring(0, 8) + ".png")
    [void](Save-Capture $win $tmp)
    $ocr = Invoke-Ocr $tmp
    if ($Command -eq 'ocr') { Remove-Item $tmp -Force; $ocr; break }
    $pat = $Rest[0]; if (-not $pat) { Die "usage: .\observe.ps1 click-text <regex> [right|double]" }
    if ($ocr -match $ConsentRe) { Die "refusing to click: an elevation/consent dialog is on screen and that consent is human-only (capture kept: $tmp)" }
    $hit = $ocr | Where-Object { $_ -match $pat } | Select-Object -First 1
    if (-not $hit) { Die "no OCR text matching /$pat/ (capture kept: $tmp; run ocr to see what is on screen)" }
    Remove-Item $tmp -Force
    $f = $hit -split "`t"
    "click-text: `"$($f[0])`" at $($f[1]),$($f[2]) (window units)"
    Send-Click ($win.x + [int]$f[1]) ($win.y + [int]$f[2]) $(if ($Rest[1]) { $Rest[1] } else { 'left' }) 'click'
  }

  'key' {
    $name = $Rest[0]; if (-not $name) { Die "usage: .\observe.ps1 key <name> [hold-seconds]" }
    $dur = if ($Rest[1]) { [double]$Rest[1] } else { 0.05 }
    $win = Resolve-Window $conf
    Set-Foreground $win
    Send-Key (Get-KeyCode $name) $dur
  }

  'type' {
    $text = $Rest[0]; if (-not $text) { Die "usage: .\observe.ps1 type <text>" }
    $win = Resolve-Window $conf
    Set-Foreground $win
    # SendKeys treats + ^ % ~ ( ) { } [ ] as syntax, so a literal one must be
    # braced. A slash is safe here, unlike the AppleScript path on macOS.
    [System.Windows.Forms.SendKeys]::SendWait(($text -replace '([+^%~(){}\[\]])', '{$1}'))
  }

  { $_ -in 'click', 'move' } {
    if ($Rest.Count -lt 2) { Die "usage: .\observe.ps1 $Command <x> <y> [right|double]" }
    $win = Resolve-Window $conf
    Set-Foreground $win
    Send-Click ($win.x + [int]$Rest[0]) ($win.y + [int]$Rest[1]) $(if ($Rest[2]) { $Rest[2] } else { 'left' }) $Command
  }

  'launch' {
    $install = $conf.install
    if ($install -like 'kuluu:*') { $install = (& kuluu install path $install.Substring(6)).Trim() }
    if (-not $install) { Die "launch needs an install: set FFXI_OBSERVE_INSTALL" }
    if (-not $conf.loader) { Die "launch needs a loader: set FFXI_OBSERVE_LOADER (e.g. _bootloader\xiloader.exe)" }
    $loader = Join-Path $install $conf.loader
    if (-not (Test-Path $loader)) { Die "loader not found: $loader" }
    $largs = $conf.loader_args
    if ($conf.server) { $largs = "$largs --server $($conf.server)" }
    "observe: $loader $largs"
    # Loaders resolve DATs, config and their own DLLs against the working
    # directory rather than argv[0].
    Start-Process -FilePath $loader -ArgumentList $largs -WorkingDirectory (Split-Path -Parent $loader)
  }

  default {
    Get-Content $PSCommandPath | Select-Object -Skip 1 -First 36 | ForEach-Object { $_ -replace '^#>', '' }
    exit 1
  }
}
