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
ccn pair               # or get a code here and type it into the app
```

## What you get

| On your phone | How it works underneath |
| --- | --- |
| "Claude has finished" the moment a turn ends | the `Stop` hook |
| Approve or deny a permission before the terminal even asks | the `PermissionRequest` hook returns the decision |
| Answer a question Claude asked | the `PreToolUse` hook on `AskUserQuestion` supplies the answer |
| Send a short follow-up right after Claude finishes | `Stop` returns it as Claude's next instruction |

If you are at the keyboard (any input in the last 30 s) nothing waits: the terminal prompt appears
at once and the phone gets an informational card. If you are away, the hook holds for the phone's
answer, up to a ceiling you choose, and releases the moment you touch the keyboard.

## Commands

```
ccn pair [CODE]         pair with a phone (installs the hooks)
ccn doctor              check hooks, credentials, server, presence detection
ccn test                send a test notification
ccn devices             list paired phones; ccn devices revoke <name>
ccn config show         effective settings (also editable from the phone)
ccn config set KEY VAL  e.g. hold_ceiling_seconds 300, quiet_hours 22:00-08:00, stop_hold_seconds 0
ccn sync                pull settings from the server now
ccn setup               reinstall the hooks (after moving the binary)
ccn uninstall           remove hooks, pairings, credentials and local state
```

Settings: `notify_on_done`, `notify_on_permission`, `notify_on_question`, `quiet_hours`,
`hold_ceiling_seconds` (30–3600, default 600), `presence_threshold_seconds` (default 30),
`stop_hold_seconds` (0–600, default 120), `sound`, and the local-only `desktop_notifications`.

## Files

- `~/.config/ccn/` (`%APPDATA%\ccn\` on Windows, or `$CCN_HOME`): `config.toml`, `credentials`
  (mode 0600), `sessions/`, `ccn.log`
- `~/.claude/settings.json`: seven hook entries, each `ccn hook`. Everything else in the file is
  left untouched, and a one-time backup is kept beside it.

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
