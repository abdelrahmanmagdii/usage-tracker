# UsageBar on Windows

UsageBar started as a macOS menu-bar app. The Windows build reuses the React dashboard and Tauri backend, with a Windows notification-area
icon, native login-file paths, and an NSIS `.exe` installer. It is intended for
Windows 10/11 x64 with WebView2.

## 1. Install build prerequisites once

Install the following prerequisites if they are missing:

- [Node.js 24 LTS](https://nodejs.org/).
- [Visual Studio 2022 Build Tools](https://visualstudio.microsoft.com/visual-cpp-build-tools/).
  In its installer select **Desktop development with C++**, including the MSVC
  compiler and Windows SDK.
- [Rust through rustup](https://rustup.rs/). Use the default **MSVC** toolchain.
- [Microsoft Edge WebView2 Runtime](https://developer.microsoft.com/en-us/microsoft-edge/webview2/)
  if it is not already installed.

See the official [Tauri prerequisites](https://v2.tauri.app/start/prerequisites/)
for details. Restart PowerShell/VS Code after installing so it sees the new PATH.
Check `node --version`, `rustc --version`, and `cargo --version`.

## 2. Run from source

Open PowerShell and run:

```powershell
cd 'C:\path\to\usage-tracker'
npm.cmd ci
npm.cmd run tauri dev
```

Keep that terminal open during development. The first launch compiles Rust
dependencies and can take several minutes. `npm.cmd` avoids PowerShell script
execution-policy issues. `npm run dev` alone opens only the web frontend and
cannot read real usage through the native backend.

## 3. Connect your tools

Sign in through the Windows version of the coding tool you want to track, then
click **Refresh** in UsageBar. UsageBar has no separate account or API key field.
At least one provider must have a supported local login to show real usage.

| Provider | Windows login source |
| --- | --- |
| Codex | A working `codex` command on PATH, using its existing CLI login and App Server. Verify `codex --version`, and sign in through the CLI if necessary. |
| Claude Code | `%USERPROFILE%\.claude\.credentials.json`, or `.credentials.json` in `CLAUDE_CONFIG_DIR` when set. Sign in through Claude Code; the Claude desktop app uses a separate login. |
| Cursor | `%APPDATA%\Cursor\User\globalStorage\state.vscdb`. Sign in inside Cursor. |
| OpenCode Go | `%USERPROFILE%\.local\share\opencode\auth.json`, or `%XDG_DATA_HOME%\opencode\auth.json`. Requires a Go login, rather than a Zen entry. |
| Devin CLI | `%USERPROFILE%\.local\share\devin\credentials.toml`, or `%XDG_DATA_HOME%\devin\credentials.toml`. Requires a CLI that writes this supported format. |
| Antigravity | Keep the signed-in Windows app or IDE running. UsageBar discovers its loopback listener and, for newer IDE builds, reads the short-lived local CSRF token from Antigravity's own session log. A still-valid token in `%USERPROFILE%\.gemini\antigravity-cli\antigravity-oauth-token` is also supported. |
| Gemini CLI | `%USERPROFILE%\.gemini\oauth_creds.json`. Sign in with Google in the Gemini CLI; API-key and Vertex AI auth have no quota to read. |

Windows home directories use `USERPROFILE`, with `HOME` as a fallback.
macOS Keychain integration is skipped on Windows. Tools installed and signed in
only inside WSL are not automatically discovered; use native Windows logins.
Optional provider paths and unofficial endpoints can change between tool versions.
These integrations still require live verification with your signed-in tools.

If Codex is installed in a custom location, set `USAGEBAR_CODEX_PATH` to the
full path of its `codex.exe` or `codex.cmd` before launching UsageBar. For a
development session, for example:

```powershell
$env:USAGEBAR_CODEX_PATH = 'C:\path\to\codex.exe'
npm.cmd run tauri dev
```

The app also checks the standard `%APPDATA%\npm\codex.cmd` location.
After changing PATH, restart UsageBar; a desktop sign-out/sign-in may be needed
for Start-menu launches to inherit a new PATH.

## 4. Everyday use

The **Codex floating meter** starts automatically at the bottom-right of your
primary display, touching the top edge of the taskbar. It stays on top when the dashboard is
hidden. Its default size is a single 224 × 26 logical-pixel row, showing both the
**5h** and **7d** (weekly) quota remaining and their reset countdowns in rounded
chips. Each chip has a thin quota-remaining line. Percentages turn amber at
35% remaining and red at 20%. Hover for 250 ms to expand
the meter and see the local reset day/time beneath each countdown; move away
to collapse it again. The native window shrinks too, so the unused area does
not block clicks. Click
**Codex ↗** to open the dashboard or the refresh icon to fetch fresh readings.
Missing readings show a dash; stale percentages have a `~` prefix. Hover and
drag the dotted grip at the left of the expanded header to move the meter.
It stays where you put it for the current session and returns to the taskbar
corner on the next launch. The **↳** hover button restores its bottom-right
position immediately. The **−** button hides the meter; right-click a tray
icon and choose **Show Codex meter** to restore it. The **×** button quits
UsageBar, closing both the dashboard and meter.

- The window opens on launch and appears in the Windows taskbar by default,
  alongside the system-tray icon. Click the tray icon near the clock to hide
  or restore it. If Windows puts that icon in the overflow area, click **^**.
- Hover over the icon for a usage tooltip. Open the dashboard to see all
  providers, quota windows, remaining percentages, and reset countdowns.
- Windows does not display percentage/countdown text beside tray icons.
  Long tooltips are shortened to Windows' 127 UTF-16-unit limit.
- In **Settings**, choose providers, Compact (one icon) or Extended (one icon
  per tool), alert thresholds, and **Launch at Login**.
- Closing or dismissing the dashboard hides it and removes its taskbar button
  while tracking and the floating meter continue. Click the tray icon or the
  meter's Codex label to restore it. The normal minimize button still minimizes
  to the taskbar. To stop the installed
  app, right-click a tray icon and choose **Quit UsageBar**.
- Percentages mean quota **remaining**. This app tracks provider quota windows;
  it is not a system-wide network, token, or API billing meter.
- If readings become stale, open the relevant CLI/app to renew its login and
  refresh UsageBar. It never refreshes or rewrites those credentials itself.

## 5. Build and install for daily use

To install a standalone copy in `%LOCALAPPDATA%\UsageBar` and enable startup
automatically for your Windows account, stop the development copy and run:

```powershell
npm.cmd run windows:install
```

It builds the app with the frontend embedded, copies it to that stable folder,
and registers **UsageBar** in Windows Startup Apps. At sign-in only the floating
meter appears; the dashboard stays hidden. VS Code, Node.js, Rust, and the
development server do not need to be running. Turn off **Launch at Login** in
UsageBar Settings or disable **UsageBar** in Windows **Settings > Apps > Startup**
to stop automatic launches. Re-run the install command after making changes to
update that installed copy. `npm.cmd run windows:install -- -DebugBuild` uses a
faster standalone debug build with the same startup behavior and no console window.

To build an installer instead:

Stop the development copy before opening an installed copy. In PowerShell:

```powershell
cd 'C:\path\to\usage-tracker'
npm.cmd run windows:build
Get-ChildItem '.\src-tauri\target\release\bundle\nsis\*-setup.exe'
```

Run the generated `*-setup.exe`, then start **UsageBar** from the Start menu.
You can close the build terminal after installation. The installer is per-user.
This local build is unsigned, so Windows may show a publisher/SmartScreen prompt.
Uninstall through **Settings > Apps > Installed apps > UsageBar**.

Windows auto-updates are disabled because the existing signed update feed is
for the upstream macOS app. Build and run a newer Windows installer to update.
The original `app:install` script is macOS-only; use `windows:build` on Windows.

Alternatively, after committing and pushing this port to your GitHub repository,
run **Actions > Windows build > Run workflow**, then download the
**UsageBar-Windows** artifact from the successful run and extract its installer.
The workflow tests and builds without publishing a release or requiring signing
secrets. No workflow was run as part of making this local port.

## Checks and troubleshooting

```powershell
npm.cmd run tauri -- info
npm.cmd run build
npm.cmd test
cargo test --manifest-path src-tauri/Cargo.toml
```

- `cargo` is missing: install Rust and restart your terminal. The npm Tauri
  launcher also checks the default Windows rustup directory, so it can work
  when VS Code still has the PATH from before Rust was installed.
- `link.exe` or Windows SDK missing: modify Build Tools and add the C++ workload.
- Missing provider: check that the native Windows tool is signed in and that
  its login is stored in the supported location above.
- No notifications: install the app first and check Windows notification settings
  and Do Not Disturb. Notifications from development builds may behave differently.
- A hidden dashboard remains available from the tray and floating meter; closing it does not quit.

The frontend production build and all 88 JavaScript tests passed during the
port. The native Windows checks, application build, installer packaging, and
installed application launch also pass. Provider behavior depends on the local
versions and login state of the supported tools and should be exercised before
publishing a Windows release.
Installer packaging follows [Tauri's Windows installer documentation](https://v2.tauri.app/distribute/windows-installer/).
