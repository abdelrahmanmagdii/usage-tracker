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
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr hWnd);
}
public static class DemoDesktop {
  public delegate bool EnumProc(IntPtr hWnd, IntPtr lParam);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc lpEnumFunc, IntPtr lParam);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr hWnd);
  [DllImport("user32.dll")] public static extern int GetWindowText(IntPtr hWnd, System.Text.StringBuilder lpString, int nMaxCount);
  [DllImport("user32.dll")] public static extern int GetClassName(IntPtr hWnd, System.Text.StringBuilder lpClass, int nMaxCount);
  [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr hWnd, int nCmdShow);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr hWnd, out uint pid);
  [DllImport("user32.dll")] public static extern IntPtr FindWindow(string cls, string title);
  [DllImport("user32.dll")] public static extern IntPtr FindWindowEx(IntPtr parent, IntPtr after, string cls, string title);
  public static void ParkOthers(uint keepPid) {
    EnumWindows((h, l) => {
      if (!IsWindowVisible(h)) return true;
      var cls = new System.Text.StringBuilder(256);
      GetClassName(h, cls, 256);
      string c = cls.ToString();
      if (c == "Shell_TrayWnd" || c == "Shell_SecondaryTrayWnd" || c == "Progman" || c == "WorkerW") return true;
      uint pid;
      GetWindowThreadProcessId(h, out pid);
      if (keepPid != 0 && pid == keepPid) return true;
      var title = new System.Text.StringBuilder(512);
      GetWindowText(h, title, 512);
      string t = title.ToString();
      if (t.IndexOf("UsageBar", System.StringComparison.OrdinalIgnoreCase) >= 0) return true;
      if (t.IndexOf("Codex quota", System.StringComparison.OrdinalIgnoreCase) >= 0) return true;
      if (t.Length == 0) return true;
      ShowWindow(h, 6);
      return true;
    }, IntPtr.Zero);
  }
  public static void HideDesktopIcons() {
    IntPtr progman = FindWindow("Progman", null);
    IntPtr def = FindWindowEx(progman, IntPtr.Zero, "SHELLDLL_DefView", null);
    if (def == IntPtr.Zero) {
      IntPtr worker = IntPtr.Zero;
      while (true) {
        worker = FindWindowEx(IntPtr.Zero, worker, "WorkerW", null);
        if (worker == IntPtr.Zero) break;
        def = FindWindowEx(worker, IntPtr.Zero, "SHELLDLL_DefView", null);
        if (def != IntPtr.Zero) break;
      }
    }
    IntPtr list = FindWindowEx(def, IntPtr.Zero, "SysListView32", "FolderView");
    if (list != IntPtr.Zero) ShowWindow(list, 0);
  }
}
"@

function Set-RecordingResolution {
  try {
    Add-Type @"
using System;
using System.Runtime.InteropServices;
public class DemoDisplay {
  [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Ansi)]
  public struct DEVMODE {
    [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 32)] public string dmDeviceName;
    public short dmSpecVersion;
    public short dmDriverVersion;
    public short dmSize;
    public short dmDriverExtra;
    public int dmFields;
    public int dmPositionX;
    public int dmPositionY;
    public int dmDisplayOrientation;
    public int dmDisplayFixedOutput;
    public short dmColor;
    public short dmDuplex;
    public short dmYResolution;
    public short dmTTOption;
    public short dmCollate;
    [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 32)] public string dmFormName;
    public short dmLogPixels;
    public int dmBitsPerPel;
    public int dmPelsWidth;
    public int dmPelsHeight;
    public int dmDisplayFlags;
    public int dmDisplayFrequency;
    public int dmICMMethod;
    public int dmICMIntent;
    public int dmMediaType;
    public int dmDitherType;
    public int dmReserved1;
    public int dmReserved2;
    public int dmPanningWidth;
    public int dmPanningHeight;
  }
  [DllImport("user32.dll", CharSet = CharSet.Ansi)]
  public static extern int ChangeDisplaySettings(ref DEVMODE devMode, int flags);
}
"@
    $dm = New-Object DemoDisplay+DEVMODE
    $dm.dmSize = [Runtime.InteropServices.Marshal]::SizeOf($dm)
    $dm.dmPelsWidth = 1920
    $dm.dmPelsHeight = 1080
    $dm.dmFields = 0x180000
    $code = [DemoDisplay]::ChangeDisplaySettings([ref]$dm, 0)
    Write-Log "display change result $code (0 means 1920x1080)"
  } catch {
    Write-Log "display change skipped: $_"
  }
}

function Set-CleanDesktop([uint32]$KeepPid) {
  try { [DemoDesktop]::HideDesktopIcons() } catch { Write-Log "desktop icons: $_" }
  try { [DemoDesktop]::ParkOthers($KeepPid) } catch { Write-Log "park windows: $_" }
}

try {
  Set-RecordingResolution
  $wall = Join-Path $env:TEMP "usagebar-wall.png"
  $bmp = New-Object System.Drawing.Bitmap 1920, 1080
  $gfx = [System.Drawing.Graphics]::FromImage($bmp)
  $pen = New-Object System.Drawing.Pen ([System.Drawing.Color]::White)
  for ($y = 0; $y -lt 1080; $y++) {
    $t = $y / 1079.0
    $r = [int](246 + (228 - 246) * $t)
    $gchan = [int](243 + (233 - 243) * $t)
    $pen.Color = [System.Drawing.Color]::FromArgb(255, $r, $gchan, 255)
    $gfx.DrawLine($pen, 0, $y, 1919, $y)
  }
  $pen.Dispose()
  $bmp.Save($wall, [System.Drawing.Imaging.ImageFormat]::Png)
  $gfx.Dispose(); $bmp.Dispose()
  [DemoWall]::SystemParametersInfo(20, 0, $wall, 3) | Out-Null
} catch {
  Write-Log "wallpaper skipped: $_"
}
Set-CleanDesktop 0

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
$appProc = Start-Process -FilePath $installed.FullName -ArgumentList "--demo" -WorkingDirectory $installed.DirectoryName -PassThru
Write-Log "pid $($appProc.Id)"

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
Set-CleanDesktop ([uint32]$appProc.Id)
Start-Sleep -Milliseconds 400
try { [DemoWall]::SetForegroundWindow($appProc.MainWindowHandle) | Out-Null } catch {}
Write-Log "show: $(Send-Demo 'SHOW')"
Start-Sleep -Seconds 1
Set-CleanDesktop ([uint32]$appProc.Id)

# -t is only a backstop. The story quits ffmpeg when the interactions end,
# so the file does not sit on a frozen last frame.
if ($Seconds -lt 36) { $Seconds = 36 }
$raw = Join-Path $Out "usagebar-windows-raw.mp4"
$ffInfo = New-Object System.Diagnostics.ProcessStartInfo
$ffInfo.FileName = "ffmpeg"
$ffInfo.Arguments = "-y -f gdigrab -framerate 30 -draw_mouse 0 -i desktop -t $Seconds -c:v libx264 -preset veryfast -crf 16 -pix_fmt yuv420p `"$raw`""
$ffInfo.UseShellExecute = $false
Write-Log "recording until the storyboard ends (backstop ${Seconds}s)"
$recordStarted = Get-Date
$ff = [System.Diagnostics.Process]::Start($ffInfo)
Start-Sleep -Milliseconds 500
Write-Log "widget expand: $(Send-Demo 'WIDGET EXPAND')"

function Convert-Geometry([string]$line) {
  if (-not $line) { return $null }
  $p = $line.Split(" ", [System.StringSplitOptions]::RemoveEmptyEntries)
  if ($p.Length -lt 7 -or $p[0] -ne "GEOMETRY") { return $null }
  $x = [int][double]$p[1]; $y = [int][double]$p[2]
  $w = [int][double]$p[3]; $h = [int][double]$p[4]
  $sw = [int][double]$p[5]; $sh = [int][double]$p[6]
  if ($x -lt 0) { $w += $x; $x = 0 }
  if ($y -lt 0) { $h += $y; $y = 0 }
  if ($x + $w -gt $sw) { $w = $sw - $x }
  if ($y + $h -gt $sh) { $h = $sh - $y }
  $w -= $w % 2
  $h -= $h % 2
  if ($w -lt 20 -or $h -lt 16) { return $null }
  return "${w}:${h}:${x}:${y}"
}

# The window label is codex-widget. A collapsed meter is 26px tall; keep
# asking until the expanded card is the one we will crop.
$widgetRect = $null
$widget = ""
for ($try = 1; $try -le 8; $try++) {
  Start-Sleep -Milliseconds 400
  $widget = Send-Demo "GEOMETRY codex-widget"
  Write-Log "widget try ${try}: $widget"
  $widgetRect = Convert-Geometry $widget
  if ($widgetRect) {
    $widgetHeight = [int]($widgetRect.Split(":")[1])
    if ($widgetHeight -ge 100) { break }
  }
  Send-Demo "WIDGET EXPAND" | Out-Null
  $widgetRect = $null
}
$main = Send-Demo "GEOMETRY main"
Write-Log "main: $main"
$mainRect = Convert-Geometry $main
if (-not $mainRect -or -not $widgetRect) {
  throw "window geometry not ready: main=$main widget=$widget"
}
# Publish from the moment both windows are in their final rectangles, so the
# clip does not open on the collapsed meter.
$trim = [math]::Round(((Get-Date) - $recordStarted).TotalSeconds + 0.15, 2)
$gifAt = [math]::Round($trim + 0.4, 2)
$shotPath = Join-Path $Out "shot.txt"
$shotBody = @"
kind=pair
main=$mainRect
widget=$widgetRect
trim=$trim
gif_ss=$gifAt
gif_t=6
"@
[System.IO.File]::WriteAllText($shotPath, $shotBody.Replace("`r`n", "`n"))
Write-Log "shot pair main=$mainRect widget=$widgetRect trim=$trim"

Start-Sleep -Seconds 1.6
foreach ($step in 1..12) {
  Send-Demo "SCROLL 150" | Out-Null
  Start-Sleep -Milliseconds 950
}
Start-Sleep -Milliseconds 400
Write-Log "settings: $(Send-Demo 'SETTINGS')"
Start-Sleep -Seconds 4.6
Write-Log "close settings: $(Send-Demo 'CLOSE_SETTINGS')"
Start-Sleep -Milliseconds 500
Send-Demo "SCROLL_TOP" | Out-Null
Start-Sleep -Seconds 1.4
$ended = [math]::Round(((Get-Date) - $recordStarted).TotalSeconds, 2)
[System.IO.File]::AppendAllText($shotPath, "end=$ended`n")
Write-Log "story end ${ended}s"
if (-not $ff.HasExited) { $ff.WaitForExit() }
try { Send-Demo "QUIT" | Out-Null } catch {}
Copy-DemoLogs
if (-not (Test-Path $raw) -or (Get-Item $raw).Length -lt 10000) {
  throw "recording missing or tiny: $raw"
}
Write-Log "wrote $raw ($([int]((Get-Item $raw).Length / 1MB)) MB)"
New-Item -ItemType Directory -Force -Path (Join-Path $Out "frames") | Out-Null
ffmpeg -y -i $raw -vf "fps=1/2" (Join-Path $Out "frames\win-%02d.png")
if ($LASTEXITCODE -ne 0) { Write-Log "frame extract failed (video is still uploaded)" }
