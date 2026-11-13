# Managed zsh (Stage 3)

goatty sessions run zsh with a configuration goatty generates: autosuggestions, syntax highlighting, completion in fzf (fzf-tab), Starship, fzf's history and file search, and shell integration (OSC 133/7). Nothing in the user's dotfiles changes. Code: `crates/goatty-shell`. Startup files: `crates/goatty-shell/zsh/`.

The tools (fzf, fzf-tab, autosuggestions, syntax highlighting, Starship) are **built-in plugins** since #98: ordinary plugin manifests in `crates/goatty-shell/plugins/<id>/`, compiled into goatty. They get the same install plans, settings UI and load-order slots as registry plugins ([`plugins.md`](plugins.md#built-in-plugins)).

## How a session starts

goattyd (or `goatty --local`) asks `goatty_shell::Launcher` for the shell, using the host settings (`~/.config/goatty/host.json`, re-read for every session). If it's zsh and `shell.managed` is on (the default), the launcher makes sure the active *generation* matches those settings: if not, it builds, validates and activates a new one (see [`profiles.md`](profiles.md)). It then starts `zsh -l` with:

| Variable              | Value                                              |
|-----------------------|----------------------------------------------------|
| `ZDOTDIR`             | `~/.local/share/goatty/current/zsh`                |
| `GOATTY_ORIG_ZDOTDIR` | the user's `ZDOTDIR`, only if goattyd's environment had one |

zsh then reads the startup files from goatty's directory:

```
.zshenv    source settings.zsh (generated: paths + feature toggles)
           restore the user's ZDOTDIR, source the user's .zshenv
           interactive? → ZDOTDIR = goatty's dir again   (else: done, plain zsh)
.zprofile  (login) source the user's .zprofile with their ZDOTDIR, re-pin goatty's
.zshrc     options, history, PATH, plugins-early.zsh (plugin fpath + env), keys, compinit
           [user's .zshrc, if shell.zsh.user_rc]
           plugins-line-editor.zsh (fzf → fzf-tab → autosuggestions)
           plugins-prompt.zsh (starship), or goatty's fallback prompt when no prompt plugin loaded
           plugins.zsh (plugin aliases + code) → aliases.zsh (yours) → integration
           ZDOTDIR = the user's again
           ~/.config/goatty/zshrc (the user's additions)
           plugins-last.zsh (syntax highlighting: it wraps every widget defined before it)
           goatty-status / goatty-setup / goatty-reload, reload-on-request trap, missing-tools hint
.zlogin    the user's own, because ZDOTDIR is already theirs again
```

Rules that came out of testing against real zsh (`crates/goatty-shell/tests/zsh_startup.rs`):

- The user's files run at top level, never inside a function. Otherwise `typeset` in them would create locals.
- The user's files see their own `ZDOTDIR` (unset → `$HOME`). If their `.zshenv` sets `ZDOTDIR` (the common `~/.config/zsh` pattern), the later files come from there.
- A user `ZDOTDIR` that resolves to goatty's own directory is never sourced as "the user's". Without this guard, `.zprofile` sourced itself in a loop.
- `typeset name=(…)` must not be prefixed with `builtin`, because that disables typeset's array-assignment syntax.
- If `settings.zsh` fails to load, `.zshenv` restores the user's `ZDOTDIR` and returns: the session degrades to plain zsh instead of breaking.
- Non-interactive zsh started during startup (e.g. `zsh -c` from a user's `.zprofile`) gets the user's `ZDOTDIR` straight away and behaves like plain zsh.
- After startup, `ZDOTDIR` is exactly what the user had (set or unset), so nested shells, `exec zsh` and `.zlogout` behave like plain zsh. `goatty-reload` restarts the current shell with goatty's config. `SIGURG` (sent by `goattyd shell reload` or the settings window after an apply) makes the shell reload at its next idle prompt; see `docs/profiles.md`.
- Debian/Ubuntu's `/etc/zsh/zshrc` runs `compinit` unless `skip_global_compinit` is set; `.zshenv` sets it so compinit runs once, with goatty's dump file in `~/.cache/goatty/`.
- fzf's key bindings are only evaluated when zle has a terminal (`-o zle && -t 0`). Without one, fzf's option save/restore fails on `zle`.

## Files and directories

```
~/.config/goatty/host.json      the host settings (edit in the settings window)
~/.local/share/goatty/generations/<n>/   one built configuration each (newest 10 kept):
  zsh/settings.zsh              paths, feature list, history options, what goatty-setup installs
  zsh/plugins-*.zsh             the plugins, one file per load slot (see plugins.md)
  zsh/.zshenv .zprofile .zshrc  static, from crates/goatty-shell/zsh/
  zsh/integration.zsh           OSC 133 + OSC 7
  zsh/starship.toml             generated from the prompt settings; used unless you prefer your own starship.toml
  host.json, info.json          the settings it was built from; when, by which goatty, on which generation, what changed, its name
~/.local/share/goatty/current → generations/<n>   the active one
~/.local/share/goatty/tools/<package>/<version>/   managed installs (built-in and registry plugins' packages)
~/.local/share/goatty/bin/<exe>                 → active managed binary (appended to the shell's PATH)
~/.local/share/goatty/plugins/<package>         → active managed zsh plugin (a package with a `source` script)
~/.config/goatty/zshrc          the user's additions (goatty never writes it)
~/.cache/goatty/                zcompdump, completion cache, downloads
~/.local/state/goatty/shell-hint-dismissed      set by `goatty-setup --dismiss` or the GUI's "Don't ask again"
~/.local/state/goatty/shell-setup-offered       set once goatty's window offered the install (the shell hint stays quiet)
```

XDG variables are respected. History stays in the file plain zsh would use (`${ZDOTDIR:-$HOME}/.zsh_history`), so existing history keeps working.

## Features and settings

`goatty_config::HostSettings`, edited on the settings window's Shell, Prompt and History pages (or in `host.json`):

| Setting | Default | Env |
|---|---|---|
| `shell.program` | zsh if installed | `GOATTY_SHELL` |
| `shell.managed` | on | `GOATTY_SHELL_MANAGED=0/1` |
| `shell.zsh.user_env` (`.zshenv`, `.zprofile`) | on | |
| `shell.zsh.user_rc` (`.zshrc`) | off | `GOATTY_ZSH_USER_RC=0/1` |
| `shell.zsh.integration` | on | |
| `prompt.style` (`rich`/`lean`/`classic`/`rainbow`), `prompt.symbols` (`nerd`/`emoji`/`text`/`none`), `prompt.show` (`all`/`essential`) | rich, nerd, all | |
| `prompt.two_line`, `prompt.newline_before`, `prompt.transient`, `prompt.duration_threshold_ms` (0 = never) | on, off, off, 2000 | |
| `prompt.separator`, `prompt.heads`, `prompt.tails` (Classic/Rainbow), `prompt.frame`, `prompt.connection` (two lines) | angled, sharp, flat, none, none | |
| `prompt.directory.*`, `prompt.character.*`, `prompt.colors.*`, `prompt.modules.<module>.{enabled, side, symbol, color}` | see below | |
| `history.size` (100 000), `history.ignore_duplicates`, `history.ignore_space`, `history.share_between_sessions` | 100 000, on, on, off | |
| `plugins.<id>.enabled` for `fzf`, `fzf-tab`, `autosuggestions`, `syntax-highlighting`, `starship` | on | |
| `plugins.fzf.settings.{hidden_files, previews}` | on, on | |
| `plugins.fzf-tab.settings.previews` | on | |
| `plugins.autosuggestions.settings.{strategy, max_length}` | `history-completion`, 80 | |
| `plugins.syntax-highlighting.settings.brackets` | off | |
| `plugins.starship.settings.prefer_user_config` | off | |

Each built-in plugin has its own page under Plugins with all its settings; Prompt shows the Starship switches, History the fzf and fzf-tab ones. `host.json` version 1 had the switches as `shell.zsh.{autosuggestions, syntax_highlighting, fzf}`, `prompt.starship`, `history.atuin` and the options as `prompt.prefer_user_config`, `history.atuin_up_arrow`: loading one moves them to `plugins.*` (version 2; only keys the file had, so it stays sparse). Version 3 (#165) drops `plugins.atuin`: Atuin left the built-in plugins, and its entry would only be reported as a plugin that isn't installed.

`.zshrc` is off by default because most `.zshrc` files load their own prompt and plugins, which would fight goatty's. With it on, it runs after compinit and before goatty's plugins, so goatty's prompt still wins. Turn off the Starship plugin to keep your own prompt.

### The prompt

The `prompt` settings (`goatty_config::prompt`) cover about what `p10k configure` does, on top of Starship. The settings window's Prompt page edits them with a live preview.

- **Style**: `rich` is Starship's own look ("on main", "via 🦀 v1.85"); `lean` is colored text without words; `classic` puts every segment on one bar (`colors.segment`, 236 by default) with thin `separator`s between segments; `rainbow` gives each segment its own background (the module's color) and text in `colors.segment_text`. In `classic` and `rainbow`, `heads` (the end pointing toward the input) and `tails` shape the segment ends: flat, sharp, round, slanted, blurred.
- **Symbols** come from Starship's presets: `nerd` (`nerd-font-symbols`, drawn with goatty's bundled JetBrainsMono Nerd Font whatever the terminal font is), `emoji` (Starship's defaults), `text` (`plain-text-symbols`: any font), `none`.
- **Layout**: `two_line` puts the input on its own line; then `frame` (`left`: `╭─`/`╰─`, `full`: also `─╮`/`─╯`) and `connection` (`dotted`/`solid`, between the left side and what's on the right) apply. `transient` redraws an accepted command's prompt as just the prompt character (`.zshrc`, after integration so it keeps the OSC 133 marks; `transient_rprompt` drops the right prompt).
- **Modules**: every Starship module goatty knows (`goatty_config::prompt::MODULES`, from Starship 1.27's defaults and presets) can be switched (`show: essential` starts from directory, git and duration), put on the `right` (after a `$fill` on the first line with two lines, else `right_format`), and given its own `symbol` and `color` (text in lean/classic, background in rainbow; git's modules share the branch's segment). The directory always shows; the command duration's switch is `duration_threshold_ms`.
- **Directory**: `truncation_length` (4; 0 = whole path), `truncate_to_repo`, `abbreviate` (keep cut folders as their first letters, like fish), `truncation_symbol`, `home_symbol`, `read_only`, `repo_root` (bold), `substitutions`.
- **Character**: `success`/`error`/`vicmd` symbols ("" = `❯`/`❯`/`❮`, or `>`/`x`/`<` with text symbols) and `success_color`/`error_color`.

Colors are Starship colors: the theme's 16 by name (`bright-blue`) or number, 16–255, or `#rrggbb`. `PromptSettings::layout` describes what goes where (modules, ends, separators, frame, fill); `goatty-shell/src/starship.rs` writes it as Starship format strings and the preview (`goatty-app/src/ui/prompt_preview.rs`) draws it with sample values. Separators between Classic segments survive modules that don't show because each sits in a conditional group with its segment, on the side away from an always-shown anchor (the directory, or the OS icon). `host.json`'s `minimal` style (up to version 3) loads as `lean` with text symbols, essential modules and no branch symbol (version 4).

Each plugin checks for its tool at shell startup and is skipped if the tool is missing. Binaries come from `$commands`, which sees the PATH after the user's `.zprofile` ran; zsh plugins come from their package's `system_paths` (distro/Homebrew/NixOS), then the managed copy. A system install always wins over a managed one.

Defaults chosen here: emacs keymap; Up/Down search history by prefix; Home/End/Delete/Ctrl+arrows/Ctrl+Backspace/Shift+Tab bound to the sequences goatty sends; word motions stop at `/`; completion with case-insensitive matching, in fzf (fzf-tab; zsh's menu selection when fzf-tab is off or missing); `auto_cd`; 100k history entries, `extended_history`, `inc_append_history`, no duplicates; fzf on Ctrl+R/Ctrl+T/Alt+C.

## fzf: history, files, completion

fzf is goatty's one fuzzy finder (#165), split over two built-in plugins:

- **fzf** loads fzf's key bindings (`fzf --zsh`): **Ctrl+R** searches the zsh history (the same file plain zsh uses) and puts the pick on the command line, **Ctrl+T** inserts files, **Alt+C** changes directory. Files and directories come from `fd` (`fdfind` on Debian/Ubuntu: hidden ones included unless `hidden_files` is off, `.git` excluded, symlinks followed); without fd, fzf walks the directory itself with the same rules (`--walker`, `--walker-skip=.git`). With `previews` on, Ctrl+T previews the file with bat and Alt+C the directory with eza, each only when that tool is installed.
- **fzf-tab** (Aloxaf/fzf-tab 1.3.0) makes Tab open the matches in fzf: no group headers, `/` accepts a directory and completes inside it, zsh's own menu off. Its previews: directory contents (eza) and files (bat) for paths and `cd`, the full command line for `kill`/`ps`, the value for variables (`export`, `unset`, `$…`), `git diff` for `git add/diff/restore`, the last commit for `git log/checkout/switch/show`, `systemctl status`, `tldr` and `man` (at the preview's width). It needs fzf (`requires.commands`) and loads after fzf's bindings, before autosuggestions.

**Look.** fzf's options come from a file the fzf plugin generates (`plugins/fzf/fzfrc`, through `FZF_DEFAULT_OPTS_FILE`): reverse layout, 60 % height, and colors given as the terminal's 16 ANSI colors (`fg:-1,bg:-1,hl:2,…`), so fzf, its previews (bat with `--theme=ansi` unless `BAT_THEME` is set, eza's own ANSI colors) and fzf-tab follow goatty's theme, local or remote. Previews sit on the right at 60 %, below in panes narrower than 80 columns, wrap long lines, and Ctrl+/ cycles below/hidden/right. fzf's key bindings paste that file into `FZF_DEFAULT_OPTS`, which fzf splits like a shell: values with `|`, `<` or `(` are quoted (`zsh::tests::fzf_options_survive_the_key_bindings`).

**Yours win.** `FZF_DEFAULT_OPTS` is applied after goatty's file; `FZF_DEFAULT_OPTS_FILE`, `FZF_DEFAULT_COMMAND`, `FZF_CTRL_T_COMMAND`, `FZF_ALT_C_COMMAND`, `FZF_CTRL_T_OPTS` and `FZF_ALT_C_OPTS` set before goatty's plugins load (`.zshenv`, `.zprofile`) are kept, and `~/.config/goatty/zshrc` runs after them anyway (zstyles too). The values goatty exports are remembered in `_goatty_fzf_<NAME>`, so `goatty-reload`, whose new shell inherits them, replaces them instead of mistaking them for yours.

**Missing helpers.** fd, bat and eza are optional: each plugin records them in `goatty-status` as `helper:fd`, `helper:bat`, `helper:eza` (ok with the path, or `missing`; bat and eza only while previews are on). Diagnostics shows them as optional rows that don't count as missing tools, and neither `goatty-setup` nor the first-run setup installs them; the registry's fd, bat and eza plugins do.

**Atuin** was a built-in plugin until #165. It's now a registry plugin (`goattyd plugin install atuin`) that loads after fzf and takes Ctrl+R, optionally Up. A download goatty made for the old built-in (`tools/atuin/`, `bin/atuin`) is reused by that plugin at the same version, or removed with `goattyd shell uninstall atuin` (or `--all`).

## Shell integration

`integration.zsh` emits OSC 133 `A` (prompt start) and `B` (input start) inside `PS1`, re-added on every prompt if a theme rebuilt it. It emits `C` from `preexec`, then `D;<status>` and OSC 7 (`file://host/percent-encoded/path`) from `precmd`. zsh restores `$?` for every hook, so hook order doesn't affect the status. rio-vt consumes OSC 133 silently and parses OSC 7. Surfacing these in the UI (command blocks, cwd titles) is #28.

## Installing tools

`goattyd shell install [NAME…] [-y]` (or `goatty-setup` inside a goatty shell, which passes exactly the missing tools of the built-in plugins that are on and marked for default install, then reloads the shell) installs the packages of built-in plugins. It plans them like any plugin install (`Plugins::plan` on the built-in candidate: a package already on the system or already installed by goatty at the pinned version downloads nothing), shows the downloads with sizes and asks first; without a terminal it requires `--yes`. `goattyd plugin install starship` does the same with the full plan (permissions, shell code). Each package:

1. Downloads the pinned release archive with `curl` (fallback `wget`). The sha256 pin is what makes an archive trusted, so goattyd needs no TLS stack of its own.
2. Checks the archive against the sha256 in the plugin's manifest before unpacking anything.
3. Unpacks into `tools/<id>/.<version>.partial-<pid>` (the `tar` crate refuses paths outside it), checks the expected binaries or `source` script, then renames it to `tools/<id>/<version>`.
4. Switches `bin/<exe>` or `plugins/<id>` by renaming a temp symlink over the old one, then removes older versions.

No sudo, nothing outside goatty's data directory. `goattyd shell uninstall NAME…|--all` removes goatty's downloads (the plugins stay on and use system copies if there are any). For a remote goattyd this all happens on the remote host, which is also where `goatty-setup` runs.

| Built-in plugin     | Package (id)                               | Version | Builds                    | Load slot   | Installed by goatty-setup |
|---------------------|--------------------------------------------|---------|---------------------------|-------------|---------------------------|
| fzf                 | fzf (`fzf`)                                | 0.74.5  | x86_64/aarch64 Linux      | line-editor | yes |
| fzf-tab             | fzf-tab (`fzf-tab`)                        | 1.3.0   | source tarball            | line-editor (after fzf) | yes |
| autosuggestions     | zsh-autosuggestions (`autosuggestions`)    | 0.7.1   | source tarball            | line-editor (after fzf, fzf-tab) | yes |
| syntax-highlighting | zsh-syntax-highlighting (`syntax-highlighting`) | 0.8.0 | source tarball          | last        | yes |
| starship            | Starship (`starship`)                      | 1.27.0  | x86_64/aarch64 Linux musl | prompt      | yes |

Pinned 2026-11-07 to releases at least two weeks old (fzf-tab 2026-11-13: v1.3.0 is from 2026-03-16). Checksums were computed from the downloads and matched upstream's published `.sha256`/checksums files where they exist (Starship, fzf); GitHub's source tarballs (fzf-tab, the zsh plugins) have none, so their checksum was taken from two separate downloads. fzf-tab's system copies: Arch's AUR `fzf-tab-git`, NixOS/Nix `zsh-fzf-tab`. The package ids are the tool ids from before #98, so managed installs made then (`tools/<id>/`, `bin/` and `plugins/` links) are used as they are. zsh itself isn't installed: if it's missing, goatty starts `$SHELL` and `goattyd shell status` says so.

## First run and the GUI

On the very first start (no `settings.json`, no `~/.local/state/goatty/welcome-shown`), goatty opens a **welcome window** with three steps: a theme (or follow the desktop) and the text size; the built-in tools of the window's host, the missing ones preselected and installed one at a time with progress, plus the prompt style; then the shortcuts worth knowing. Everything else keeps its defaults. The command palette's "Welcome and setup" opens it again.

When a tab connects to a host (local or remote) whose managed shell misses default tools, a sticky toast offers to install them: Install / Not now / Don't ask again. That happens once per host per run, never while the welcome window is open, and never after "Don't ask again" (the same marker as `goatty-setup --dismiss`). The in-shell hint stays quiet in sessions of a goatty window that offers the setup itself (goattyd sets `GOATTY_GUI_SETUP=1` for clients with protocol v9, `goatty --local` always; the shell unsets it after startup), and on hosts where a window already offered it (`shell-setup-offered`). Installing reloads the host's running shells, so an idle prompt has the new tools at once.

The GUI talks to the host through `goatty_shell::setup` (protocol v9 `Shell`, JSON like the plugin operations; with `--local` in-process): `status`, `install` (built-in tool ids), `offered`, `dismiss`. A goattyd older than v9 gets no offer and the Diagnostics page says to restart it.

## Diagnostics

Settings → Diagnostics (or `goatty --settings=diagnostics`, or "Shell diagnostics" in the palette) shows the same report as `goattyd shell status` for the window's host: the program and configuration, the startup time, each feature with where it came from (system path or goatty's copy and its version), what the startup printed to stderr, the user's own startup files, and an Install button for missing tools. "Copy report" puts it on the clipboard for a bug report.

`goattyd shell status` prints the configuration, then starts the managed shell the way a session would (login + interactive, no TTY), runs `goatty-status`, and lists each feature with where it came from and the startup time. Anything the startup printed to stderr is shown. Inside a shell, `goatty-status` prints the same feature table.

## Startup cost

Measured 2026-11-07 on the dev machine (CachyOS, zsh 5.9.2), `zsh -l -i -c exit` with no TTY, median of 21 runs, sandboxed `$HOME`, completion dump already built:

| Configuration                                        | ms   |
|------------------------------------------------------|------|
| `zsh -f -i` (no startup files at all)                | 2    |
| plain login shell (system files only)                | 32   |
| goatty base: options, keys, compinit, integration    | 37   |
| + autosuggestions                                    | 42   |
| + syntax highlighting                                | 44   |
| + Starship init                                      | 59   |
| + Atuin init (everything on, Atuin built in then)    | 60–67 |

Moving the tools to built-in plugins (#98) didn't change it: 61 ms median on both `main` and the branch (2026-11-08, same machine, sandboxed `$HOME` with the host's Starship, fzf and distro zsh plugins, Atuin missing; `goattyd shell status` built the generation, then 21 runs of `zsh -l -i -c exit` each, two rounds).

fzf and fzf-tab by default (#165), measured 2026-11-13 on the same machine: sandboxed `$HOME` with the host's fzf 0.74.5, fd, bat, eza, Starship and distro zsh plugins, goatty's fzf-tab 1.3.0, median of 21 runs of `zsh -l -i -c exit`, two rounds. Without a TTY fzf's key bindings aren't loaded, so the TTY column (through `script`, which adds its own ~10 ms) is the one that counts:

| Configuration                        | no TTY ms | TTY ms |
|--------------------------------------|-----------|--------|
| fzf and fzf-tab off                  | 59.5–59.7 | 72.2–72.5 |
| + fzf (`fzf --zsh` and its eval, fd/bat/eza lookups) | 58.7–59.7 | 77.1–80.1 |
| + fzf-tab (sourcing it, zstyles)     | 62.5–64.4 | 83.6–84.3 |

So fzf costs about 6 ms (one process spawn for `fzf --zsh`) and fzf-tab about 5 ms. Caching `fzf --zsh` belongs to #41.

goatty's part is about 30 ms, half of it `starship init` (two process spawns). Caching the init scripts is #41. The first start after a zsh upgrade or on a fresh machine builds the completion dump (178 ms once in the real home dir; 47 ms after that, measured with `goattyd shell status`).

## Plugins

Installed plugins (Stage 5, [`plugins.md`](plugins.md)) render into `plugins-early.zsh`, `plugins.zsh` and `plugins/<id>/` of each generation, so previews, `zsh -n` validation and rollback cover them like everything else. `goatty-status` lists them as `plugin:<id>`.

## Not done yet

Caching tool init scripts (#41, including `fzf --zsh`), installs for macOS and other architectures (#42), managed completions for installed tools (#43), updating managed tools after a pin bump (#45), an interactive GUI smoke test (#47), and the terminal side of shell integration (#28).
