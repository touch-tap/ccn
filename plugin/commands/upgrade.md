---
description: Unlock Claude Code Notifier on this computer with a licence, or check the one it has
argument-hint: "[licence key, or \"lifetime\"]"
allowed-tools: Bash(command -v ccn), Bash(ccn licence), Bash(ccn activate:*)
---

Help the user licence this computer for Claude Code Notifier. A licence covers up to 3 computers, and every phone paired with a licensed computer is unlocked. It is bought on the web; keys are never entered in the phone app.

What they gave, if anything: "$ARGUMENTS"

1. Check ccn is installed with `command -v ccn`. If it isn't, tell them to install it first with `/ccn:setup`, and stop.
2. If they gave a key (it looks like `CCN-XXXX-XXXX-XXXX-XXXX`), run `ccn activate <key>` and report the result in a line or two. If the licence is already on 3 computers, show the list it prints and the `--replace` command, but don't run it without them choosing which computer.
3. Otherwise run `ccn licence` and summarise it in one line. If there is no licence, don't start the purchase yourself: it opens a browser and waits for the payment, which needs their terminal. Tell them to type `! ccn upgrade` here (or `! ccn upgrade --lifetime` to pay once), or run it in any terminal. An annual licence can be switched to lifetime the same way, keeping its key.

Keep it short. Never print a full licence key back unless they typed it.
