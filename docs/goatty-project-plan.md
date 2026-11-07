# goatty — Project Plan and Technical Blueprint

> **A modern, approachable terminal environment built in Rust.**  
> GPU-accelerated terminal emulation, a managed Zsh setup, configurable workspaces, and an open plugin ecosystem — without requiring users to learn shell configuration first.

**Document status:** Initial architecture proposal  
**Prepared:** 7 November 2026  
**Project name:** goatty

---

## 1. Product vision

goatty should make the terminal feel like a polished, configurable desktop application rather than a blank prompt that users must assemble themselves.

A new user should be able to install goatty, open it, and immediately have:

- A useful, attractive terminal with sensible defaults.
- A ready-to-use Zsh environment.
- Autosuggestions, syntax highlighting, completion and searchable history.
- A configurable prompt and terminal theme.
- Tabs, resizable split panes and restorable workspaces.
- Convenient keyboard shortcuts, including familiar copy and paste behavior.
- A GUI for installing and configuring tools and shell integrations.

Experienced users should still get a normal shell. goatty must not invent a proprietary command language, lock users into a custom shell, or prevent them from using their existing tools and dotfiles.

The defining idea is **a complete terminal environment, not just another terminal emulator**.

### Product principles

1. **Works out of the box.** The default setup should be useful without reading documentation or editing dotfiles.
2. **GUI first, not GUI only.** Settings are discoverable in the GUI, while the underlying configuration remains inspectable and manually overridable.
3. **Use existing tools.** Integrate mature shells and utilities rather than reimplementing them without a strong reason.
4. **Keep the host safe.** Do not silently replace the user's login shell, rewrite their dotfiles, modify global PATH, or install privileged services.
5. **Reproducible and reversible.** Profiles, plugin versions and generated configuration should be reviewable, backed up and rollback-capable.
6. **Open ecosystem.** Third parties should be able to publish integrations and expose settings without changing goatty's source code.
7. **Separate the components.** The GUI, terminal engine, process/session manager, shell environment and plugin system communicate through explicit interfaces.

---

## 2. Recommended stack

| Area | Recommendation | Role |
|---|---|---|
| Language | Rust | Core application, services, process management and plugin runtime |
| Desktop UI | **GPUI** | Native, GPU-accelerated desktop interface |
| Terminal VT core | **Rio-VT (`rio-vt`)** | ANSI/VT parsing, terminal grid, scrollback, selection and search state |
| Terminal rendering | goatty-owned renderer integrated with GPUI | Render the terminal grid efficiently; do not render each cell as a separate UI component |
| PTY abstraction | Start by evaluating **`portable-pty`**; retain a platform-specific abstraction | Create and manage pseudoterminals across operating systems |
| Default shell | **Zsh** | Real, familiar interactive shell |
| Prompt | **Starship**, generated from goatty's prompt settings | Cross-shell prompt rendering and customization |
| History | **Atuin** as the initial rich history backend; `fzf` as an optional general fuzzy finder | Searchable command history and fuzzy selection |
| Shell enhancements | `zsh-autosuggestions`, `zsh-syntax-highlighting`, Zsh completion system | Suggestions, highlighting and completion |
| Profile model | Versioned JSON with typed Rust structures | Single source of truth for user configuration |
| Plugin manifests | Declarative YAML or TOML, with JSON Schema for settings | Describe integrations and generate GUI settings |
| Managed tool storage | User-level goatty data directory with versioned installs and shims | Install tools without taking ownership of the host system |
| Background process | Rust `goattyd` service/daemon | Own PTYs, sessions, workspaces and environment state independently of the GUI |
| Remote configuration | Authenticated API with a later web client | Manage a headless installation without requiring the desktop renderer |

### Why GPUI

GPUI is a Rust-native, GPU-accelerated UI framework designed for applications such as Zed. It supports both higher-level views and lower-level custom elements, which is important for implementing a terminal surface efficiently.

It is a good fit for goatty's native feel and Rust-first architecture. However, GPUI is still pre-1.0 and actively developed, so APIs can change. Treat the framework choice as a decision to validate with an early prototype, not as a reason to skip a rendering spike.

### Why Rio-VT

Rio extracted its terminal core into `rio-vt`, a Rust library that provides the VT state machine, ANSI/escape-sequence parser, terminal grid, scrollback, selection, search and terminal-facing event model. It does **not** require goatty to use Rio's own window system, GPU renderer or UI.

That separation is exactly what goatty needs: reuse a real terminal implementation while owning the desktop UI, terminal rendering, tabs, panes and session management.

Keep Rio-VT behind a small goatty-owned `TerminalEngine` interface. Its API is still evolving, so pin a tested version and keep the option to evaluate `wezterm-term` as a fallback. `wezterm-term` is also an embeddable Rust terminal model, but lives inside the larger WezTerm repository.

### Important distinction: terminal core versus renderer

Rio-VT is **not** a ready-made GPUI terminal widget. goatty still needs to implement the rendering and interaction layer:

- Font shaping, glyph caching and efficient cell drawing.
- Cell backgrounds, foregrounds, attributes and cursor rendering.
- Selection, mouse interaction and scrolling.
- Resize/reflow behavior and damage-driven updates.
- Clipboard integration, hyperlinks and supported graphics protocols.

Use a custom GPUI element or equivalent low-level rendering path. Avoid creating thousands of ordinary UI elements for terminal cells. Batch drawing and update only the regions that changed where practical.

### Why not Tauri as the primary desktop framework?

Tauri is viable and could render a capable terminal using a WebView and WebGL. But goatty's core goal is a native Rust desktop experience with a custom high-performance terminal surface. GPUI keeps the desktop UI in the Rust application.

A browser-based settings client can still be added later as a separate client of the daemon API. Choosing GPUI does not prevent headless or web-based configuration.

---

## 3. High-level architecture

The desktop UI should not own the lifetime of shell processes. Split goatty into a desktop client and a background core process.

```text
                 ┌────────────────────────────┐
                 │       goatty desktop       │
                 │                            │
                 │  GPUI · tabs · panes       │
                 │  terminal renderer         │
                 │  settings · command palette│
                 └─────────────┬──────────────┘
                               │ local IPC
                               │ authenticated API remotely, later
                 ┌─────────────▼──────────────┐
                 │          goattyd           │
                 │                            │
                 │ sessions and workspaces    │
                 │ PTY/process ownership      │
                 │ profiles and config       │
                 │ plugin and package manager│
                 │ shell integration          │
                 └─────────────┬──────────────┘
                               │
                 ┌─────────────▼──────────────┐
                 │      shell / processes     │
                 │                            │
                 │ Zsh · SSH · editors · CLI  │
                 └────────────────────────────┘
```

> **Update (7 Nov 2026):** goatty is remote-capable from the start. The desktop app is always a client of a goattyd, which can run on this machine or on another host reached over SSH (including Tailscale) or any stdio command. See [`remote-architecture.md`](remote-architecture.md) for the decisions, protocol, flow control and the R1–R3 roadmap.

### Desktop client

The desktop application is responsible for the visible experience:

- Window chrome, top bar and tabs.
- Split-pane layout and resizing.
- Terminal rendering and user input.
- Selection, clipboard and context menus.
- Settings screens and command palette.
- Workspace and session selection.

It connects to `goattyd` for process output, input, resizing, session state and configuration operations.

### Background core (`goattyd`)

The daemon owns the long-lived state:

- PTY creation and process lifecycle.
- Running shell sessions and their working directories.
- Workspace and pane metadata.
- Profile activation and generated configuration.
- Plugin installation and tool versions.
- Local IPC/API for desktop and CLI clients.

Run it as a user-level process/service rather than a privileged system daemon. Use the operating system's normal user service mechanism where appropriate (for example, systemd user services on Linux and launchd on macOS). On Windows, use a suitable user-session process model and Windows pseudoconsole support.

### Session persistence: define it precisely

A daemon-owned PTY can keep a shell and its foreground process alive when the desktop window closes, as long as the daemon remains running. Reopening goatty can reconnect to that process and restore its view.

A normal process cannot survive a machine reboot. After reboot, goatty can restore the workspace layout and optionally recreate shells in their previous directories, but it cannot resume an arbitrary process at the exact instruction where it stopped. Do not promise reboot-proof process persistence.

Persist layout and metadata separately from the live terminal process. Do not treat a serialized screen snapshot as a substitute for a live PTY.

---

## 4. Shell strategy: Zsh first

Zsh should be goatty's default shell. It offers a powerful interactive environment, a mature completion system and a broad ecosystem that is familiar to many developers. It is not fully Bash- or POSIX-compatible, so goatty should explain that it is a shell, not a universal command interpreter.

### Default Zsh experience

Ship a curated, managed configuration with:

| Capability | Initial implementation |
|---|---|
| Interactive shell | Zsh |
| Completion | Zsh's programmable completion system (`compinit`) plus installed completion definitions |
| Autosuggestions | `zsh-autosuggestions` |
| Syntax highlighting | `zsh-syntax-highlighting` |
| Prompt | Starship, configured by goatty |
| Searchable history | Atuin, configured for Zsh and bound to `Ctrl+R` |
| Fuzzy selection | `fzf`, optional but supported |
| Shell lifecycle hooks | Zsh hooks such as `precmd` and `preexec`, installed carefully |
| Terminal semantic integration | OSC 133 and OSC 7 shell integration |

The load order matters. In particular, syntax highlighting should be loaded after plugins that modify Zsh's line editor. Atuin and other line-editor integrations also need to be tested together so keybindings and hooks do not override one another unexpectedly.

### Shell support policy

Separate **shell launching** from **first-class shell integration**:

- Launching: allow users to launch a compatible shell executable even when goatty has no dedicated adapter for it.
- First-class integration: provide tested startup handling, prompt semantics, working-directory reporting, completion setup and plugin adapters.

Recommended progression:

1. Zsh — default and fully integrated first.
2. Bash — compatibility and a first-class adapter after the initial release.
3. Fish — supported alternative, with its own adapter.
4. PowerShell — later, with Windows-specific testing.

Do not create separate plugin ecosystems per shell. A plugin should declare its capabilities and provide shell-specific integration files only where necessary. The goatty shell adapter chooses the right implementation.

### Avoid taking over the user's dotfiles

The default goatty environment should be app-managed and isolated from the user's normal login shell configuration.

For Zsh, investigate launching with a goatty-controlled `ZDOTDIR` and generated startup files. Those files can initialize goatty's managed configuration and, if the user opts in, chain to the user's original Zsh startup files. This must be designed carefully to avoid recursion, duplicate plugin loading or surprising changes to shell startup order.

Never silently overwrite `~/.zshrc`, `~/.zshenv`, `~/.zprofile`, `~/.zlogin`, `~/.bashrc` or equivalent files. Provide an explicit, inspectable and reversible external-integration option for users who want goatty-managed tools or shell features in VS Code and other terminals.

A CLI command such as `goatty env --shell zsh` can print environment initialization for users who want to integrate it manually.

---

## 5. Shell integration and command awareness

A terminal emulator normally sees a stream of bytes. It does not automatically know where a prompt ends, where a command begins, what directory the shell is in, or whether a command succeeded.

Add a small, auditable shell-integration layer for Zsh. It should emit standard terminal escape sequences, especially:

- **OSC 133** — semantic prompt and command markers: prompt start, command input start, command execution and command completion/exit status.
- **OSC 7** — current working-directory reporting.

The terminal parses these markers and associates them with the correct pane/session. This enables:

- Command boundaries and command blocks.
- Exit-status and duration indicators.
- Previous/next command navigation.
- Selecting or copying an entire command's output.
- Working-directory-aware tab titles.
- Better session restoration and workspace context.

Zsh does not emit all of this integration automatically in the same way as some other shells; goatty should provide a small Zsh integration script using the appropriate hooks. Keep it readable, versioned and easy to disable. Do not depend on parsing prompt text heuristically when semantic integration is available.

---

## 6. Profile and configuration model

The profile is the single source of truth for goatty-managed settings. The GUI should edit this model, not edit shell configuration files directly.

The model should cover at least:

- Shell choice and shell startup behavior.
- Theme, color palette, fonts, font size, cursor and spacing.
- Prompt layout and enabled prompt modules.
- Autosuggestions, highlighting, completion and history behavior.
- Installed and enabled plugins.
- Keyboard shortcuts and clipboard behavior.
- Workspace and tab preferences.
- Optional external shell integration.

Use a versioned JSON representation with typed Rust structures. Add schema validation and migrations so profiles remain usable as goatty evolves.

### Configuration compilation and activation

Treat configuration as a build-and-activate operation:

1. Read the active profile and installed plugin manifests.
2. Resolve dependencies and platform/shell adapters.
3. Generate shell and prompt configuration into goatty-owned locations.
4. Validate generated files where possible.
5. Show a diff or summary of changes.
6. Activate the new generation atomically.
7. Retain the previous generation for rollback.

Keep generated files separate from user-authored overrides. Make it clear which files goatty owns. Do not silently merge arbitrary user dotfiles into generated output.

Support profile export/import so users can share a setup without sharing credentials, private paths, command history or other personal data.

---

## 7. Plugin registry and extension model

The plugin ecosystem is a central product feature. It should not be a traditional plugin manager that merely sources shell scripts. A plugin describes a capability and the resources needed to provide it; goatty decides how to install, configure and expose it.

### Plugin categories

1. **Environment plugins** — install command-line tools, shell completions, aliases, functions and environment variables.
2. **Prompt/appearance plugins** — provide prompt modules, colors, themes, symbols and visual indicators.
3. **Application plugins** — add goatty commands, context actions, tab indicators, workspace templates or other UI-level features.

These categories can share a manifest and settings schema, but must have different permission levels.

### Declarative plugin manifest

Each plugin should declare:

- Stable identifier, name, version, description and publisher.
- Supported goatty versions, operating systems and shells.
- Dependencies and compatibility requirements.
- Tool installation sources, versions and checksums.
- Shell-specific integration files (Zsh first; other adapters where available).
- Prompt modules and their supported settings.
- GUI settings schema, ideally JSON Schema-backed.
- Optional command-palette actions and keybinding contributions.
- Required permissions and a clear explanation of why they are needed.

Settings should be schema-driven. For common settings such as booleans, strings, numbers, enums, paths and repeated values, goatty should render suitable controls automatically. Plugin authors should not need to implement a custom GUI for ordinary settings.

### Plugin execution and security

Do not load arbitrary native Rust libraries into the desktop application as the initial extension model. Native plugins create ABI, compatibility and security problems.

Start with declarative manifests and known integration files. Later, evaluate WebAssembly/WASI for application-level extensions with a narrow, explicit host API.

Shell integrations and installed executables are powerful: they can access the user's files, environment and credentials. Treat them as executable code, not harmless configuration. Provide:

- Explicit approval before installing or enabling executable integrations.
- Signed or otherwise verifiable registry metadata and package checksums.
- Version pinning and compatibility checks.
- Publisher/source information and readable installation plans.
- Per-plugin permissions and clear warnings.
- Updates that can be reviewed and rolled back.
- A complete uninstall path.

A signed registry does not make a malicious plugin safe; it establishes provenance and integrity. Trust and permission decisions still matter.

### Registry distribution

Start with a Git-backed registry containing a signed/versioned index and versioned plugin manifests. This is simple for contributors and easy to review. Design the client so additional community, private or organization registries can be added later.

---

## 8. Tool installation and host integration

Use an **application-managed user-space environment by default**, with optional integration into the host shell.

The goal is to let goatty manage versions and configuration without silently taking control of the operating system.

### Managed tools

Store managed tools and plugin assets in a goatty-owned user data directory. Use versioned installations and a stable `bin` or shim directory that points to the selected versions.

When a new goatty shell starts, construct its environment by adding the managed binary directory to that process's `PATH`. Do not modify the global system PATH by default.

### Installation resolution

For each required tool:

1. Detect whether a compatible version is already installed.
2. Reuse the existing tool when the user permits it and it meets the requirement.
3. Otherwise offer a managed installation.
4. Use a system package manager only when appropriate and after clearly explaining any system-level action.
5. Never request administrator privileges silently.

Some tools are simple standalone binaries; others require services, drivers, libraries, runtimes or system permissions. The plugin model must distinguish these cases. For example, installing a Docker CLI is not the same operation as installing and enabling a Docker daemon.

### External integration

Offer a separate opt-in feature to make goatty-managed tools available outside goatty. Show exactly which startup file or environment mechanism will be changed, create a backup, use a clearly marked and reversible integration block where appropriate, and offer removal.

The preferred integration should be a small source/include of a goatty-owned environment file, not a copy of generated configuration into the user's dotfiles.

Nix/Home Manager can be an optional export or advanced backend later. It should not be required for the default experience.

---

## 9. Terminal UX and interaction model

The visible application should feel like a modern desktop tool, with the terminal remaining the primary workspace.

### Main interface

- Top bar with tabs, workspace selection and new-tab/split actions.
- Resizable horizontal and vertical panes.
- Optional settings sidebar or dedicated settings window.
- Command palette for discoverability.
- Clear session status and working-directory information.
- Theme, font, padding and cursor customization.

Keep the initial interface calm and approachable. Advanced functionality should be discoverable rather than permanently occupying the screen.

### Keyboard and clipboard behavior

Use platform conventions, with an explicit preference for users who want terminal-style shortcuts.

Recommended starting behavior on Windows/Linux:

| Shortcut | Behavior |
|---|---|
| `Ctrl+C` | Copy when text is selected; otherwise send interrupt to the foreground process |
| `Ctrl+V` | Paste clipboard contents |
| `Ctrl+Shift+C` | Always copy |
| `Ctrl+Shift+V` | Always paste |
| `Ctrl+R` | Open searchable command history |
| `Ctrl+Shift+P` | Open command palette |
| `Ctrl+Shift+T` | New tab |

Use `Cmd` equivalents for the conventional copy/paste shortcuts on macOS, with a discoverable keymap. Make shortcuts configurable.

Additional behavior:

- Optional copy-on-select, disabled by default to avoid unexpected clipboard replacement.
- Safe multiline paste preview/confirmation, especially for suspicious or large pasted command blocks.
- Word, line, path and URL selection behavior.
- Searchable scrollback.
- Clickable URLs and file paths with configurable handling.
- Copy command, copy output, rerun command and open working directory actions when command boundaries are known.

The fundamental terminal behavior must remain intact: when there is no selection, `Ctrl+C` must send the interrupt character to the foreground process. In full-screen applications, key handling must not steal input unexpectedly.

---

## 10. Headless and remote configuration

The daemon should expose a local API that is independent of GPUI. The desktop client, CLI and any future web client should call the same core operations.

A future web interface can provide:

- Profile and plugin configuration.
- Tool inventory and update management.
- Session/workspace status.
- Remote diagnostics.
- Headless server setup.

Do not expose a configuration or process-control API publicly without authentication. Bind to loopback by default. For remote administration, prefer SSH tunneling initially; add explicit authentication, authorization and transport security before supporting direct network access.

The web UI is a later client, not a reason to make the terminal itself a WebView or to choose Tauri as the core architecture.

---

## 11. Development roadmap

Build in stages. The first technical milestone is terminal correctness, not the plugin marketplace.

### Stage 0 — Validate the foundation

Build a small GPUI prototype that embeds Rio-VT, launches Zsh through a PTY and renders a working terminal.

Compare Rio-VT with `wezterm-term` behind the same small internal interface. Validate:

- ANSI colors and text attributes.
- Unicode, combining characters, wide characters and Nerd Font glyphs.
- Resize and reflow.
- Selection, clipboard and scrollback.
- Full-screen TUIs such as Neovim and `htop`.
- tmux running inside goatty.
- Large and rapid terminal output.
- Mouse reporting and alternate screen behavior.
- Supported image protocols where practical.

**Exit criterion:** goatty can run real interactive terminal applications reliably and feels responsive. If GPUI's custom rendering path proves too difficult or slow, revisit that boundary before building more features.

### Stage 1 — Usable native terminal

Implement the desktop window, terminal rendering, PTY input/output, resize, selection, clipboard, scrollback, theme/font settings and Zsh launch. Add a basic tab and split model if it does not compromise terminal correctness.

### Stage 2 — Daemon and sessions

Move PTY ownership into `goattyd`. Add reconnection after closing the GUI, persistent workspace layouts, tab/pane restoration, named workspaces and session management. Define how crashes and unexpected daemon termination are handled.

### Stage 3 — Default Zsh environment

Add the managed Zsh startup configuration, shell integration, completion, autosuggestions, syntax highlighting, Starship and Atuin. Make the initial setup one-click and provide a diagnostic screen for missing tools or failed integrations.

> **Update (7 Nov 2026):** the managed zsh, shell integration (shell side), pinned user-space tool installs and CLI diagnostics (`goattyd shell status|install`, `goatty-setup` in the shell) are in `crates/goatty-shell`. See [`managed-zsh.md`](managed-zsh.md). The GUI first-run prompt and diagnostics screen are still open (see the Stage 3 milestone).

### Stage 4 — Profile engine and settings GUI

Make the profile the source of truth. Add configuration compilation, validation, previews, diff display, atomic activation, backup/rollback and profile import/export. Implement the first settings pages for appearance, shell, prompt, history, shortcuts and workspaces.

> **Update (7 Nov 2026):** done in its first form, see [`profiles.md`](profiles.md). The profile is split into client and host documents. Host settings compile into validated, atomically activated generations with rollback. Diffs preview changes before applying. Settings files are saved with backups. Profiles import and export. A settings window covers appearance, shell, prompt, history, shortcuts, workspace and profile. Follow-ups are in the Stage 4 milestone.

### Stage 5 — Plugin system

Define and document the plugin manifest, settings schema, package model, shell adapter interface and permission model. Publish a small built-in registry: Git, fzf, Atuin, zoxide, Docker CLI integration, Python and Node.js. Add community registry support only after the first-party flow is reliable.

### Stage 6 — Shell and platform expansion

Add Bash and Fish adapters, followed by PowerShell. Add optional host integration and documented VS Code setup. Validate PTY, shell startup and rendering behavior independently on every supported OS.

### Remote track (parallel to Stages 1–4)

R0 (protocol v2, SSH/stdio transports, flow control, reconnect) is done. R1 (host switcher, remote install and version matching, askpass, adaptive window, lazy history) follows once tabs exist. See [`remote-architecture.md`](remote-architecture.md) §8.

### Stage 7 — Remote and advanced ecosystem

Add the authenticated remote API, web configuration client, CLI profile management, private registries and a constrained extension runtime such as WASM/WASI if real plugin use cases justify it.

---

## 12. Explicit non-goals for the first release

Do not attempt to build all of these at the same time:

- A new shell or shell language.
- A replacement for Zsh, Bash or Fish.
- A custom prompt engine before Starship proves insufficient.
- A custom history engine before Atuin proves insufficient.
- A full package manager for every operating system.
- A Nix replacement or mandatory Nix installation.
- Arbitrary native application plugins.
- A remote terminal server exposed to the public internet.
- Full feature parity across every shell and operating system.
- AI/agent functionality; it is not part of the product's core identity.

The goal is to make existing tools work together coherently, not to rebuild the entire command-line ecosystem.

---

## 13. Main technical risks and mitigations

| Risk | Why it matters | Mitigation |
|---|---|---|
| GPUI is pre-1.0 | APIs may change and platform support may evolve | Pin versions, isolate UI code, keep the first prototype small |
| Rio-VT API evolves | Terminal-core updates could break integration | Wrap it behind goatty's own interface and pin tested versions |
| Terminal rendering complexity | Unicode, shaping, reflow, graphics and high-volume output are difficult | Build a renderer spike and compatibility suite before adding product features |
| PTY differences by OS | Unix PTYs and Windows pseudoconsole behavior differ | Use a platform abstraction and test each OS early |
| Zsh startup complexity | Startup files, plugin order and user customization can conflict | Use an app-owned startup directory, explicit chaining and diagnostics |
| Plugin security | Shell scripts and binaries can execute with user privileges | Declarative manifests, explicit permissions, checksums, review and rollback |
| Configuration drift | Users may manually edit generated files | Separate generated config from overrides; show diffs and preserve ownership boundaries |
| Background process lifecycle | Closing the UI, crashing the daemon and rebooting are different events | Document lifecycle semantics and test reconnect/recovery behavior |
| Cross-platform scope | A polished terminal requires substantial OS-specific work | Choose one primary OS for the first MVP; expand only after the core is stable |

---

## 14. First concrete tasks

The first implementation sprint should be narrowly focused:

1. Create a minimal Rust + GPUI desktop application.
2. Add Rio-VT and write a thin adapter around its terminal state and input/output APIs.
3. Create a PTY using `portable-pty` as the first candidate; verify platform behavior before committing to it.
4. Launch the system Zsh in an interactive PTY.
5. Render the terminal grid in a custom GPUI element.
6. Implement keyboard input, resize, scrolling and basic selection/copy/paste.
7. Test Neovim, tmux, `htop`, Unicode, alternate-screen programs and high-volume output.
8. Record performance and correctness issues; compare the same prototype with `wezterm-term` if Rio-VT presents a blocking problem.

Do not start with registry accounts, plugin marketplace UI, a web frontend or a large settings system. First prove that goatty can be a reliable terminal.

---

## 15. Reference projects and documentation

These are the primary sources to consult while implementing. Pin actual crate versions and re-check APIs before adopting them; the ecosystem is actively evolving.

### Core UI and terminal

- [GPUI repository and README](https://github.com/zed-industries/zed/tree/main/crates/gpui) — Rust GPU-accelerated UI framework; custom elements and rendering model.
- [GPUI documentation](https://gpui.rs/) — API and examples.
- [Rio-VT on docs.rs](https://docs.rs/rio-vt/latest/rio_vt/) — embeddable terminal core.
- [Rio Terminal repository](https://github.com/raphamorim/rio) — implementation and release history.
- [Rio changelog](https://rioterm.com/changelog) — useful for tracking terminal-core changes.
- [WezTerm terminal core](https://github.com/wezterm/wezterm/tree/main/term) — alternative terminal model to benchmark against Rio-VT.
- [WezTerm documentation](https://wezterm.org/) — useful reference for terminal and multiplexer behavior.
- [portable-pty documentation](https://docs.rs/portable-pty/latest/portable_pty/) — cross-platform PTY abstraction to evaluate.

### Shell and integrations

- [Zsh manual](https://zsh.sourceforge.io/Doc/Release/) — shell behavior, startup files, ZLE and completion.
- [Zsh completion system](https://zsh.sourceforge.io/Doc/Release/Completion-System.html) — programmable completion.
- [Starship documentation](https://starship.rs/) — cross-shell prompt configuration.
- [Atuin documentation](https://docs.atuin.sh/latest/) — searchable history and shell integration.
- [Atuin shell integration](https://github.com/atuinsh/atuin/blob/main/docs/docs/guide/shell-integration.md) — Zsh hooks and embedded-terminal considerations.
- [zsh-autosuggestions](https://github.com/zsh-users/zsh-autosuggestions) — inline command suggestions.
- [zsh-syntax-highlighting](https://github.com/zsh-users/zsh-syntax-highlighting) — command-line syntax highlighting.
- [fzf](https://github.com/junegunn/fzf) — general-purpose command-line fuzzy finder.
- [Kitty shell integration documentation](https://sw.kovidgoyal.net/kitty/shell-integration/) — useful reference for Zsh integration and command-aware terminal UX.
- [Microsoft shell integration documentation](https://learn.microsoft.com/en-us/windows/terminal/tutorials/shell-integration) — explanation of semantic prompt escape sequences.
- [Zellij shell integration documentation](https://zellij.dev/documentation/shell-integration.html) — OSC 133 command and prompt markers.

### Optional declarative environment backend

- [Home Manager manual](https://nix-community.github.io/home-manager/) — possible advanced Nix export/integration later; not a default dependency.

---

## Final direction

Build **goatty** as a native Rust terminal environment with GPUI, Rio-VT and a custom GPU terminal renderer. Use a separate Rust daemon to own PTYs, sessions, profiles and plugin operations. Make Zsh the default shell, with a curated managed configuration built around Starship, Atuin, Zsh autosuggestions, syntax highlighting and the native completion system.

Make the profile the source of truth. Let declarative plugins contribute tools, shell integrations, prompt modules and GUI settings. Install tools in a goatty-managed user environment by default, and make host-shell integration opt-in, inspectable and reversible. Add Bash and Fish adapters after the Zsh experience is solid. Add remote/web configuration only after the local daemon API is stable.

**The first milestone is not the plugin registry. It is a fast, correct GPUI terminal running Zsh through Rio-VT and a PTY.** Prove that foundation, then build the environment and ecosystem around it.
