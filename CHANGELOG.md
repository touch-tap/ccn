# Changelog

## 0.4.6 (not released)

> **Do not release before the Antigravity Notifier phone apps are live on the App Store and
> Google Play.** This version suggests Antigravity Notifier to people who use Google Antigravity.
> On 27 Nov 2026 both apps were still in store review, so the suggestion links to
> antigravitynotifier.com and never to a store; released earlier, it would send people to a
> website with no app to download.

- **Windows: a hook could be kept waiting by ccn's own background work.** The daily update check
  and the anonymous setup events are started by the hook and outlive it, and on Windows they kept
  the hook's output open, so whatever was reading it (Claude Code, a wrapper script) waited until
  they had finished: seconds for a check, as long as a download for an update. Both now start with
  none of the hook's handles. macOS and Linux were not affected.
- If you also use Google Antigravity (`agy`) and don't have antigravity-notifier, ccn mentions
  [Antigravity Notifier](https://antigravitynotifier.com), the same thing for Antigravity, with its
  one-line install: after `ccn pair` (with a QR code for the website), in `ccn` and `ccn doctor`,
  and at most twice ever when Claude Code starts. Only one app is ever suggested at a time,
  Notifier for Codex first, and the two share the limit of twice. `ccn config set cross_promo off`
  or `CCN_NO_NUDGE=1` turns both off, as before. In all 17 languages.
- The README and `ccn pair --help` now say what a bare `ccn pair` does: in a terminal it asks for
  the 6-digit code the app shows, and pressing Enter without one shows a QR code to scan instead.

## 0.4.5 and earlier

See the [releases on GitHub](https://github.com/touch-tap/ccn/releases).
