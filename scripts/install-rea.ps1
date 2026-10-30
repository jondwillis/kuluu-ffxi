# SUPERSEDED 2026-10-24: see vendor/rea/INSTALL.md
#requires -Version 5.1
<#
  Installs REA, the pinned Ghidra reader for agents, into vendor\rea (git-ignored).
  Pins live in vendor/README.md and here; every download is SHA-256 checked.
  Safe to re-run: finished parts are skipped. Ends with INSTALL PASS or FAIL.

    Ghidra   12.1.4 (ghidra_12.1.4_PUBLIC_20260921.zip)
    JDK      Temurin 21.0.12.1+1 (Windows x64 zip)
    Node     24.21.0 (Windows x64 zip, used only by REA)
    REA      rea-agents 6.4.0 (npm, --ignore-scripts)

  Writes only under vendor\rea. No PATH, registry, system Java/Node or Claude Code config changes.
  Usage (from the checkout root):
    powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts\install-rea.ps1 [-Target <FFXiMain.unpacked.dll>]
#>
param([string]$Target = "")

$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

$Repo   = Split-Path -Parent (Split-Path -Parent $MyInvocation.MyCommand.Path)
$Rea    = Join-Path $Repo 'vendor\rea'
$Dl     = Join-Path $Rea '_downloads'
$Ghidra = Join-Path $Rea 'ghidra_12.1.4_PUBLIC'
$Jdk    = Join-Path $Rea 'jdk-21'
$Node   = Join-Path $Rea 'node'

function Step($m) { Write-Host "== $m" }
function Fail($m) { Write-Host "FAIL: $m"; exit 1 }
function Sha256($p) { (Get-FileHash -Algorithm SHA256 -LiteralPath $p).Hash.ToLower() }

function Fetch($url, $file, $sha) {
  $out = Join-Path $Dl $file
  if (-not (Test-Path -LiteralPath $out) -or (Sha256 $out) -ne $sha) {
    Write-Host "   downloading $file"
    & curl.exe -fL --retry 3 --silent --show-error -o $out $url
    if ($LASTEXITCODE -ne 0) { Fail "download failed: $url" }
  }
  $got = Sha256 $out
  if ($got -ne $sha) { Fail "sha256 mismatch for $file`n   expected $sha`n   got      $got" }
  Write-Host "   sha256 ok  $file"
  return $out
}

function Unzip($zip, $inner, $dest) {
  $staged = Join-Path $Dl $inner
  if (-not (Test-Path -LiteralPath $staged)) {
    & $SystemTar -xf $zip -C $Dl
    if ($LASTEXITCODE -ne 0) { Fail "extract failed: $zip" }
  }
  if (-not (Test-Path -LiteralPath $staged)) { Fail "expected folder $inner inside $zip" }
  Move-Item -LiteralPath $staged -Destination $dest
}

function PeIdentity($p) {
  $b = [IO.File]::ReadAllBytes($p)
  $pe = [BitConverter]::ToInt32($b, 0x3C)
  return [pscustomobject]@{
    Machine = [BitConverter]::ToUInt16($b, $pe + 4)
    Tds     = [BitConverter]::ToUInt32($b, $pe + 8)
    Base    = [BitConverter]::ToUInt32($b, $pe + 24 + 28)
  }
}

function WriteUtf8($path, $text) { [IO.File]::WriteAllText($path, $text, (New-Object Text.UTF8Encoding $false)) }

Step 'checkout'
if (-not (Test-Path -LiteralPath (Join-Path $Repo 'Cargo.toml')) -or -not (Test-Path -LiteralPath (Join-Path $Repo 'vendor\README.md'))) {
  Fail "run this from a kuluu checkout (scripts\install-rea.ps1); $Repo has no Cargo.toml or vendor\README.md"
}
Write-Host "   $Repo"

Step 'box'
Write-Host ("   host {0}  user {1}  {2}  64-bit OS: {3}" -f $env:COMPUTERNAME, $env:USERNAME, [Environment]::OSVersion.VersionString, [Environment]::Is64BitOperatingSystem)
if (-not [Environment]::Is64BitOperatingSystem) { Fail 'REA needs 64-bit Windows 10 or newer' }
if ([Environment]::OSVersion.Version.Major -lt 10) { Fail 'REA needs Windows 10 or newer' }
if (-not (Get-Command curl.exe -ErrorAction SilentlyContinue)) { Fail 'curl.exe not found (ships with Windows 10 1803+)' }
# Pin Windows' tar. Launched under a Git Bash-inherited PATH, a bare `tar` resolves to /usr/bin/tar,
# which reads C:\... as remote host:path and fails extraction (found on GEOALIEN, 2026-10-14).
$SystemTar = Join-Path $env:SystemRoot 'System32\tar.exe'
if (-not (Test-Path -LiteralPath $SystemTar)) { Fail "$SystemTar not found (ships with Windows 10 1803+)" }
$drive = (Get-Item -LiteralPath $Repo).PSDrive.Name
$freeGb = [math]::Round((Get-PSDrive -Name $drive).Free / 1GB, 1)
Write-Host "   free on ${drive}: $freeGb GB"
if (-not (Test-Path -LiteralPath "$Ghidra\support\analyzeHeadless.bat") -and $freeGb -lt 4) { Fail "need 4 GB free on ${drive}:" }
New-Item -ItemType Directory -Force -Path $Rea, $Dl | Out-Null

Step 'Ghidra 12.1.4'
if (-not (Test-Path -LiteralPath "$Ghidra\support\analyzeHeadless.bat")) {
  $z = Fetch 'https://github.com/NationalSecurityAgency/ghidra/releases/download/Ghidra_12.1.4_build/ghidra_12.1.4_PUBLIC_20260921.zip' `
             'ghidra_12.1.4_PUBLIC_20260921.zip' 'ddac49f903da9d5bac833e5cc79395098b9c33cfd3279be5f31bd00387d2d4db'
  Unzip $z 'ghidra_12.1.4_PUBLIC' $Ghidra
}
Write-Host "   $Ghidra"

Step 'JDK 21 (Temurin 21.0.12.1+1)'
if (-not (Test-Path -LiteralPath "$Jdk\bin\javac.exe")) {
  $z = Fetch 'https://github.com/adoptium/temurin21-binaries/releases/download/jdk-21.0.12.1%2B1/OpenJDK21U-jdk_x64_windows_hotspot_21.0.12.1_1.zip' `
             'OpenJDK21U-jdk_x64_windows_hotspot_21.0.12.1_1.zip' 'f9d6e191ab098c0d416e7d588a24420a8621cd2f4720dab2459b8b7b2d2d8b4e'
  Unzip $z 'jdk-21.0.12.1+1' $Jdk
}
Write-Host "   $Jdk"

Step 'Node 24.21.0 (for REA only)'
if (-not (Test-Path -LiteralPath "$Node\node.exe")) {
  $z = Fetch 'https://nodejs.org/dist/v24.21.0/node-v24.21.0-win-x64.zip' `
             'node-v24.21.0-win-x64.zip' '158f7685b44de51f6c0df1d153526cbcd3e1bc739a8dfc607721cef75de9e541'
  Unzip $z 'node-v24.21.0-win-x64' $Node
}
$nodeVer = (& "$Node\node.exe" --version)
if ($nodeVer -ne 'v24.21.0') { Fail "vendored node reports $nodeVer" }
Write-Host "   $Node  $nodeVer"

Step 'REA 6.4.0'
$pkg = "$Rea\node_modules\rea-agents\package.json"
if (-not (Test-Path -LiteralPath $pkg)) {
  $env:Path = "$Node;$env:Path"
  & "$Node\npm.cmd" install --prefix $Rea --ignore-scripts --no-audit --no-fund --save-exact 'rea-agents@6.4.0'
  if ($LASTEXITCODE -ne 0) { Fail 'npm install rea-agents@6.4.0 failed' }
}
$reaVer = (Get-Content -LiteralPath $pkg -Raw | ConvertFrom-Json).version
if ($reaVer -ne '6.4.0') { Fail "REA is $reaVer, vendor/README.md pins 6.4.0" }
Write-Host "   $Rea\node_modules\rea-agents  $reaVer"

Step 'target DLL'
$TargetDir = Join-Path $Rea 'targets'
$dst = Join-Path $TargetDir 'FFXiMain.unpacked.dll'
New-Item -ItemType Directory -Force -Path $TargetDir | Out-Null
if (-not (Test-Path -LiteralPath $dst)) {
  $cands = @()
  if ($Target) { $cands += $Target }
  $cands += (Join-Path $Repo 'artifacts\FFXiMain.unpacked.dll'),
            'C:\tmp\ffximain_work\FFXiMain.unpacked.dll',
            (Join-Path $Repo 'research\cow_ffxi_disassembly\artifacts\FFXiMain.unpacked.dll'),
            'C:\cow_ffxi_disassembly\artifacts\FFXiMain.unpacked.dll'
  $src = $cands | Where-Object { Test-Path -LiteralPath $_ } | Select-Object -First 1
  if (-not $src) {
    foreach ($root in (Join-Path $Repo 'research\cow_ffxi_disassembly'), 'C:\cow_ffxi_disassembly') {
      if (-not $src -and (Test-Path -LiteralPath $root)) {
        $src = Get-ChildItem -LiteralPath $root -Recurse -Filter 'FFXiMain.unpacked.dll' -ErrorAction SilentlyContinue |
               Select-Object -First 1 -ExpandProperty FullName
      }
    }
  }
  if (-not $src) { Fail 'FFXiMain.unpacked.dll not found. Re-run with -Target <path to it>' }
  Copy-Item -LiteralPath $src -Destination $dst
  Write-Host "   copied from $src"
}
$id = PeIdentity $dst
Write-Host ("   machine 0x{0:X}  TimeDateStamp 0x{1:X8}  ImageBase 0x{2:X8}" -f $id.Machine, $id.Tds, $id.Base)
if ($id.Machine -ne 0x14C -or $id.Tds -ne 0x6A995428 -or $id.Base -ne 0x10000000) {
  Fail 'target is not the retail-2026-09 unpacked PE (x86, TDS 0x6A995428, base 0x10000000)'
}
$targetSha = Sha256 $dst
Write-Host "   $dst"
Write-Host "   sha256 $targetSha"

Step 'rea.mcp.json'
$cfg = [ordered]@{
  mcpServers = [ordered]@{
    rea = [ordered]@{
      type    = 'stdio'
      command = "$Node\node.exe"
      args    = @("$Rea\node_modules\rea-agents\scripts\rea.mjs", 'mcp')
      env     = [ordered]@{
        GHIDRA_INSTALL_DIR            = $Ghidra
        JAVA_HOME                     = $Jdk
        REA_ANALYSIS_PROVIDER         = 'ghidra'
        REA_GHIDRA_STARTUP_TIMEOUT_MS = '3600000'
        REA_MCP_INPUT_SCHEMA_PROFILE  = 'compact'
      }
      timeout = 3900000
    }
  }
}
WriteUtf8 (Join-Path $Rea 'rea.mcp.json') (($cfg | ConvertTo-Json -Depth 6) + "`n")
Write-Host "   $Rea\rea.mcp.json"

Step 'python'
$pyExe = $null; $pyPre = @()
if (Get-Command py -ErrorAction SilentlyContinue) { $pyExe = 'py'; $pyPre = @('-3') }
elseif (Get-Command python -ErrorAction SilentlyContinue) { $pyExe = 'python' }
if (-not $pyExe) { Fail 'Python 3.8+ not found (py -3 or python)' }
$v = & $pyExe @pyPre -c "import sys; print('%d.%d' % sys.version_info[:2])" 2>$null
if ($LASTEXITCODE -ne 0 -or -not $v -or [version]"$v" -lt [version]'3.8') { Fail "Python 3.8+ not found ($pyExe reports '$v')" }
$pyLine = (@($pyExe) + $pyPre) -join ' '
Write-Host "   $pyLine  (Python $v)"

Step 'allow/deny list (scripts\rea-vendor.py sync)'
Push-Location $Repo
& $pyExe @pyPre scripts\rea-vendor.py sync
$syncExit = $LASTEXITCODE
Pop-Location
if ($syncExit -ne 0) { Fail 'sync failed (see output above)' }

Step 'smoke and wiring launchers'
WriteUtf8 (Join-Path $Rea 'run_smoke.cmd') "@echo off`r`ncd /d `"$Repo`"`r`nset PYTHONIOENCODING=utf-8`r`n$pyLine scripts\rea-vendor.py smoke > vendor\rea\smoke.log 2>&1`r`n>> vendor\rea\smoke.log echo EXIT %ERRORLEVEL%`r`n"
WriteUtf8 (Join-Path $Rea 'run_wiring.cmd') "@echo off`r`ncd /d `"$Repo`"`r`nset PYTHONIOENCODING=utf-8`r`n$pyLine scripts\rea-vendor.py wiring`r`n"
Write-Host "   $Rea\run_smoke.cmd"
Write-Host "   $Rea\run_wiring.cmd"

Step 'INSTALL.md'
$installMd = Join-Path $Rea 'INSTALL.md'
if (-not (Test-Path -LiteralPath $installMd)) {
  WriteUtf8 $installMd @"
# vendor\rea: local REA install (git-ignored)

Installed $(Get-Date -Format s) on $env:COMPUTERNAME by scripts\install-rea.ps1. Pins: vendor/README.md.

- REA rea-agents $reaVer, Node $nodeVer, Ghidra 12.1.4, Temurin JDK 21.0.12.1+1
- Target (pass this absolute path to mcp__rea__open_binary):
  $dst
  FFXiMain.dll retail-2026-09, TDS 0x6A995428, image base 0x10000000, sha256 $targetSha
- Session flags:
  --mcp-config $Rea\rea.mcp.json --settings $Rea\rea-loop-settings.json
- Agent rules: .agents\skills\retail-grounding\references\rea.md

(No pre-made smoke section: whoever runs the smoke appends its own heading and
lines — a stub heading here invited duplicated ones, found 2026-10-14.)
"@
}
Write-Host "   $installMd"

Write-Host ''
Write-Host 'INSTALL PASS'
