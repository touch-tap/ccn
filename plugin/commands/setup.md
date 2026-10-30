---
description: Connect Claude Code Notifier, so Claude can reach your phone when it needs you
argument-hint: "[6-digit code from the app]"
allowed-tools: Bash(command -v ccn), Bash(ccn doctor), Bash(ccn setup), Bash(npx -y @touchtap/ccn setup), Bash(npx -y @touchtap/ccn pair:*), Bash(ccn pair:*)
---

Help the user connect this computer to the Claude Code Notifier phone app (iOS or Android). It sends a push when Claude finishes or needs permission, and they can approve, deny or answer from the phone.

The code they gave, if any: "$ARGUMENTS"

1. Check whether ccn is installed with `command -v ccn`. If it is, run `ccn doctor` and summarise anything that isn't working.
2. Install the hooks first, before any pairing: `ccn setup` when ccn is installed, otherwise `npx -y @touchtap/ccn setup`. It is quiet when they are already in place. From now on they get desktop notifications on this computer whenever Claude finishes or needs them, even before a phone is paired; say so in one line.
3. If they gave a 6-digit code (shown in the app under "Connect your computer"), pair with it yourself: `ccn pair <code>` when ccn is installed, otherwise `npx -y @touchtap/ccn pair <code>`. Report the result in one or two lines, and tell them a test notification is on its way to their phone.
4. If they gave no code, do not run the pairing yourself: it shows a QR code and waits for the phone, which needs their terminal. Tell them, briefly, to either:
   - type `! npx @touchtap/ccn` here (or run `npx @touchtap/ccn` in any terminal) and scan the QR code with the phone's camera, which also offers the app if they don't have it yet; or
   - open the app (App Store: https://apps.apple.com/app/id6757701908, Google Play: https://play.google.com/store/apps/details?id=com.touchtap.ccnotifier), choose their computer, and run `/ccn:setup <code>` with the 6-digit code it shows.

Keep it short. Never print or store the user's credentials.
