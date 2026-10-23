# Record the real UsageBar Windows build (dashboard, floating Codex meter, tray).
# Requires a built NSIS installer or src-tauri/target/release/usagebar.exe.
# Launch is --demo / USAGEBAR_DEMO=1, so every meter is sample data.
param(
  [int]$Seconds = 40
)

$ErrorActionPreference = "Stop"
$Root = Split-Path -Parent (Split-Path -Parent $MyInvocation.MyCommand.Path)
Set-Location $Root
$Out = Join-Path $Root "out"
New-Item -ItemType Directory -Force -Path $Out | Out-Null
$Log = Join-Path $env:TEMP "usagebar-demo.log"
$PortFile = Join-Path $env:TEMP "usagebar-demo.port"

function Write-Log($Message) { Write-Host "[demo-record] $Message" }

$script:CopiedLogs = $false
function Copy-DemoLogs {
  if ($script:CopiedLogs) { return }
  $script:CopiedLogs = $true
  Copy-Item $Log (Join-Path $Out "demo.log") -ErrorAction SilentlyContinue
  Copy-Item $PortFile (Join-Path $Out "port.txt") -ErrorAction SilentlyContinue
}
trap { Copy-DemoLogs; throw $_ }

function Send-Demo([string]$Command) {
  $port = [int](Get-Content $PortFile -Raw).Trim()
  $client = New-Object System.Net.Sockets.TcpClient
  $client.Connect("127.0.0.1", $port)
  $stream = $client.GetStream()
  $stream.ReadTimeout = 5000
  $payload = [Text.Encoding]::ASCII.GetBytes("$Command`n")
  $stream.Write($payload, 0, $payload.Length)
  $buffer = New-Object byte[] 512
  $read = $stream.Read($buffer, 0, $buffer.Length)
  $client.Close()
  [Text.Encoding]::ASCII.GetString($buffer, 0, $read).Trim()
}

Add-Type -AssemblyName System.Drawing
Add-Type @"
using System;
using System.Runtime.InteropServices;
public static class DemoMouse {
  [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
  [DllImport("user32.dll")] public static extern void mouse_event(uint flags, uint dx, uint dy, uint data, UIntPtr extra);
  public static void Move(int x, int y) { SetCursorPos(x, y); }
  public static void Click(int x, int y) {
    SetCursorPos(x, y);
    mouse_event(0x0002, 0, 0, 0, UIntPtr.Zero);
    mouse_event(0x0004, 0, 0, 0, UIntPtr.Zero);
  }
}
public class DemoWall {
  [DllImport("user32.dll", CharSet = CharSet.Unicode)]
  public static extern int SystemParametersInfo(int action, int param, string path, int winIni);
}
"@

try {
  $wall = Join-Path $env:TEMP "usagebar-wall.png"
  $bmp = New-Object System.Drawing.Bitmap 1920, 1080
  $gfx = [System.Drawing.Graphics]::FromImage($bmp)
  $gfx.Clear([System.Drawing.Color]::FromArgb(255, 244, 240, 255))
  $bmp.Save($wall, [System.Drawing.Imaging.ImageFormat]::Png)
  $gfx.Dispose(); $bmp.Dispose()
  [DemoWall]::SystemParametersInfo(20, 0, $wall, 3) | Out-Null
} catch {
  Write-Log "wallpaper skipped: $_"
}

Get-Process UsageBar, usagebar -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
Remove-Item $Log, $PortFile -ErrorAction SilentlyContinue

$installed = $null
$setup = Get-ChildItem "$Root\src-tauri\target\release\bundle\nsis\*-setup.exe" -ErrorAction SilentlyContinue | Select-Object -First 1
if ($setup) {
  Write-Log "installing $($setup.Name)"
  $proc = Start-Process $setup.FullName -ArgumentList "/S" -PassThru -Wait
  Write-Log "installer exit $($proc.ExitCode)"
  Start-Sleep -Seconds 1
  foreach ($candidate in @(
      "$env:LOCALAPPDATA\UsageBar\UsageBar.exe",
      "$env:LOCALAPPDATA\Programs\UsageBar\UsageBar.exe"
    )) {
    if (Test-Path $candidate) { $installed = Get-Item $candidate; break }
  }
  if (-not $installed) {
    $installed = Get-ChildItem "$env:LOCALAPPDATA\Programs" -Recurse -Filter UsageBar.exe -ErrorAction SilentlyContinue |
      Select-Object -First 1
  }
}
if (-not $installed) {
  $fallback = Join-Path $Root "src-tauri\target\release\usagebar.exe"
  if (-not (Test-Path $fallback)) { throw "no UsageBar.exe and no NSIS installer" }
  Write-Log "running unpackaged $fallback"
  $installed = Get-Item $fallback
}
Write-Log "exe $($installed.FullName)"

$env:USAGEBAR_DEMO = "1"
Start-Process -FilePath $installed.FullName -ArgumentList "--demo" -WorkingDirectory $installed.DirectoryName

$ready = $false
foreach ($i in 1..80) {
  if ((Test-Path $PortFile) -and (Test-Path $Log) -and (Select-String -Path $Log -Pattern "listening" -Quiet)) {
    $ready = $true
    break
  }
  Start-Sleep -Milliseconds 500
}
if (-not $ready) {
  Copy-DemoLogs
  if (Test-Path $Log) { Get-Content $Log }
  throw "demo control did not start"
}
$pong = Send-Demo "PING"
Write-Log "ping: $pong"
if ($pong -ne "PONG demo") { throw "demo mode did not answer: $pong" }
Start-Sleep -Seconds 1

$ffArgs = @(
  "-y", "-f", "gdigrab", "-framerate", "30", "-draw_mouse", "1", "-i", "desktop",
  "-t", "$Seconds", "-c:v", "libx264", "-preset", "veryfast", "-crf", "18", "-pix_fmt", "yuv420p",
  (Join-Path $Out "usagebar-windows-raw.mp4")
)
Write-Log "recording ${Seconds}s"
$ff = Start-Process ffmpeg -ArgumentList $ffArgs -PassThru -NoNewWindow
Start-Sleep -Seconds 2

# Dashboard is already open. Hover the floating meter, then expand it even if
# the pointer misses the hover target.
$widget = Send-Demo "GEOMETRY widget"
Write-Log "widget: $widget"
$parts = $widget.Split(" ", [System.StringSplitOptions]::RemoveEmptyEntries)
if ($parts.Length -ge 7 -and $parts[0] -eq "GEOMETRY") {
  $cx = [int]$parts[1] + [int]([int]$parts[3] / 2)
  $cy = [int]$parts[2] + [int]([int]$parts[4] / 2)
  [DemoMouse]::Move($cx, $cy)
  Start-Sleep -Milliseconds 700
}
Write-Log "widget expand: $(Send-Demo 'WIDGET EXPAND')"
Start-Sleep -Seconds 3.2
Write-Log "widget collapse: $(Send-Demo 'WIDGET COLLAPSE')"
Start-Sleep -Milliseconds 800

$main = Send-Demo "GEOMETRY main"
Write-Log "main: $main"
$mp = $main.Split(" ", [System.StringSplitOptions]::RemoveEmptyEntries)
if ($mp.Length -ge 7 -and $mp[0] -eq "GEOMETRY") {
  $mx = [int]$mp[1] + 80
  $my = [int]$mp[2] + 160
  [DemoMouse]::Move($mx, $my)
}
foreach ($step in 1..8) {
  Send-Demo "SCROLL 175" | Out-Null
  Start-Sleep -Milliseconds 1050
}
Start-Sleep -Milliseconds 400
Write-Log "settings: $(Send-Demo 'SETTINGS')"
Start-Sleep -Seconds 3.2
Write-Log "close settings: $(Send-Demo 'CLOSE_SETTINGS')"
Start-Sleep -Milliseconds 800
Send-Demo "SCROLL_TOP" | Out-Null
Start-Sleep -Seconds 1

# Hide the dashboard and bring it back from the tray, which is the same window.
Write-Log "hide: $(Send-Demo 'HIDE')"
Start-Sleep -Seconds 1
$sw = 1024; $sh = 768
if ($mp.Length -ge 7) { $sw = [int]$mp[5]; $sh = [int]$mp[6] }
try {
  Add-Type -AssemblyName UIAutomationClient, UIAutomationTypes -ErrorAction Stop
  $root = [System.Windows.Automation.AutomationElement]::RootElement
  $cond = New-Object System.Windows.Automation.PropertyCondition(
    [System.Windows.Automation.AutomationElement]::NameProperty, "UsageBar")
  $btn = $root.FindFirst([System.Windows.Automation.TreeScope]::Descendants, $cond)
  if ($btn) {
    $rect = $btn.Current.BoundingRectangle
    $tx = [int]($rect.X + $rect.Width / 2)
    $ty = [int]($rect.Y + $rect.Height / 2)
    Write-Log "tray click at $tx,$ty"
    [DemoMouse]::Click($tx, $ty)
  } else {
    Write-Log "tray element not found"
    [DemoMouse]::Click($sw - 70, $sh - 12)
  }
} catch {
  Write-Log "tray automation failed: $_"
  [DemoMouse]::Click($sw - 70, $sh - 12)
}
Start-Sleep -Seconds 1
$visible = Send-Demo "VISIBLE"
Write-Log "visible after tray: $visible"
if ($visible -ne "VISIBLE 1") {
  Write-Log "show hook: $(Send-Demo 'SHOW')"
}
Start-Sleep -Seconds 2

if (-not $ff.HasExited) { $ff.WaitForExit() }
try { Send-Demo "QUIT" | Out-Null } catch {}
Copy-DemoLogs

$raw = Join-Path $Out "usagebar-windows-raw.mp4"
if (-not (Test-Path $raw) -or (Get-Item $raw).Length -lt 10000) {
  throw "recording missing or tiny: $raw"
}
Write-Log "wrote $raw ($([int]((Get-Item $raw).Length / 1MB)) MB)"
New-Item -ItemType Directory -Force -Path (Join-Path $Out "frames") | Out-Null
ffmpeg -y -i $raw -vf "fps=1/2" (Join-Path $Out "frames\win-%02d.png")
if ($LASTEXITCODE -ne 0) { Write-Log "frame extract failed (video is still uploaded)" }
