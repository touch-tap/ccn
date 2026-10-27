#!/bin/sh
# The plugin's hooks hand each Claude Code event (JSON on stdin) to ccn. ccn stands aside when
# settings.json already has its own hooks or the Mac app's, so nothing is sent twice.
# Until ccn is installed, the only output is a one-line pointer at the start of a session.
event="$1"

# ccn is a native binary wherever it is installed from. Other tools are also called ccn (an npm
# package installs a Node script by that name), so anything starting with #! is not ours.
is_ccn() {
  [ -f "$1" ] && [ -x "$1" ] && [ "$(head -c 2 "$1" 2>/dev/null)" != "#!" ]
}

for candidate in "$HOME/.local/bin/ccn" "$(command -v ccn 2>/dev/null)" /opt/homebrew/bin/ccn /usr/local/bin/ccn \
  "${LOCALAPPDATA:+$LOCALAPPDATA/Programs/ccn/ccn.exe}"; do
  if [ -n "$candidate" ] && is_ccn "$candidate"; then
    exec "$candidate" hook --plugin
  fi
done

if [ "$event" = SessionStart ]; then
  printf '%s\n' '{"systemMessage":"Claude Code Notifier is not connected yet. Run /ccn:setup, or `npx @touchtap/ccn` in a terminal, to get a push on your phone when Claude needs you."}'
fi
exit 0
