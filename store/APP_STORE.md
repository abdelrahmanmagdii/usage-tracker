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

## What's New (1.0)

First Mac App Store release: menu bar meters, Tibo Watch reset radar, and share cards for X and LinkedIn.

## App Privacy

Data Not Collected. The developer does not operate a collection endpoint. The app reads local CLIs and calls provider usage APIs on your behalf.

## Review notes

UsageBar is a menu bar extra (LSUIElement / Accessory). Click the meter in the menu bar to open the popover.

It cannot invent quota without a signed-in provider:

1. Install the Codex CLI and run `codex login`, and/or Claude Code CLI, Cursor, OpenCode Go.
2. Allow Keychain access if macOS asks.
3. To inspect Share without a live quota, the GitHub README demo GIF shows the card. Share copies a PNG and opens https://x.com/intent/tweet or LinkedIn share-offsite.

Temporary sandbox exceptions let the app read `~/.claude`, Cursor's Application Support DB, OpenCode `auth.json`, and spawn the user-installed `codex` binary. We do not write those locations.

Encryption: ITSAppUsesNonExemptEncryption is false (HTTPS only).

Demo account: we cannot issue OpenAI/Anthropic/Cursor credentials. Review on a Mac with at least one of those tools signed in, or watch the README demo.

## Screenshots

Capture `store/screenshots/*.html` at 1280×800 (16:10) and upload the 1280×800 Mac size in App Store Connect. Optional 2560×1600 retina.
