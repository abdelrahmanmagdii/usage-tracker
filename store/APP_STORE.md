# App Store Connect copy

Paste these fields when creating the Mac app **UsageBar** (`com.usagebar.app`). Privacy URL and support URL must stay live (GitHub Pages on `main`).

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

Share a 16:9 snapshot to X or LinkedIn from the popover. The card is percentage, window, and countdown. No email, tokens, or paths.

Usage stays on this Mac. Codex talks to the local Codex app server. Claude, Cursor, and OpenCode reuse the logins those tools already keep.

Requires the matching CLI or app to be signed in on this Mac.

## Keywords (100 characters)

quota,codex,claude,cursor,opencode,menubar,ratelimit,reset,usage,developer

## What's New (0.1.0)

First release: menu bar meters for Codex, Claude, Cursor, and OpenCode Go, Tibo Watch reset radar, and share cards for X and LinkedIn.

## App Privacy

UsageBar does not operate a backend and does not receive your quota. Declare **no data collected by the developer**.

Claude, Cursor, and OpenCode meters send the login those tools already store to the matching vendor usage API (Anthropic, Cursor, OpenCode) so the meter can render. Codex stays on-device via the local app server. That is App Functionality for those vendors' products, not tracking by UsageBar. Align the nutrition labels with `website/privacy.html`.

## Review notes

UsageBar is a menu bar extra (LSUIElement / Accessory). Click the meter in the menu bar to open the popover.

It cannot invent quota without a signed-in provider:

1. Install the Codex CLI and run `codex login`, and/or Claude Code CLI, Cursor, OpenCode Go.
2. Allow Keychain access if macOS asks.
3. To inspect Share without a live quota, the GitHub README demo GIF shows the card. Share copies a PNG (it stays on the clipboard) and opens https://x.com/intent/tweet or LinkedIn share-offsite. The user pastes the image; caption is a separate copy.

Temporary sandbox exceptions let the app read `~/.claude`, Cursor's Application Support DB, OpenCode `auth.json`, and spawn the user-installed `codex` binary (Homebrew `/opt/homebrew/bin/codex` and `/usr/local/bin/codex`). Network opens for X/LinkedIn use `NSWorkspace`; `/usr/bin/open` is a fallback. We do not write those locations.

Launch at Login uses a Launch Agent on the GitHub build. If that is blocked in the sandbox, reviewers can skip it — the app is fully usable without it. System Settings → Login Items can also add UsageBar.

Encryption: ITSAppUsesNonExemptEncryption is false (HTTPS only).

Demo account: we cannot issue OpenAI/Anthropic/Cursor credentials. Review on a Mac with at least one of those tools signed in, or watch the README demo.

## Screenshots

Capture `store/screenshots/*.html` at 1280×800 (16:10) and upload the 1280×800 Mac size in App Store Connect. Optional 2560×1600 retina.
