# ccn — Claude Code Notifier for the terminal (macOS, Windows, Linux)

Get a push on your phone when Claude Code finishes or needs you, and approve, deny or answer from
there, with a Windows toast or macOS notification on the desktop too. Works on macOS, Windows and
Linux, in any terminal, with no app running in the background.

`ccn` is one small binary that Claude Code calls through its hooks. It runs for a few milliseconds
when something happens and exits. Installing it puts the hooks in place at once, so from then on
every `claude` session is covered, however you start it: first as desktop notifications on this
computer, and on your phone as soon as you pair one.

## Install

```sh
# macOS / Linux: installs the hooks, then shows a QR code to pair your phone
curl -fsSL https://claudecodenotifier.com/install.sh | sh
# or pair in the same step with the code your phone shows
curl -fsSL https://claudecodenotifier.com/install.sh | sh -s 482913

# Windows (PowerShell), and pair in the same step
irm https://claudecodenotifier.com/install.ps1 | iex; ccn pair 482913

# Windows with winget or Scoop (once published)
winget install TouchTap.ccn; ccn pair 482913
scoop bucket add touch-tap https://github.com/touch-tap/scoop-bucket; scoop install ccn; ccn pair 482913

# Homebrew, or anyone with Node
brew trust --formula touch-tap/tap/ccn && brew install touch-tap/tap/ccn && ccn
npx @touchtap/ccn

# from source
cargo install --path .
```

Desktop notifications start working as soon as the hooks are installed: Claude finished, Claude
needs permission, Claude has a question. To get them on your phone too, and approve or answer from
there, pair with the [Claude Code Notifier](https://claudecodenotifier.com) app whenever you like:

```sh
ccn                    # show a QR code here and scan it with the phone's camera
ccn pair 482913        # or type the code the app shows under Computers > Add a computer
ccn pair --no-wait     # print the QR code and return at once (for scripts)
```

`ccn` on its own (and so `npx @touchtap/ccn`) installs the hooks and pairs a computer that has no
phone yet. The QR code opens the app, or the App Store / Google Play when it isn't installed, and
lasts a day; you can leave `ccn` waiting or press Ctrl+C, the pairing completes on its own when the
phone connects. Once paired, the phone gets a test notification it can answer, and `ccn` shows the
answer. Until a phone is paired, the first desktop notification and the tenth point at `ccn pair`,
and Claude Code shows a one-line reminder once a week; `ccn config set pair_nudges off` stops that.

### As a Claude Code plugin

```
/plugin install ccn --marketplace touch-tap/ccn
```

or, on Claude Code before 2.1.275, `/plugin marketplace add touch-tap/ccn` then
`/plugin install ccn@touch-tap`. Then run `/ccn:setup` (or `/ccn:setup 482913` with the app's
code). It installs the hooks first (`ccn setup`), then pairs. The plugin's hooks call the installed
`ccn` and stand aside when `ccn setup` has already put ccn's hooks (or the Mac app's) in
`settings.json`, so nothing is sent twice. Until ccn is installed they only print a one-line
pointer when a session starts.

## What you get

| On your phone | How it works underneath |
| --- | --- |
| "Claude has finished" the moment a turn ends | the `Stop` hook |
| Approve or deny a permission before the terminal even asks | the `PermissionRequest` hook returns the decision |
| Answer a question Claude asked | the `PreToolUse` hook on `AskUserQuestion` supplies the answer |
| Send a short follow-up right after Claude finishes | `Stop` returns it as Claude's next instruction |

Before a phone is paired, the same three moments show as desktop notifications on this computer
(no network, no waiting), and nothing is decided for you.

If you are at the keyboard (any input in the last 30 s) nothing waits: the terminal prompt appears
at once. While you are at the terminal Claude runs in, the phone gets nothing (on macOS that means
the terminal is also the frontmost app; elsewhere, keyboard activity alone); if you are active in
another app it gets an informational card. `ccn config set quiet_when_present off` sends the card
every time. If you are away, the hook holds for the phone's
answer, up to a ceiling you choose, and releases the moment you touch the keyboard.

## Commands

```
ccn                     install the hooks, pair if no phone is paired yet, otherwise show the status
ccn pair [CODE]         pair with a phone; without a code, shows a QR code (--no-wait returns at once)
ccn doctor              check hooks, credentials, server, presence detection
ccn test                send a test notification
ccn devices             list paired phones; ccn devices revoke <name>
ccn config show         effective settings (also editable from the phone)
ccn config set KEY VAL  e.g. hold_ceiling_seconds 300, quiet_hours 22:00-08:00, stop_hold_seconds 0
ccn sync                pull settings from the server now
ccn setup               install the hooks and desktop notifications (idempotent; pair does it first)
ccn upgrade [--check]   update ccn to the latest release (checked once a day in the background too)
ccn unlock [--lifetime] buy a licence in the browser: unlocks every phone paired with this computer
ccn activate KEY        use a licence key here (up to 3 computers); ccn licence shows it
ccn uninstall           remove hooks, pairings, credentials and local state
ccn forget [--yes]      erase this computer and its pairings from the server, then uninstall
```

Settings: `notify_on_done`, `notify_on_permission`, `notify_on_question`, `quiet_hours`,
`quiet_when_present` (default on), `hold_ceiling_seconds` (30–3600, default 600),
`presence_threshold_seconds` (default 30), `stop_hold_seconds` (0–600, default 120), `sound`, and
the local-only `desktop_notifications`, `pair_nudges` (suggest pairing while no phone is; default
on), `credential_store` (`keychain` or `file`) and `auto_update`.

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

## Languages

`ccn` speaks the OS language when it has it: English, Arabic, German, Spanish, French, Hebrew,
Italian, Japanese, Korean, Dutch, Polish, Portuguese (Brazil), Russian, Turkish, Vietnamese,
Simplified and Traditional Chinese (the same 17 as the apps and the website). `CCN_LANG=de`
overrides the OS setting, for one command or in your shell profile; anything else falls back to
English. Command names, setting keys and `--help`'s own headings stay English. Translations live
in [`locales/`](locales/README.md), one file per language.

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
failed at which stage; pairing started, paired or failed with a short reason, and whether a refused
code was recovered with the QR code; that the licence prices were shown; how ccn was installed; the
OS and processor). The hooks send one: that the licence reminder was shown, with its variant.
Nothing about your sessions, prompts or computer name is included. Set `CCN_NO_ANALYTICS=1` or
`DO_NOT_TRACK=1` to turn them off. Requests carry an `Accept-Language` header with your OS language
(`CCN_LANG` overrides it) so the server can answer in it.

## Development

```sh
cargo build && cargo test
CCN_SERVER_URL=http://localhost:8787 CCN_HOME=/tmp/ccn-dev CLAUDE_CONFIG_DIR=/tmp/claude-dev \
  ./target/debug/ccn pair 123456
echo '{"hook_event_name":"Stop","session_id":"s1","cwd":"/tmp","last_assistant_message":"done"}' \
  | CCN_PRESENCE=away ./target/debug/ccn hook
```

`CCN_PRESENCE=away|present|file:<path>` pins the presence check for tests, `CCN_NOTIFY=off` keeps
desktop notifications quiet, and `CCN_NO_NUDGE=1` turns off every reminder to pair or to unlock.

Releasing: see [RELEASING.md](RELEASING.md).

Server: [CCNotifier_Server](../CCNotifier_Server) (Cloudflare Worker). Design and roadmap: the
"CCNotifier CLI: the multiplatform plan" doc.

## Licence

ccn is source-available under the [PolyForm Shield License 1.0.0](LICENSE.md). You can use it, change it and share it, including at work, as long as you keep the copyright notice and don't use it to build a product that competes with Claude Code Notifier.
