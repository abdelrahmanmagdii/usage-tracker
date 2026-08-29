# App Store Connect copy

Paste these fields when creating the Mac app **UsageBar** (`com.abdelrahmanamer.usagebar`). Privacy URL and support URL must stay live (GitHub Pages on `main`).

## Listing

- **Name:** UsageBar
- **Subtitle (30):** Quota meters for AI coding
- **Category:** Developer Tools
- **Secondary:** Productivity
- **Age:** 4+
- **Price:** Free
- **Copyright:** 2026 Abdelrahman Amer
- **Marketing URL:** https://abdelrahmanmagdii.github.io/usage-tracker/
- **Privacy URL:** https://abdelrahmanmagdii.github.io/usage-tracker/privacy.html
- **Support URL:** https://abdelrahmanmagdii.github.io/usage-tracker/support.html

## Description

UsageBar lives in the Mac menu bar and shows how much Codex, Claude Code, Cursor, and OpenCode Go quota you have left — and when each window resets.

Numbers match the official apps. Compact mode keeps a single icon so macOS is less likely to hide it. Pin 5-hour, weekly, or a model window such as Fable.

Usage stays on this Mac. Codex talks to the local Codex app server. Claude, Cursor, and OpenCode reuse the logins those tools already keep.

Requires the matching CLI or app to be signed in on this Mac.

## Keywords (100 characters)

quota,codex,claude,cursor,opencode,menubar,ratelimit,reset,usage,developer

## What's New (0.1.0)

First release: menu bar meters for Codex, Claude, Cursor, and OpenCode Go, plus Tibo Watch reset radar.

## App Privacy

UsageBar does not operate a backend and does not receive your quota. Declare **no data collected by the developer**.

Claude, Cursor, and OpenCode meters send the login those tools already store to the matching vendor usage API (Anthropic, Cursor, OpenCode) so the meter can render. Codex stays on-device via the local app server. That is App Functionality for those vendors' products, not tracking by UsageBar. Align the nutrition labels with `website/privacy.html`.

## App Review Information (Guideline 2.1)

Apple’s “Information Needed” reply is a questionnaire, not a crash. Do **not** upload a new binary unless they ask. Reply in the Resolution Center, attach the screen recording, then paste the Notes block below into **App Review Information → Notes** so the next submission already has it.

### Screen recording (required)

Record on a physical Mac, latest macOS, QuickTime Player → File → New Screen Recording. Capture the **full menu bar**. Keep it under ~2 minutes. Start from a cold launch:

1. Show Desktop / Finder. Open `/Applications` and double-click **UsageBar**.
2. Point at the menu bar (no Dock icon — this is an Accessory / `LSUIElement` app). Click the compact meters (`97 · 57 · …`).
3. Click each provider row if more than one is visible (Codex, Claude, Cursor, OpenCode).
4. Click the wide **Settings** button. Show Tools (hide/show a provider), then Layout (compact vs extended).
5. Close Settings. Click **Refresh**. Right-click the menu bar icon and show Quit (do not have to quit).
6. If macOS shows Keychain (“Claude Code-credentials”) or notification permission, leave those prompts in the clip.

Skip anything the app does not have: UsageBar account, IAP, subscriptions, UGC, ATT, camera, location, contacts.

Name the file something like `UsageBar-review-demo.mov` and attach it on the Resolution Center thread.

### Notes field (paste)

Replace the Mac model with Apple menu → About This Mac.

```
USAGEBAR — APP REVIEW INFORMATION

UsageBar is a macOS menu bar extra (NSApplicationActivationPolicyAccessory / LSUIElement). It has no Dock icon and no main window. After launch, look at the top-right menu bar for compact meters such as "97 · 57". Click that item to open the popover. Settings is the wide button at the bottom. Right-click the menu bar item for Settings, Refresh, and Quit.

1. SCREEN RECORDING
Attached. Recorded on a physical Apple silicon Mac running the latest macOS. It begins by launching UsageBar from /Applications, then shows the menu bar item, popover meters, Settings (tools and layout), Refresh, and the context menu.

No UsageBar account, registration, login, or account deletion.
No in-app purchases or subscriptions.
No user-generated content, reporting, or blocking.
No location, contacts, camera, microphone, or App Tracking Transparency prompts.
macOS may show a Keychain prompt the first time the Claude meter reads the existing "Claude Code-credentials" item, and a notification prompt if alerts are enabled. Those system prompts are included when they appear.

2. DEVICES AND OS TESTED BEFORE SUBMITTING
- [MODEL from About This Mac], Apple silicon, macOS 26.3 (physical device)
The Mac App Store binary is Apple silicon only, minimum macOS 12.

3. FUNCTIONS AND TARGET AUDIENCE
UsageBar is a free Developer Tools app for people who write software with Codex, Claude Code, Cursor, and/or OpenCode Go. Those tools bury remaining quota inside a CLI or nested settings, so it is easy to hit a rate-limit reset mid-session. UsageBar shows remaining percent and the official reset countdown in the Mac menu bar — the same numbers those apps already display — so leftover quota is visible at a glance. Compact mode keeps a single icon so macOS is less likely to hide it.

4. SETUP AND ACCESS TO MAIN FEATURES
UsageBar has no account of its own. We cannot issue OpenAI, Anthropic, Cursor, or OpenCode demo logins.

To use the app:
a) Launch UsageBar from Applications. It appears only in the menu bar.
b) Have at least one signed-in tool on the same Mac: Codex CLI (`codex login`), Claude Code CLI, the Cursor app, or OpenCode Go (`/connect`).
c) Click the menu bar meters. Hidden tools are not polled. Onboarding copy explains how to sign in if a meter is missing.
d) Settings (wide footer button): which tools to show, which quota window each meter follows, compact vs extended, alerts, launch at login.

Sandbox: read-only temporary exceptions for ~/.claude, ~/.codex, ~/.local/share/opencode, ~/Library/Application Support/Cursor, and spawning the user-installed Codex binary at /opt/homebrew/bin/codex and /usr/local/bin/codex. UsageBar does not write those locations.
Launch at Login may be unavailable in the App Store sandbox; the app is fully usable without it. System Settings → Login Items can add UsageBar.

5. EXTERNAL SERVICES
UsageBar does not operate a backend. Quota is never sent to the developer.
- Codex: local `codex app-server --stdio` on this Mac.
- Claude Code: existing OAuth token to https://api.anthropic.com/api/oauth/usage
- Cursor: local login database to https://api2.cursor.sh
- OpenCode Go: local auth.json to https://opencode.ai/zen/go/v1/usage
- Tibo Watch (optional reset radar): public JSON at https://raw.githubusercontent.com/abdelrahmanmagdii/usage-tracker/main/data/resets.json
No payment processor. No UsageBar authentication. HTTPS only; ITSAppUsesNonExemptEncryption is false.

6. REGIONAL DIFFERENCES
None. Features and content are the same in every region.

7. REGULATED INDUSTRY / PROTECTED MATERIAL
Not a regulated industry. UsageBar is an independent utility, not affiliated with OpenAI, Anthropic, Cursor, or OpenCode. It does not redistribute those products or their content. It only displays quota the user already has through tools they installed. No extra license or credential is required.
```

### Resolution Center cover note

```
Thank you for the review. UsageBar is a Mac menu bar extra with no Dock icon — after launch, the meters are in the top-right menu bar.

I have attached a screen recording from a physical Mac that starts at launch and walks through the popover and Settings. Answers to items 1–7 are in this reply and in App Review Information → Notes.

There is no UsageBar account or demo login. Live meters need at least one of Codex, Claude Code, Cursor, or OpenCode already signed in on the Mac. The recording shows that signed-in state. The app is free, with no IAP, UGC, or ATT.
```

## Screenshots

Upload the 1280×800 PNGs in `store/screenshots/` (Mac size in App Store Connect). They are the real popover, laid out on a 16:10 canvas. Optional 2560×1600 retina if you recapture later.
