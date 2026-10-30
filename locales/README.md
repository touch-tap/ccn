# Translating ccn

`ccn` is the Claude Code Notifier command for the terminal. Everything a person reads from it
(terminal output, desktop notifications, the one-line messages it hands to Claude Code, `--help`
descriptions) lives in one YAML file per language in this folder. `en.yml` is the source; a
translation is a copy of it with the values changed and the keys left alone. No Rust changes are
needed: the files are compiled into the binary, and a missing file or key falls back to English.

## Files

One file per language, named exactly (case matters):

```
en.yml  ar.yml  de.yml  es.yml  fr.yml  he.yml  it.yml  ja.yml  ko.yml  nl.yml  pl.yml
pt-BR.yml  ru.yml  tr.yml  vi.yml  zh-Hans.yml  zh-Hant.yml
```

These are the same 17 as the phone apps and the website. The binary maps the OS locale onto one of
them (`pt_PT` → `pt-BR`, `zh-TW`/`zh-HK` → `zh-Hant`, everything else Chinese → `zh-Hans`,
`iw` → `he`); `CCN_LANG=de` overrides the OS.

Keep the file's shape: `_version: 1` at the top, then the same nested keys as `en.yml`, in the
same order, so a diff against English is readable. Comments (`# ...`) are notes for you; copy or
drop them as you like. Every value is double-quoted. Inside the quotes, `\"` is a quote and `\n`
is a line break; leading spaces (`"  ccn unlock ..."`) are deliberate indentation and must stay.

A new or edited file is picked up by the next `cargo build` (build.rs watches this folder).

`cargo test` checks a translation: it must have exactly the keys `en.yml` has, no more and no
fewer, every value non-empty, and the same `%{placeholders}` in each value as English. Run it
(or ask someone to) before handing a file in.

## Placeholders

`%{name}` is filled in at run time: `%{path}`, `%{version}`, `%{count}`, `%{price}`. Keep every
placeholder the English value has, spelt exactly the same, and move it wherever your sentence needs
it. Never translate the word inside the braces. A few keys say in a comment that a value arrives
padded to a column width (`devices.row`, `licence.computer_row`): keep the placeholders in the
same order there, or the columns stop lining up.

`common.date_format` is not a sentence but a strftime pattern for dates in sentences ("annual,
renews 27 Oct 2027"). The program only knows English month names, so use a numeric form for your
language: `"%d.%m.%Y"` (de), `"%d/%m/%Y"` (fr, es, pt-BR, it, tr, vi), `"%Y/%m/%d"` (ja, zh),
`"%Y.%m.%d."` (ko), `"%d.%m.%Y"` (ru, pl), `"%d-%m-%Y"` (nl), `"%d/%m/%Y"` (ar, he).

Plurals: the only counted words are hours and minutes for how long a pairing code lasts.
`pair.hours_one`/`pair.minutes_one` are used for exactly 1 and `pair.hours_other`/
`pair.minutes_other` for every other number (2, 5, 24, 90). Write `_other` so it reads correctly
for all of those in your language, even if it would ideally have more forms. Other counts
(`%{count} of 3 computers`) are in a fixed frame; phrase them so they read for 0 to 3.

## What must stay as it is

Do not translate:

- Command lines and command names: `ccn pair`, `ccn pair <code>`, `ccn setup --replace-mac-app`,
  `ccn unlock --lifetime`, `ccn activate <key>`, `ccn doctor`, `ccn config set pair_nudges off`,
  `claude`, `npx @touchtap/ccn`. The `<code>` and `<key>` placeholders inside them may be
  translated if your language's readers would expect it, but keep the angle brackets.
- Setting keys and their values: `notify_on_done`, `quiet_hours`, `hold_ceiling_seconds`,
  `credential_store`, `auto_update`, `pair_nudges`, `keychain`, `file`, `on`, `off`, `HH:MM`.
  They are what the user types back.
- File and product names: `settings.json`, `SHA256SUMS`, `ccn.log`, `config.toml`, `tmux`,
  `AppleScript`, `npx`, `Homebrew`, App Store, Google Play, `Ctrl+C`, `Enter`.
- Product names: "Claude Code Notifier", "Claude Code", "Claude". The Mac app is "the Claude Code
  Notifier Mac app"; the phone app is "the Claude Code Notifier app".
- URLs and paths: `claudecodenotifier.com/licence`, `%{path}`, `%{url}`.
- Key formats: `CCN-…-%{last4}`.
- The `ok ` / `!! ` markers are added by the program, not in the YAML.

Translate "System Settings > Notifications" the way Apple writes it in your language, and
"Computers > Add a computer" the way the phone app shows it (see the app's own strings; this is
the Settings tab's "Computers" screen and its "Add a computer" button).

`forget.yes_word` is the word the user types to confirm `ccn forget` (English `yes` is always
accepted too); `forget.prompt` shows it through `%{yes}`. `licence.no_initial` is the first letter
of your word for "no", for the `[Y/n]` prompt in `licence.yes_no`; `n` always means no as well.

## Register and voice

Match the phone apps, the website and the push copy for your language (PushCopy strings in the
iOS and Android apps), so a person who installs ccn from the app hears the same voice:

- fr: `tu`, not `vous`
- de: `du`
- tr: `siz`
- ko: 해요체 (polite informal: 해요, 돼요)
- ja: です/ます, no 敬語 beyond that
- he: plural imperative where English gives an instruction (הריצו, סרקו), otherwise neutral
- ar: Modern Standard Arabic, second person singular masculine for instructions is fine as the
  apps do it
- es: `tú`; neutral Latin American and European vocabulary where possible (no `vosotros`)
- pt-BR: `você`
- it, nl, pl, ru, vi, zh: the informal-but-polite register of good developer tools

Imperatives are instructions to the reader ("Run `ccn pair` in a terminal"), not commands at
them; keep them short. "Phone" means the paired iPhone or Android phone; "this computer" means the
one `ccn` is running on. "Pair" is the product's word for connecting a phone (the apps use it
too), "unlock" for buying a licence; use the same words the app uses on screen.

## Length

- Desktop notification bodies (`local.*`, `hook.unpaired_notification`, `hook.needs_attention`):
  at most 200 characters after `%{...}` is filled in, and ideally one short sentence, because
  macOS and Windows cut notifications to two or three lines. `local.first_nudge` is appended to
  the end of another message, so keep it very short (English: 33 characters).
- `systemMessage` lines (`local.pair_message`, `hook.*_message`, `update.available_message`): one
  line that Claude Code shows in the terminal; aim for the English length, never more than about
  220 characters. They start with "Claude Code Notifier:" so the reader knows who is talking: keep
  that prefix.
- `--help` descriptions (`cli.*`): one line each, no full stop at the end (clap prints them in a
  table). `cli.about` is the first line of `ccn --help`.
- Column layouts (`status.hint_*`, `licence.buy_options`, `licence.have_key`, `pair.pitch_*`):
  the command and the explanation are separated by spaces so they line up; keep the spaces and
  change only the words after them.
- Multi-line values (`pair.scan`, `pair.done`, `setup.mac_app_note`, `forget.warning`) are wrapped
  at about 95 characters with `\n`; rewrap for your language so no line is much longer than that.

## Checking your work

```sh
cargo test                      # keys and placeholders match English
cargo build && CCN_LANG=de ./target/debug/ccn --help
CCN_LANG=de ./target/debug/ccn licence
CCN_LANG=de ./target/debug/ccn config set sound maybe    # an error message
```

If a value looks right in the file but English shows up, the key is misspelt or the file is named
wrongly (it must be one of the 17 names above, exactly).
