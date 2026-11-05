# Simulating UsageBar

Three ways to see the app run without a Mac, real logins, or real quota:

| What | Command | Runs | Good for |
| --- | --- | --- | --- |
| Browser simulation | `npm run sim`, then open `/sim.html` | the unmodified React app in any browser, with a simulated backend | UI work, any scenario, time travel |
| Screenshot suite | `npm run sim:shots` | the same page in headless Chromium (Playwright) | visual review, the CI smoke test, CSP check |
| Real app on Linux | `npm run sim:linux` | the real Tauri binary (Rust backend + WebKitGTK) under Xvfb, demo mode | checking the real webview and backend wiring |

Everything writes to `sim-shots/` (gitignored).

## 1. Browser simulation (`sim.html`)

```sh
npm run sim                     # Vite on http://localhost:1430
open "http://localhost:1430/sim.html?hud"
```

(Any Vite dev server serves it: `npm run dev` → `http://localhost:1420/sim.html`.)

`sim.html` loads `src/sim/main.ts`, which installs a simulated backend and
then imports the **unmodified** `src/main.tsx`. Nothing in the app knows it is
simulated. The harness only touches the boundary the app already talks
through, so it keeps working while app internals are refactored:

- **Tauri IPC** — `mockWindows` + `mockIPC` from `@tauri-apps/api/mocks`.
  Every command in `src-tauri/src/commands.rs` is answered by
  `src/sim/backend.ts` (`get_*_state`, `refresh_*`, `get_app_prefs`,
  `get_tray_windows`, all `set_*` settings, `complete_onboarding`,
  `is_demo_mode`, `get_autostart`, `set_reset_incoming`, `open_post_url`,
  window commands, the notification plugin). Events — `<provider>://state`,
  `usagebar://prefs`, `usagebar://show-onboarding`, `usagebar://demo` — go
  through a small listen/unlisten/emit bus in `main.ts`. (Not
  `shouldMockEvents`: in `@tauri-apps/api` 2.12 that mock never removes a
  listener on unlisten — it reads `args.id` while the API sends `eventId` —
  so StrictMode's double-mounted effects would receive every event twice.)
  An IPC command the simulator does not know is rejected like Tauri does
  (`command X not found`) and logged as a `[sim] unhandled IPC command`
  warning; `sim:shots` fails on it, so a new command shows up immediately.
- **`Date`** — replaced by a subclass whose "now" comes from a simulated
  clock with a speed multiplier. Timers stay real, so the app's one-second
  tick sees `speed` simulated seconds per tick.
- **`fetch`** — the reset feed (`raw.githubusercontent.com/.../resets.json`)
  and the Bluesky relays get fixtures; any other cross-origin request is
  refused and logged. A simulation is offline and repeatable.
- **`Notification`** — records instead of showing (`window.__sim.notifications`).
- **localStorage** — the app's `codex-meter.*` keys are cleared at boot, then
  `codex-meter.quota-history.v1` is seeded from the same model the backend
  serves, so the per-window charts and "at this pace" projections have data
  that leads straight into the live numbers.

### The simulated backend

- **Prefs** follow `src-tauri/src/prefs.rs`: sparse per-provider map,
  defaults omitted on the wire, `usagebar://prefs` broadcast after every change
  that goes through `tray::apply_preference_change` (not after
  `complete_onboarding`), thresholds validated like
  `normalize_alert_thresholds`. Hidden providers are not polled; showing one
  refreshes all, like `set_provider_visible`.
- **Quota windows** use the shapes and starting numbers from
  `src-tauri/src/demo.rs` (`rateLimitsByLimitId` snapshots, `primary` /
  `secondary`, unix-second `resetsAt`). Each provider has a seeded activity
  curve (10-minute slots: working-hours sessions, quiet nights and weekends,
  occasional bursts). A window's usage is the activity consumed since it
  started, scaled so the scenario's number holds exactly at the start; windows
  renew on their period, so moving the clock past `resetsAt` renews them.
  Values are whole percents, like the real APIs.
- **Polling**: Codex every 30 s (it pushes in reality), Claude 60 s,
  Antigravity 120 s, the others 300 s — simulated seconds. A connection change
  (sign-out, recovery, App Server start) is reported at once.
- **Failures**: a provider that fails keeps its last good reading
  (`Showing the last known usage …`), `refresh_*` can reject (the frontend's
  `stateAfterRefreshFailure` path), `cli_not_found` hides it.
- **Usage alerts** follow `alerts.rs` (first sighting seeds silently, one
  notification for the highest threshold crossed, "fresh window" when usage
  drops from ≥50% to <10%) into `__sim.notifications`. **Pace alerts** ("runs
  out in ~… at this pace") use the popover's `projectedRunOut` with the
  `alerts.rs` firing rules: at most once per window, ≥30 min of samples,
  confirmed on two refreshes in a row, run-out within 3 h and at least
  max(30 min, 10% of the window) before the reset, below the top threshold.
- **Tray title**: `__sim.tray()` returns what the Rust tray would show
  (combined `53% · 9% · …`, `~` when stale, `⚡` while a reset is incoming).

Determinism: the model is a pure function of `seed`, `scenario`, `start` and
the simulated time (plus anything you trigger through `__sim`). With a fixed
`seed` and `start` and `speed=0` the page renders identically every time;
activity uses UTC hours so it does not depend on the machine's time zone
(the app's own "Tonight · 5:12 PM" labels do — `sim:shots` pins `UTC`).
`sim:shots` also stops CSS animations for each capture (see below).

### Scenarios (`?scenario=`)

| id | what it shows |
| --- | --- |
| `default` | demo.rs numbers on a normal working day; Claude's Fable week renews 47 minutes in |
| `busy` | every tool in heavy use, bursts, windows hitting 100%, a Codex reset announced for +42 min (reset banner, Reset Radar, `⚡` tray) |
| `idle` | nothing used for six hours: meters hold still, countdowns run, windows renew |
| `nearly-out` | most windows above 85%, several at the limit; alerts fire as usage crosses 95% |
| `errors` | Devin's API returns 503 (refresh rejects) until +25m, Gemini signed out with a personal account, Antigravity not running; Claude signs out at +10m and recovers at +40m; Codex App Server restarts at +20m; Cursor times out at +60–90m |
| `fresh-install` | onboarding open, no history, only Codex and Claude Code signed in; Codex "starting" for 3 s |

### URL parameters

| param | default | meaning |
| --- | --- | --- |
| `scenario` | `default` | see above |
| `seed` | `1` | activity seed |
| `start` | now | simulated start, ISO (`2026-11-05T15:00:00Z`) or epoch ms |
| `speed` | `1` | simulated seconds per real second; `0` freezes the clock (screenshots), `60` = a minute per second |
| `advance` | – | open the app this far into the scenario (`90m`, `2h`, `1d6h`); history is seeded up to that moment |
| `providers` | all | visible providers, e.g. `codex,claude` (others start hidden in prefs) |
| `dense` | off | dense layout pref |
| `combined=0` | on | one menu-bar item per provider (affects `__sim.tray()`) |
| `onboarding=1` / `0` | per scenario | force the walkthrough on / off |
| `alerts=0` | on | usage-alerts pref |
| `pace=0` | on | pace-alerts pref (only fires while usage alerts are on) |
| `alert=42` | per scenario | announce a Codex reset this many minutes after the start |
| `feed` | `fixture` | `fixture`, `empty`, or `live` (let feed/Bluesky requests through) |
| `demo` | off | `is_demo_mode` → true (header says "Demo data", no reset-feed polling) |
| `notify` | `granted` | `Notification.permission`: `granted`, `denied`, `default` |
| `latency` | `650` | refresh latency in ms (randomised ±50%) |
| `history=0` | per scenario | do not seed quota history; `historyHours` (12), `historyStep` (15 min) tune it |
| `keep` | off | keep existing localStorage instead of clearing the app's keys |
| `autostart` | off | `get_autostart` result |
| `platform` | browser | `windows` or `mac` user agent (Windows copy: "system tray", "PC") |
| `bg` | `wallpaper` | what is behind the transparent window: `wallpaper`, `white`, `black`, `none` |
| `hud` | off | control strip: simulated time, tray title, speed / jump / scenario buttons |
| `settings` | off | open Settings after boot |
| `widget` | off | render the Codex floating widget (`?widget`, as `src/main.tsx` reads it) |
| `verbose` | off | log every invoke/emit to the console |

### `window.__sim`

```js
__sim.now()                    // simulated ISO time
__sim.speed(600)               // 10 simulated minutes per second; __sim.pause(), __sim.play(1)
__sim.advance("2h")            // jump ahead ("90", "15m", "1d6h"); __sim.travelTo("2026-11-06T09:00:00Z")
__sim.signOut("claude", 600)   // signed out for 10 simulated minutes (omit to stay signed out)
__sim.fail("cursor", "HTTP 500", 300)   // error state; refresh_cursor rejects meanwhile
__sim.uninstall("devin"); __sim.recover("devin"); __sim.setConnection("codex", "starting", undefined, 5)
__sim.surpriseReset("codex")   // renew a provider's windows early (the app may flag a surprise reset)
__sim.refresh()                // poll every provider now (or __sim.refresh("claude"))
__sim.state("claude")          // what get_claude_state would return
__sim.prefs(); __sim.setPrefs({ denseLayout: true })   // emits usagebar://prefs
__sim.openSettings(); __sim.closeSettings(); __sim.scroll(300); __sim.showOnboarding()
__sim.emit("usagebar://show-onboarding", null)          // any raw event
__sim.tray()                   // { combined, title, titles[] } the Rust tray would show
__sim.notifications            // usage alerts + app notifications
__sim.log                      // last 2000 invokes / emits / warnings
__sim.unhandled()              // IPC commands the simulator did not know
```

`openSettings`/`closeSettings`/`scroll` use the app's `usagebar://demo`
listener (the one the recording scripts drive); if that ever becomes
demo-only, click the footer button instead.

### Extending

- New backend command → add a `case` in `SimBackend.handle` (`src/sim/backend.ts`).
- New provider → add it to `SIM_PROVIDERS` and `baseProviders()` in
  `src/sim/model.ts` (the `get_<id>_state` / `refresh_<id>` / `<id>://state`
  names follow automatically).
- New scenario → a function in `model.ts` registered in `SCENARIOS`.
- `npx vitest run src/sim` covers the model and the prefs helpers.

`sim.html` is not part of `npm run build` (Vite only builds `index.html`), so
the harness never ships in the app bundle.

## 2. Screenshot suite (`npm run sim:shots`)

```sh
npm run sim:shots                                   # everything below, ~1.5 min
npm run sim:shots -- --scenarios busy,errors --themes dark --no-full
npm run sim:shots -- --timelapse --video            # + media
npm run sim:shots -- --prod                         # production build + CSP check
npm run sim:shots -- --help
```

Starts Vite on a free port (no clash with a running `npm run dev`), opens
`sim.html` in Chromium at 388×720, deviceScaleFactor 2, time zone UTC, with
`seed=7`, `start=2026-11-05T15:00:00Z`, `speed=0`, and writes to `sim-shots/`:

- `<scenario>-<light|dark>-<roomy|dense>.png` — the popover as it opens;
- `…-full.png` — viewport grown to the full scroll height;
- `settings-<theme>.png` / `-full.png`, `onboarding-<theme>-step1..4.png`,
  `widget-<theme>.png` / `-hover.png`;
- `--timelapse`: `timelapse/frame-NNN.png` (busy day, 15-minute steps, HUD
  on), `timelapse.mp4`, `timelapse.gif` (ffmpeg);
- `--video`: `walkthrough.webm` / `.mp4` — scroll, Fable renewal, Claude
  sign-out, Settings, dense layout;
- `report.json` — per capture: page errors, console errors/warnings, CSP
  reports, failed requests, blocked network requests, unhandled IPC
  commands, the simulated tray title; for the run: options, Chromium
  version, and the fonts the UI and the numbers were actually drawn with;
- `index.html` — the contact sheet: scenario × theme × density grid,
  Settings / onboarding / widget rows, every issue printed under the
  capture that logged it, full-height captures one click away. Relative
  links, so it opens straight from an unzipped CI artifact;
- `contact-sheet.png` — the same page as one image.

`node tools/sim/contact-sheet.mjs DIR…` rebuilds `index.html` from
`report.json`; with `--summary` it prints the Markdown CI puts in the job
summary.

Exit code is 1 if any capture threw, logged a page error or console error,
or invoked an unknown IPC command. `--prod` builds `sim.html` for production
and serves it with the CSP from `src-tauri/tauri.conf.json` as
`Content-Security-Policy-Report-Only` (dev mode has no CSP, matching
`devCsp: null`); violations land in `report.json` and fail the run with
`--strict-csp`. `--copy-to DIR` also copies the output elsewhere.

Note: "dark" captures use Chromium's `prefers-color-scheme: dark`. The app
currently pins `color-scheme: light` and has no dark styles, so they only
differ in the simulated wallpaper behind the window.

Screenshots are taken with Playwright's `animations: "disabled"`:
transitions are finished and infinite animations (the danger-dot pulse, the
meter sheen, spinners) stop on their first frame. Without it the pulsing
Reset Radar dot made `busy` captures differ from run to run; with it two runs
on the same machine are byte-identical, and dev and `--prod` captures match.

Playwright is a devDependency, pinned exactly (`1.63.0`): each Playwright
release drives one Chromium revision, so the version decides which browser
build CI runs. After `npm ci`, download that browser once with
`npx playwright install chromium` (`--only-shell` is enough; on Linux add
`--with-deps` for the system libraries). A global Playwright install also
works; `PLAYWRIGHT_BROWSERS_PATH` is honoured as usual. ffmpeg is optional.

**Fonts.** The app has no web fonts: the UI uses
`-apple-system, BlinkMacSystemFont, "SF Pro Text", "Segoe UI", Inter, sans-serif`
and numbers `ui-monospace, SFMono-Regular, Menlo, monospace`. On a Mac that is
SF Pro and SF Mono / Menlo; on Linux it is whatever is installed, so install
Inter (`fonts-inter` on Debian/Ubuntu) or the UI falls back to DejaVu Sans
and looks nothing like the real app. `report.json` → `fonts` and the last
line of the run say what was used (CI: Inter and DejaVu Sans Mono). Pixels
from different machines are not comparable; captures from one machine are.

### In CI

The `simulator` job in `.github/workflows/ci.yml` runs on every pull request,
on pushes to `main`, and from the Actions tab (`ubuntu-24.04`, about 4–5
minutes in all):

1. `npx playwright install --with-deps --only-shell chromium`, plus
   `fonts-inter`;
2. `npm run sim:shots -- --out sim-shots/dev --themes light --no-full` —
   the Vite dev server, where React reports its own mistakes (missing keys,
   invalid nesting) as console errors that a production build strips;
3. `npm run sim:shots -- --prod --strict-csp` — the full set against a
   production build with the app's CSP; any CSP report fails the job. Runs
   even if step 2 failed;
4. `node tools/sim/contact-sheet.mjs --summary sim-shots sim-shots/dev` into
   the job summary: pass/fail per capture, the failures' first lines, fonts;
5. `sim-shots/` (both runs, `index.html`, `contact-sheet.png`,
   `report.json`) uploaded as the `sim-shots` artifact, also when a step
   failed. Download it from the run page and open `index.html`.

The job runs with `TZ=UTC` and `LANG=C.UTF-8`; the pages themselves are
pinned to UTC / `en-US`, seed 7 and a frozen clock at
`2026-11-05T15:00:00Z`. To reproduce a CI failure locally, run the same
command; to see the captures as CI does, use the artifact.

There is deliberately no pixel-baseline comparison: an intended UI change
would fail it, and font and antialiasing differences between machines make
baselines brittle. Review the contact sheet instead.

## 3. Real app on Linux (`npm run sim:linux`)

Linux is not a supported target, but the app builds and runs here unchanged:

```sh
npm run sim:linux                    # debug build; frontend served by Vite
npm run sim:linux -- --embedded      # frontend built into the binary, production CSP active
npm run sim:linux -- --no-demo       # real providers against an empty, isolated $HOME
npm run sim:linux -- --hold 600      # keep it running on the Xvfb display for 10 minutes
```

What it does: `cargo build` (a debug build pointed at a private Vite port with
`TAURI_CONFIG='{"build":{"devUrl":…}}'`, or `--features tauri/custom-protocol`
after `npm run build` for `--embedded`), copies the binary into
`sim-shots/linux/` (the cargo target dir may be shared), starts `Xvfb`, and
launches `usagebar --demo` under `dbus-run-session` with `HOME`/`XDG_*`
pointing at `sim-shots/linux/home`. It then drives the window through the
demo control port (`SHOW`, `GEOMETRY main`, `SCROLL 700`, `SETTINGS`,
`QUIT`; see `src-tauri/src/demo.rs`) and screenshots the root window with
ImageMagick `import`, cropped to the window: `linux-main.png`,
`linux-scrolled.png`, `linux-settings.png` (plus `*-screen.png`), and
`linux-run.log`.

Requirements: Rust, the WebKitGTK 4.1 / GTK 3 / libayatana-appindicator dev
packages, `Xvfb`, `dbus` (`dbus-run-session`), ImageMagick.

Findings on Linux (Debian-based container, Xvfb, no window manager):

- `cargo build` succeeds with no source changes (only dead-code warnings for
  macOS keychain helpers). The `cfg(not(macos))` code paths compile.
- The app starts, the demo control port answers, `show_dashboard` shows the
  388×560 window, WebKitGTK renders the real frontend with real demo data,
  scrolling and Settings work, and `QUIT` exits cleanly. The production build
  (`--embedded`) renders under the real CSP with no visible breakage.
- The tray icon is created without error, but there is no StatusNotifier host
  under Xvfb, so nothing displays it; the menu-bar title cannot be seen
  (use `__sim.tray()` in the browser simulation instead).
- No vibrancy/blur: the window is plain transparent, and without a compositor
  transparent pixels come out black (the black frame and the small black
  square at the top-left, where the hidden dismiss button sits).
- The first frame can take a few seconds to reach the X framebuffer
  (software GL, `DRI3` warning); the script retries until it is not black.
- Notifications go to `xdg-desktop-portal`, which is activated on the private
  D-Bus session but has no notification backend; nothing is shown. The
  script kills the session's portal processes and unmounts the document
  portal's FUSE mount when it exits.
- Without `--demo`, the window opens on the onboarding walkthrough (fresh
  prefs in the isolated `$HOME`) and every provider reports not found.
