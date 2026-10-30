# ccn — Claude Code Notifier for the terminal

Get a push on your phone when Claude Code finishes or needs you, and approve, deny or answer from
there. Works on macOS, Linux and Windows, in any terminal, with no app running in the background.

`ccn` is one small binary that Claude Code calls through its hooks. It runs for a few milliseconds
when something happens and exits. Install it once with `ccn pair`; after that every `claude`
session is covered, however you start it.

## Install

```sh
# macOS / Linux, and pair in the same step with the code your phone shows
curl -fsSL https://claudecodenotifier.com/install.sh | sh -s 482913

# Windows (PowerShell)
irm https://claudecodenotifier.com/install.ps1 | iex; ccn pair 482913

# Homebrew, or anyone with Node
brew trust --formula touch-tap/tap/ccn && brew install touch-tap/tap/ccn && ccn pair 482913
npx @touchtap/ccn pair 482913

# from source
cargo install --path .
```

Then pair with the [Claude Code Notifier](https://claudecodenotifier.com) app on your phone:

```sh
ccn pair 482913        # the code the app shows
ccn                    # or show a QR code here and scan it with the phone's camera
```

`ccn` on its own (and so `npx @touchtap/ccn`) pairs a computer that isn't paired yet. The QR code
opens the app, or the App Store / Google Play when it isn't installed. Once paired, the phone gets
a test notification it can answer, and `ccn` shows the answer.

### As a Claude Code plugin

```
/plugin install ccn --marketplace touch-tap/ccn
```

or, on Claude Code before 2.1.275, `/plugin marketplace add touch-tap/ccn` then
`/plugin install ccn@touch-tap`. Then run `/ccn:setup` (or `/ccn:setup 482913` with the app's
code). The plugin's hooks call the installed `ccn` and stand aside when `ccn pair` has already
put ccn's hooks (or the Mac app's) in `settings.json`, so nothing is sent twice. Until ccn is
installed they only print a one-line pointer when a session starts.

## What you get

| On your phone | How it works underneath |
| --- | --- |
| "Claude has finished" the moment a turn ends | the `Stop` hook |
| Approve or deny a permission before the terminal even asks | the `PermissionRequest` hook returns the decision |
| Answer a question Claude asked | the `PreToolUse` hook on `AskUserQuestion` supplies the answer |
| Send a short follow-up right after Claude finishes | `Stop` returns it as Claude's next instruction |

If you are at the keyboard (any input in the last 30 s) nothing waits: the terminal prompt appears
at once. While you are at the terminal Claude runs in, the phone gets nothing (on macOS that means
the terminal is also the frontmost app; elsewhere, keyboard activity alone); if you are active in
another app it gets an informational card. `ccn config set quiet_when_present off` sends the card
every time. If you are away, the hook holds for the phone's
answer, up to a ceiling you choose, and releases the moment you touch the keyboard.

## Commands

```
ccn                     pair if this computer isn't paired yet, otherwise show its status
ccn pair [CODE]         pair with a phone (installs the hooks); without a code, shows a QR code
ccn doctor              check hooks, credentials, server, presence detection
ccn test                send a test notification
ccn devices             list paired phones; ccn devices revoke <name>
ccn config show         effective settings (also editable from the phone)
ccn config set KEY VAL  e.g. hold_ceiling_seconds 300, quiet_hours 22:00-08:00, stop_hold_seconds 0
ccn sync                pull settings from the server now
ccn setup               reinstall the hooks (after moving the binary)
ccn upgrade [--check]   update ccn to the latest release (checked once a day in the background too)
ccn unlock [--lifetime] buy a licence in the browser: unlocks every phone paired with this computer
ccn activate KEY        use a licence key here (up to 3 computers); ccn licence shows it
ccn uninstall           remove hooks, pairings, credentials and local state
ccn forget [--yes]      erase this computer and its pairings from the server, then uninstall
```

Settings: `notify_on_done`, `notify_on_permission`, `notify_on_question`, `quiet_hours`,
`quiet_when_present` (default on), `hold_ceiling_seconds` (30–3600, default 600),
`presence_threshold_seconds` (default 30), `stop_hold_seconds` (0–600, default 120), `sound`, and
the local-only `desktop_notifications`, `credential_store` (`keychain` or `file`) and `auto_update`.

## Updating

Once a day, when a Claude Code session starts, `ccn` checks for a newer release in the background
(a detached process: the hook itself never waits on the network). When ccn owns its binary (the
install scripts, `npx`, or a copy you made) it downloads the release for this computer, checks it
against the release's `SHA256SUMS`, writes it beside the old binary and renames it into place, so a
hook running mid-update never sees a half-written file. The macOS binaries carry the same Developer
ID signature from one version to the next, so the keychain token keeps working.

```sh
ccn upgrade             # update now (also: ccn update)
ccn upgrade --check     # just say whether a newer release exists (exit 10 if so)
ccn config set auto_update off   # or CCN_NO_AUTO_UPDATE=1: no daily check, no background update
```

When a package manager owns the binary ccn never replaces it. `ccn upgrade` prints the command to
run instead, and the daily check mentions a newer release in Claude Code once a week:

| Installed with | Update with |
| --- | --- |
| Homebrew | `brew upgrade touch-tap/tap/ccn` |
| npm | `npm install -g @touchtap/ccn@latest` |
| scoop | `scoop update ccn` |
| winget | `winget upgrade TouchTap.ccn` |

## Files

- `~/.config/ccn/` (`%APPDATA%\ccn\` on Windows, or `$CCN_HOME`): `config.toml`, `sessions/`,
  `ccn.log`
- The computer's token: the macOS keychain or Windows Credential Manager (item `com.touchtap.ccn`),
  or `credentials` in the folder above (mode 0600) on Linux, or wherever `credential_store` says.
  A token from an older version moves to the keychain the first time it is read. The macOS release
  binaries are signed with TouchTap's Developer ID, so each new version reads it without a prompt;
  a build from source may prompt instead, so `ccn config set credential_store file` suits that.
- `~/.claude/settings.json`: seven hook entries, each `ccn hook`. Everything else in the file is
  left untouched, and a one-time backup is kept beside it.

## Setup analytics

`ccn pair` and the install scripts send a few anonymous events (install started, finished or
failed at which stage; pairing started, paired or failed with a short reason; how ccn was
installed; the OS and processor). The hooks send none, and nothing about your sessions, prompts or
computer name is included. Set `CCN_NO_ANALYTICS=1` or `DO_NOT_TRACK=1` to turn them off.

## Development

```sh
cargo build && cargo test
CCN_SERVER_URL=http://localhost:8787 CCN_HOME=/tmp/ccn-dev CLAUDE_CONFIG_DIR=/tmp/claude-dev \
  ./target/debug/ccn pair 123456
echo '{"hook_event_name":"Stop","session_id":"s1","cwd":"/tmp","last_assistant_message":"done"}' \
  | CCN_PRESENCE=away ./target/debug/ccn hook
```

`CCN_PRESENCE=away|present|file:<path>` pins the presence check for tests.

Releasing: see [RELEASING.md](RELEASING.md).

Server: [CCNotifier_Server](../CCNotifier_Server) (Cloudflare Worker). Design and roadmap: the
"CCNotifier CLI: the multiplatform plan" doc.

## Licence

ccn is source-available under the [PolyForm Shield License 1.0.0](LICENSE.md). You can use it, change it and share it, including at work, as long as you keep the copyright notice and don't use it to build a product that competes with Claude Code Notifier.
