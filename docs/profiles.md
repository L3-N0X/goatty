# Profiles and settings (Stage 4)

The profile is the source of truth for everything goatty manages. The settings window edits it; files stay inspectable JSON. Code: `crates/goatty-config` (model, store), `crates/goatty-shell` (`generation`, `service`), `crates/goatty-app` (`settings`, `settings_window`, `ui`).

## Two documents: client and host (#14)

| Document | File (`$XDG_CONFIG_HOME/goatty/`) | Lives on | Contains |
|---|---|---|---|
| `ClientSettings` | `settings.json` | the machine running the window | font, theme, cursor, padding, renderer, keybindings, workspace (on launch), debug |
| `HostSettings` | `host.json` | each machine running a goattyd | shell program, managed zsh and its features, user dotfiles, prompt, history |

The split follows where a setting takes effect. The window draws text, so fonts and colors are the client's. Shells run on the daemon's host, so their configuration belongs to that host. A remote host keeps its own `host.json`, and the settings window edits it through the protocol. Connecting to a different host shows that host's shell settings.

## The store

`goatty_config::store` reads and writes both documents:

- **Lenient load.** A missing file means defaults. Invalid JSON means defaults plus an issue, and the file is left as it is. An invalid value (`"size": "big"`, an unknown enum variant, a negative `u32`) reverts *that value* to its default and becomes an `Issue { path: "font.size", message }`. Every other value is kept. This uses `serde_path_to_error` in a loop: deserialize, find the failing path, put the default back there, retry.
- **Unknown keys** (for example, written by a newer goatty) are reported and kept. Saving puts them back. Maps with user-chosen keys (`keybindings`) are exempt (`Document::OPEN_MAPS`).
- **Versions and migrations.** Each document has a `"version"`. Older versions run through `Document::migrations()`, a list of `(from_version, fn(&mut Value))`. A newer version is read as well as possible and flagged.
- **Validation.** `Document::validate` clamps ranges (font size 6–48, line height 1–2, padding 0–48, history 100–10M), reports unknown theme and action names, and drops empty values.
- **Sparse save.** Only values that differ from the defaults are written, so improved defaults reach everyone who never changed that setting. The previous file goes to `backups/<name>-<unix time>.json` (newest 20 kept), and the new one replaces it atomically (temp file + rename). Unchanged content isn't rewritten.
- **Environment overrides.** `GOATTY_*` variables (table in `goatty-config/src/lib.rs`) apply on top for one run and are never saved. The settings window edits the file value and lists the overrides that are active.

## Applying host settings: generations

Host settings compile into shell configuration (plan §6). `goatty_shell::generation`:

1. Render the startup files from the settings (`zsh::render`: `settings.zsh`, `starship.toml`, static `.zshenv/.zprofile/.zshrc/integration.zsh`).
2. If the active generation has identical files *and* the same settings snapshot, do nothing.
3. Otherwise write `generations/.build-<pid>/` with `zsh/`, `host.json` (the snapshot, without env overrides) and `info.json`.
4. Validate with `zsh -n` on every startup file. On failure, discard the build: nothing changes.
5. Rename the build to `generations/<n>` and switch `current` to it by renaming a new symlink over the old one (atomic).
6. Keep the newest 10.

Sessions get `ZDOTDIR=~/.local/share/goatty/current/zsh`. Paths written into the config go through `current`, so the files don't depend on the generation number. Builders lock `generations/.lock` (`File::lock`), so goattyd, `goatty --local` and the CLI can't race.

**Rollback** switches `current` to an older generation *and* restores its `host.json` snapshot as the settings file (backing up the current one). Without restoring the settings, the next session would rebuild from the newer settings and undo the rollback.

**Apply order** (`HostService::set`): parse leniently → compute the diff against the active generation → build + validate + activate → save `host.json`. A configuration that doesn't validate is rejected before the file is touched.

## Protocol (v4)

| Request | Reply | Notes |
|---|---|---|
| `GetHostSettings` | `HostSettings { json, issues, env_overrides, generation }` | `json` is the stored document |
| `SetHostSettings { json, apply }` | `HostSettingsApplied { issues, diff, applied, generation }` | `apply: false` = validate + preview only |
| `ListGenerations` | `Generations { list }` | |
| `Rollback { generation }` | `HostSettings` | `None` = the previous one |
| `SetPalette { session, colors }` | – | 21 `0xRRGGBB`: ANSI 16, fg, bg, cursor, selection fg/bg |
| `ReloadShells` | `ShellsReloaded { count }` | v4: ask the running managed shells to restart with the active generation |

Settings travel as JSON documents rather than typed structs, so a schema change doesn't need a protocol bump. The daemon's parser handles versions and unknown keys.

`SetPalette` fixes #36: the daemon's terminal is authoritative and answers OSC 4/10/11 color queries, so it needs the colors the user actually sees. Clients send their theme after attaching and on every theme change, and resend it after a reconnect. With several clients on one session, the last one wins.

## The app

- `settings::AppSettings` (GPUI global) holds the loaded file, the effective value (file + env, with the font family resolved against the installed fonts), the installed font list and problems. `AppSettings::update` edits, validates, saves and replaces the global. Every view observes the global, so font, size, line height, padding, renderer, cursor shape/blink and theme apply live. The file is polled every 500 ms, so edits in an editor apply live too.
- Keybindings: `goatty_config::keys` holds the action table and defaults. `actions::apply_keybindings` rebuilds every binding with `KeyBinding::load`, so an invalid keystroke is reported instead of panicking.
- Keystrokes are stored canonically (`keys::canonical_keystroke`): lowercase, modifiers ordered `ctrl-alt-shift-super-fn`, aliases like `plus` resolved, and no `shift` in front of a symbol or digit. GPUI on Linux names a symbol key by the character it types and never reports Shift with it (Ctrl+Shift+= is `ctrl-+`, Ctrl+Shift with the German + key is `ctrl-*`), so `ctrl-shift-+` could never fire. Loading rewrites old spellings and drops overrides that repeat a default.
- Theme: `ThemeSettings { name, bold_is_bright, overrides }` resolves to `ThemeColors`. The terminal palette is swappable at runtime (`Terminal::set_palette`), and goatty's own UI colors derive from it (`ui::style::Ui`).
- Ctrl with + / - (Shift optional; on German layouts Ctrl+Shift++ types `*`, also bound) zooms on top of the configured size until Ctrl+0 resets it. Changing the size in settings keeps the zoom offset.

### The settings window

`Ctrl+,` (or `goatty --settings[=page]`) opens it. Pages:

| Page | Edits | Applies |
|---|---|---|
| Appearance | theme gallery, bold-is-bright, font size/line height/renderer, font family (filterable list), padding, cursor shape/blink | live, saved immediately |
| Shell | program, managed zsh, user dotfiles, autosuggestions/highlighting/fzf/integration | staged → preview → apply |
| Prompt | Starship, rich/minimal style, two-line, blank line, duration threshold, prefer own starship.toml | staged |
| History | size, duplicates, leading space, sharing, Atuin, Atuin on Up | staged |
| Shortcuts | per action: key chips (× removes), + records a new keystroke, reset; warns when a recorded key took over another action's key, when a key you added is one the shell uses (Ctrl+R, Ctrl+W, Alt+letters, plain typing keys), and when a keystroke doesn't parse | live |
| Workspace | reattach newest session vs. start new on launch | next launch |
| Profile | file locations, problems, env overrides, export/import, reset, generations with rollback | – |

Host edits collect in a draft. A bar at the bottom offers **Discard**, **Preview** (the host computes the diff of the generated files, shown colored) and **Apply** (a new generation, validated first). When an apply or a rollback changed the shell configuration, the bar offers **Reload running shells** (or **Not now**); see below. Calls to the daemon run off the UI thread. With `--local` there is no daemon, and the window uses `goatty_shell::HostService` in-process: same code path, same files.

**Export** writes `{ "goatty_profile": 1, "client": {...}, "host": {...} }` with non-default values only, minus machine-specific ones (`host.shell.program`, `client.debug`). **Import** applies the client part right away and stages the host part as a draft to preview and apply.

### Reloading running shells (#56)

New sessions start from the active generation. Shells that are already running keep the configuration they started with until they reload. After an apply or a rollback, the settings window offers **Reload running shells**. `goattyd shell reload` does the same from the command line, and `goatty-reload` reloads only the current shell.

- The daemon (or `HostService` in-process with `--local`) remembers the pid of every session it started with goatty's managed zsh (`HostService::track_shell`), and forgets it when the session exits. Plain zsh, other shells and `GOATTY_SHELL_MANAGED=0` sessions aren't tracked.
- `ReloadShells` sends each one `SIGURG`. The signal is ignored by default, so a process that isn't goatty's zsh (after `exec bash`, or a reused pid) is unaffected.
- The managed `.zshrc` traps it (`TRAPURG`). At an idle, empty prompt the shell reloads right away: the trap runs `zle .send-break`, which returns to the top level at once. From a trap, `accept-line` would only take effect on the next keypress. If the signal lands while the prompt is still being drawn (after precmd, before zle reads input), `zle` isn't active yet; a `zle-line-init` hook sees the pending request and does the same as soon as the line editor starts. The precmd hook then runs `goatty-reload` (`exec`, so the pid and the session stay). While a command runs, or with something typed on the line, the reload waits for the next prompt.
- A shell with jobs doesn't reload, since `exec` would lose the jobs. It prints a reminder to run `goatty-reload` instead.
- Reloading loses shell state that isn't in the startup files (variables, functions, `dirstack`). The working directory is kept.

## Themes

`ThemeSettings`: `name` (the theme when not following the system), `follow_system`, `light` / `dark` (used by the desktop's appearance when following), `bold_is_bright`, and `overrides` (any of the 21 colors, applied on top of whichever theme is active).

**User themes** are files in `~/.config/goatty/themes/<id>.json`: `{ "name": …, "colors": { "foreground", "background", "cursor", "selection_foreground", "selection_background", "ansi": [16] } }`. `goatty_config::ThemeLibrary` lists built-ins plus user themes. A user file with a built-in's id replaces that built-in. An unknown theme name falls back to the default and is reported as a problem. Broken theme files are skipped and reported too. The app polls the folder, so dropping a file in makes it appear in the gallery.

**Importing** (`goatty_config::theme_import`, format detected from the content):

| Format | Notes |
|---|---|
| base16, YAML or JSON (classic `base00…base0F`, or tinted-theming's nested `palette:`) | base16-shell's terminal mapping (bg = base00, fg = base05, selection = base02, ANSI from base08–base0E, bright black = base03, bright white = base07) |
| iTerm2 `.itermcolors` | `Ansi 0–15`, foreground, background, cursor, selection, selected text; components 0–1, sRGB assumed |
| Windows Terminal | one scheme object, a list, or a whole `settings.json` (`schemes`); `#RRGGBBAA` ignores alpha |
| goatty theme file | as above |

Imports get a fresh id when the name is taken (`name`, `name-2`, …).

**Editor** (Appearance page): the gallery shows every theme with `mine`, `light` and `dark` badges. With "Follow the system", a Light/Dark selector decides which side a click assigns. Below the gallery are the 21 color swatches (● = overridden). Clicking one opens it in the color picker (below), and "Revert to theme" drops that override. "Reset all colors" clears the overrides. "Save as new theme" writes the colors on screen to a new user theme and switches to it. For an active user theme there are also "Save into …" (overwrite) and "Delete". Deleting a theme the settings point at falls back to the defaults.

**Color picker** (`ui/color_picker.rs`, used by the editor; any other color setting can embed it): a hue ring with a saturation/value square, sliders per model (RGB, HSL, HSV, OKLCH; the OKLCH chroma track is gamut-limited) each with a number field, "Previous → New" (click Previous to go back), and a field that takes whatever you paste: `#rgb`, `#rrggbb` (`#rrggbbaa`, alpha ignored), `0x…`, `rgb()`, `hsl()`, `hsv()`/`hsb()`, `oklch()`, `oklab()` in CSS syntax (commas or spaces, `%`/`deg`/`turn`, `/ alpha`), or three bare numbers read in the selected model. Ctrl+V pastes a color from the clipboard anywhere in the picker, Ctrl+C copies the hex. Out-of-gamut OKLCH/OKLab values are mapped into sRGB by lowering chroma. The picker keeps float state, so dragging doesn't round to 8 bits and hue/saturation/chroma survive passing through gray, black and white. Changes preview live (`AppSettings::preview`, no file write); the file is written when the mouse is released, on Enter, or 600 ms after the last typed change. Colors you settle on (release, Enter, paste, history click, Previous) go into the **history** (24 colors, newest first), kept in `$XDG_STATE_HOME/goatty/color-history.json`: state, not a setting, so it stays out of `settings.json` and exported profiles. Color math and parsing: `goatty_config::colorspace`; history: `goatty_config::color_history`. Colors are opaque sRGB, like the themes.

**Following the desktop** (#51): GPUI reports the window appearance (on Linux from the XDG portal's `org.freedesktop.appearance color-scheme`, live). `Workspace` observes it and calls `AppSettings::set_system_dark`, which re-resolves the theme. The terminal palette, goatty's UI colors and the palette sent to the daemon (`SetPalette`) all follow. At startup the portal answer arrives a moment after the window opens, so goatty switches once right away, through the same path a later desktop toggle takes.

## Command line (#52)

Everything works without a window: scripts, SSH sessions, dotfile setups.

```
goatty settings path                     # settings.json, host.json, themes/
goatty settings show [KEY] [--all]       # stored values, every value, or one dotted key
goatty settings validate [FILE]          # exit 1 on problems (values, themes, keystrokes)
goatty settings set KEY VALUE            # e.g. font.size 15, theme.follow_system true
goatty settings unset KEY
goatty settings export FILE|-            # profile: these settings + the host's shell settings
goatty settings import FILE|- [--yes]    # host part goes to the local goattyd (else this machine's host.json)

goatty theme list | import FILE… [--name N] | export ID [FILE] | set ID [--for light|dark] | delete ID

goattyd settings path | show [KEY] [--all] | validate [FILE] | set KEY VALUE | unset KEY
```

`set` takes a dotted key and reads the value as JSON when it parses (`5000`, `true`, `null`, `"#ff00ff"`), otherwise as a string (`minimal`, `/bin/zsh`). Unknown keys and invalid values are errors, and nothing is written (`store::set_key`). Entries of open maps (`keybindings.ctrl-k`) are allowed. `goattyd settings set` applies like the settings window's Apply: a validated new generation, then the save. Client changes made with `goatty settings set` reach a running app within half a second, through the file polling. `import` asks for confirmation, and without a terminal it needs `--yes`.

## Verified

- `goatty-config`: 20 unit tests (lenient parsing per field, unknown keys kept across saves, versions, backups and pruning, themes, keybinding edits, profile round trip without private values).
- `goatty-shell`: generations (idempotence, numbering, settings-only changes, rollback, pruning, `zsh -n` rejection), `HostService` (preview/apply/rollback), launcher through `current`, and the real-zsh tests from Stage 3.
- `goatty-daemon/tests/settings.rs`: the protocol against a real goattyd with a sandboxed HOME (get, preview, apply, a bad value, generations, rollback, and `SetPalette` changing the actual OSC 11 answer read back from the PTY).
- Manually, release build, sandboxed HOME: the window renders every page. Editing `settings.json` from outside switched the terminal and the window to Catppuccin Latte, 17 px, bar cursor within the poll interval, and reported the bad value and the unknown key on the Profile page. `goattyd shell diff/status/generations/rollback` behaved as described.
- Themes and CLI (#50–#52): `theme_import` tests for every format; `ThemeLibrary` loading, shadowing, unique ids and resolution; dotted-key editing; `goatty-app/tests/cli.rs` and `goatty-daemon/tests/settings_cli.rs` run the real binaries in a sandboxed HOME (set/show/validate/unset, theme import/set/export/delete, profile export → import on a "second machine" including the host generation).
- Manually (release build, sandboxed HOME): a Windows Terminal scheme imported with `goatty theme import` showed up as "Campbell Teal" (`mine`, `dark`). With "Follow the system" on a dark desktop, the terminal used it, and the CLI-set cursor override showed as ● in the editor. `goatty theme set gruvbox-dark` from a shell restyled the running app. The startup switch from the default light appearance to the portal's dark one was the same code path as a live desktop toggle. Not done: toggling the real desktop setting (it would change the user's whole session).
- Reloading running shells (#56): `zsh_startup.rs::reloads_on_request` drives a managed zsh in a PTY (idle prompt reloads at once with the same pid, also when signaled while the prompt is drawn (`reloads_when_signaled_while_the_prompt_draws`); a running command reloads afterwards; typed text is left alone until Enter; a shell with jobs keeps its state and prints the reminder). `goatty-daemon/tests/settings.rs` checks that `ReloadShells` reaches a managed zsh session (which survives), skips `/bin/sh`, and forgets exited shells.
- Not verified yet: clicking through the controls (needs interactive testing, #49).

## Not done yet

Problems and env overrides shown on their row (#53), shortcut conflict warnings (#54), per-generation change summaries (#55), proper monospace detection (#57), keyboard navigation in the settings window (#58), and the GUI first-run/diagnostics screen (#40).
