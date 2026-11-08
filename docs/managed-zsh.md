# Managed zsh (Stage 3)

goatty sessions run zsh with a configuration goatty generates: autosuggestions, syntax highlighting, completion, Starship, Atuin, fzf key bindings and shell integration (OSC 133/7). Nothing in the user's dotfiles changes. Code: `crates/goatty-shell`. Startup files: `crates/goatty-shell/zsh/`.

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
           fzf → atuin → autosuggestions → starship (or fallback prompt) → plugins.zsh (plugin aliases + code) → integration
           ZDOTDIR = the user's again
           ~/.config/goatty/zshrc (the user's additions)
           syntax highlighting (last: it wraps every widget defined before it)
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
  zsh/settings.zsh              paths, feature list, history options, plugin search paths
  zsh/.zshenv .zprofile .zshrc  static, from crates/goatty-shell/zsh/
  zsh/integration.zsh           OSC 133 + OSC 7
  zsh/starship.toml             generated from the prompt settings; used unless you prefer your own starship.toml
  host.json, info.json          the settings it was built from, when
~/.local/share/goatty/current → generations/<n>   the active one
~/.local/share/goatty/tools/<tool>/<version>/   managed installs
~/.local/share/goatty/bin/<exe>                 → active managed binary (appended to the shell's PATH)
~/.local/share/goatty/plugins/<tool>            → active managed zsh plugin
~/.config/goatty/zshrc          the user's additions (goatty never writes it)
~/.cache/goatty/                zcompdump, completion cache, downloads
~/.local/state/goatty/shell-hint-dismissed      set by `goatty-setup --dismiss`
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
| `shell.zsh.{autosuggestions, syntax_highlighting, fzf, integration}` | on | |
| `prompt.starship`, `prompt.style` (`rich`/`minimal`), `prompt.two_line`, `prompt.newline_before`, `prompt.duration_threshold_ms` (2000; 0 = never), `prompt.prefer_user_config` | on, rich, on, off, 2000, on | |
| `history.size` (100 000), `history.ignore_duplicates`, `history.ignore_space`, `history.share_between_sessions`, `history.atuin`, `history.atuin_up_arrow` | 100 000, on, on, off, on, off | |

`.zshrc` is off by default because most `.zshrc` files load their own prompt and plugins, which would fight goatty's. With it on, it runs after compinit and before goatty's plugins, so goatty's prompt still wins. Turn off `prompt.starship` to keep your own prompt. The `minimal` prompt style uses plain characters only, for fonts without Nerd Font symbols (#46).

Each feature checks for its tool at shell startup and is skipped if the tool is missing. Binaries come from `$commands`, which sees the PATH after the user's `.zprofile` ran; zsh plugins come from a list of distro/Homebrew/NixOS paths, then the managed copy. A system install always wins over a managed one.

Defaults chosen here: emacs keymap; Up/Down search history by prefix; Home/End/Delete/Ctrl+arrows/Ctrl+Backspace/Shift+Tab bound to the sequences goatty sends; word motions stop at `/`; menu completion with case-insensitive matching; `auto_cd`; 100k history entries, `extended_history`, `inc_append_history`, no duplicates; Atuin on Ctrl+R with `--disable-up-arrow`; fzf on Ctrl+T/Alt+C (Atuin keeps Ctrl+R).

## Shell integration

`integration.zsh` emits OSC 133 `A` (prompt start) and `B` (input start) inside `PS1`, re-added on every prompt if a theme rebuilt it. It emits `C` from `preexec`, then `D;<status>` and OSC 7 (`file://host/percent-encoded/path`) from `precmd`. zsh restores `$?` for every hook, so hook order doesn't affect the status. rio-vt consumes OSC 133 silently and parses OSC 7. Surfacing these in the UI (command blocks, cwd titles) is #28.

## Installing tools

`goattyd shell install [TOOL…] [-y]` (or `goatty-setup` inside a goatty shell, which passes exactly the missing default tools and then reloads the shell). It shows the plan with sizes and asks first; without a terminal it requires `--yes`. Each tool:

1. Downloads the pinned release archive with `curl` (fallback `wget`). The sha256 pin is what makes an archive trusted, so goattyd needs no TLS stack of its own.
2. Checks the archive against the pinned sha256 in `tools.rs` before unpacking anything.
3. Unpacks into `tools/<id>/.<version>.partial-<pid>` (the `tar` crate refuses paths outside it), checks the expected binary or plugin entry, then renames it to `tools/<id>/<version>`.
4. Switches `bin/<exe>` or `plugins/<id>` by renaming a temp symlink over the old one, then removes older versions.

No sudo, nothing outside goatty's data directory. `goattyd shell uninstall TOOL…|--all` removes managed copies. For a remote goattyd this all happens on the remote host, which is also where `goatty-setup` runs.

| Tool                    | Version | Builds                                | Default |
|-------------------------|---------|---------------------------------------|---------|
| zsh-autosuggestions     | 0.7.1   | source tarball                        | yes     |
| zsh-syntax-highlighting | 0.8.0   | source tarball                        | yes     |
| Starship                | 1.27.0  | x86_64/aarch64 Linux musl             | yes     |
| Atuin                   | 18.24.2 | x86_64/aarch64 Linux musl             | yes     |
| fzf                     | 0.74.5  | x86_64/aarch64 Linux                  | no (integrated when present) |

Pinned 2026-11-07 to releases at least two weeks old. Checksums were computed from the downloads and matched upstream's published `.sha256`/checksums files where they exist (Starship, Atuin, fzf). zsh itself isn't installed: if it's missing, goatty starts `$SHELL` and `goattyd shell status` says so.

## Diagnostics

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
| + Atuin init (everything on)                         | 60–67 |

goatty's part is about 30 ms, half of it `starship init` (two process spawns). Caching the init scripts is #41. The first start after a zsh upgrade or on a fresh machine builds the completion dump (178 ms once in the real home dir; 47 ms after that, measured with `goattyd shell status`).

## Plugins

Installed plugins (Stage 5, [`plugins.md`](plugins.md)) render into `plugins-early.zsh`, `plugins.zsh` and `plugins/<id>/` of each generation, so previews, `zsh -n` validation and rollback cover them like everything else. `goatty-status` lists them as `plugin:<id>`.

## Not done yet

The GUI first-run prompt and a diagnostics screen (#40), caching tool init scripts (#41), installs for macOS and other architectures (#42), managed completions for installed tools (#43), importing existing history into Atuin on first install (#44), updating managed tools after a pin bump (#45), Nerd Font symbols for the default prompt (#46), an interactive GUI smoke test (#47), and the terminal side of shell integration (#28).
