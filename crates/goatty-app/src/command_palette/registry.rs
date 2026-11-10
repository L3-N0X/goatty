//! The commands the palette offers.
//!
//! A [`Command`] is a label plus what to run. The [`CommandRegistry`] global
//! holds them all: goatty's built-ins (registered at startup by
//! [`register_builtins`]) and, from Stage 5 (#77), the commands of plugins, which
//! register with their own [`Source`] and are removed together when the
//! plugin is unloaded.
//!
//! A command either dispatches a GPUI action (then its shortcut, if it has
//! one, is the action's key binding and shows in the palette) or calls a
//! handler. Commands run with the focused terminal as the action target,
//! after the palette has closed.

use std::rc::Rc;

use gpui::{Action, App, Global, SharedString, Window};

use crate::actions;
use crate::settings_window::{self, Page};
use crate::toast::{self, Toast};

/// Who registered a command.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Source {
    Builtin,
    /// A plugin, by its manifest id.
    #[cfg_attr(not(test), expect(dead_code, reason = "plugins register commands from Stage 5 (#77)"))]
    Plugin(SharedString),
}

/// A command's code.
pub type Handler = Rc<dyn Fn(&mut Window, &mut App)>;

/// What running a command does.
pub enum Run {
    /// Dispatch this action to the focused terminal (and up to the window
    /// and app). `key_action` is its name in `goatty_config::keys`, for
    /// showing the shortcut; `None` for actions that aren't bindable.
    Action { action: Box<dyn Action>, key_action: Option<&'static str> },
    /// Call a function.
    Handler(Handler),
}

pub struct Command {
    /// Stable id, `area.name` (`settings.open`, `docker.ps`). Used for the
    /// recently-used list, so don't rename ids casually.
    pub id: SharedString,
    /// Groups related commands and is shown before the title: "Settings".
    pub category: SharedString,
    /// What the command does, as a short imperative: "Open settings".
    pub title: SharedString,
    /// Other words people might search for ("preferences", "config").
    pub keywords: Vec<SharedString>,
    /// Shown muted on the right (a host's `user@hostname`).
    pub detail: Option<SharedString>,
    /// Listed only once something is typed (or after it was used): the
    /// hosts, which would otherwise crowd the full list.
    pub only_when_searched: bool,
    pub source: Source,
    pub run: Run,
}

impl Command {
    /// A built-in command that dispatches a bindable action.
    pub fn action(id: &str, category: &str, title: &str, key_action: &'static str) -> Self {
        let action = actions::action_by_name(key_action).unwrap_or_else(|| panic!("unknown action {key_action}"));
        Self::new(id, category, title, Run::Action { action, key_action: Some(key_action) })
    }

    /// A built-in command that calls `f`.
    pub fn handler(id: &str, category: &str, title: &str, f: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        Self::new(id, category, title, Run::Handler(Rc::new(f)))
    }

    fn new(id: &str, category: &str, title: &str, run: Run) -> Self {
        Self {
            id: SharedString::from(id.to_owned()),
            category: SharedString::from(category.to_owned()),
            title: SharedString::from(title.to_owned()),
            keywords: Vec::new(),
            detail: None,
            only_when_searched: false,
            source: Source::Builtin,
            run,
        }
    }

    pub fn keywords(mut self, words: &[&str]) -> Self {
        self.keywords.extend(words.iter().map(|w| SharedString::from(w.to_string())));
        self
    }

    /// "Category: Title", what the palette shows and matches against.
    pub fn label(&self) -> String {
        if self.category.is_empty() { self.title.to_string() } else { format!("{}: {}", self.category, self.title) }
    }

    /// Byte offset of the title in [`label`](Self::label).
    pub fn title_start(&self) -> usize {
        if self.category.is_empty() { 0 } else { self.category.len() + 2 }
    }

    /// The `goatty_config::keys` action whose shortcuts this command shows.
    pub fn key_action(&self) -> Option<&'static str> {
        match &self.run {
            Run::Action { key_action, .. } => *key_action,
            Run::Handler(_) => None,
        }
    }

    /// Run the command. The caller has closed the palette and focused the
    /// terminal, so actions reach it.
    pub fn run(&self, window: &mut Window, cx: &mut App) {
        log::debug!("command: {}", self.id);
        match &self.run {
            Run::Action { action, .. } => window.dispatch_action(action.boxed_clone(), cx),
            Run::Handler(f) => f(window, cx),
        }
    }
}

/// Every command, in registration order. A GPUI global.
#[derive(Default)]
pub struct CommandRegistry {
    commands: Vec<Rc<Command>>,
}

impl Global for CommandRegistry {}

impl CommandRegistry {
    pub fn commands(&self) -> &[Rc<Command>] {
        &self.commands
    }

    /// Add `command`, replacing an earlier one with the same id (a plugin
    /// reloading). Returns whether one was replaced.
    pub fn register(&mut self, command: Command) -> bool {
        let command = Rc::new(command);
        match self.commands.iter_mut().find(|c| c.id == command.id) {
            Some(slot) => {
                if slot.source != command.source {
                    log::warn!("command {} from {:?} replaces the one from {:?}", command.id, command.source, slot.source);
                }
                *slot = command;
                true
            }
            None => {
                self.commands.push(command);
                false
            }
        }
    }

    /// Remove every command from `source` (a plugin was unloaded).
    #[cfg_attr(not(test), expect(dead_code, reason = "plugins register commands from Stage 5 (#77)"))]
    pub fn unregister_source(&mut self, source: &Source) {
        self.commands.retain(|c| &c.source != source);
    }

    #[cfg_attr(not(test), expect(dead_code, reason = "plugins register commands from Stage 5 (#77)"))]
    pub fn get(&self, id: &str) -> Option<&Rc<Command>> {
        self.commands.iter().find(|c| c.id == id)
    }

    /// Register commands into the app's registry (created if needed).
    pub fn extend(cx: &mut App, commands: impl IntoIterator<Item = Command>) {
        let reg = cx.default_global::<Self>();
        for c in commands {
            reg.register(c);
        }
    }
}

/// goatty's own commands.
pub fn builtins() -> Vec<Command> {
    let page = |id: &str, title: &str, page: Page, keywords: &[&str]| {
        Command::handler(id, "Settings", title, move |_, cx| settings_window::open_at(cx, Some(page))).keywords(keywords)
    };
    vec![
        Command::action("settings.open", "Settings", "Open settings", "open_settings").keywords(&["preferences", "options", "config"]),
        page("settings.shortcuts", "Keyboard shortcuts", Page::Shortcuts, &["keybindings", "keys", "hotkeys", "bind"]),
        page("settings.appearance", "Appearance and themes", Page::Appearance, &["theme", "colors", "font", "cursor"]),
        page("settings.shell", "Shell", Page::Shell, &["zsh", "managed", "dotfiles", "zshrc"]),
        page("settings.prompt", "Prompt", Page::Prompt, &["starship"]),
        page("settings.history", "History", Page::History, &["atuin"]),
        page(
            "settings.aliases",
            "Aliases: add, edit and switch aliases",
            Page::Aliases,
            &["alias", "abbreviations", "commands", "shorthand"],
        ),
        page(
            "plugins.open",
            "Plugins: browse, install and configure",
            Page::Plugins,
            &["extensions", "install", "tools", "eza", "zoxide", "bat"],
        ),
        page(
            "settings.terminal",
            "Links and pasting",
            Page::Terminal,
            &["terminal", "urls", "hyperlinks", "paste", "confirm", "open files"],
        ),
        page(
            "settings.workspace",
            "Workspace: tabs, panes and sessions",
            Page::Workspace,
            &["launch", "tab bar", "style", "compact", "icons", "split", "dim"],
        ),
        page("settings.profile", "Profile: export and import", Page::Profile, &["backup", "sync"]),
        page("settings.hosts", "Hosts: add hosts, plain SSH", Page::Hosts, &["ssh", "remote", "servers", "connect"]),
        page("settings.about", "About goatty", Page::About, &["version", "info", "bug report", "license"]),
        page("shell.diagnostics", "Shell diagnostics", Page::Diagnostics, &["doctor", "status", "startup", "slow", "missing", "tools"]),
        Command::handler("app.welcome", "goatty", "Welcome and setup", |_, cx| crate::welcome::open(cx)).keywords(&[
            "first run",
            "onboarding",
            "install tools",
            "starship",
            "atuin",
            "getting started",
        ]),
        Command::handler("settings.open_file", "Settings", "Open settings.json in an editor", |_, cx| {
            let Some(path) = goatty_config::client_file() else {
                toast::show(cx, Toast::error("Can't find settings.json").body("Neither $XDG_CONFIG_HOME nor $HOME is set."));
                return;
            };
            if !path.exists() {
                // Opening a missing file does nothing visible; start an empty document.
                let created = path.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|()| std::fs::write(&path, "{}\n"));
                if let Err(e) = created {
                    toast::show(cx, Toast::error(format!("Couldn't create {}", path.display())).body(e.to_string()));
                    return;
                }
            }
            cx.open_with_system(&path);
            toast::show(cx, Toast::info("Opening settings.json").body(path.display().to_string()).key("settings.open_file"));
        })
        .keywords(&["json", "edit", "config"]),
        Command::handler("themes.open_folder", "Themes", "Open the themes folder", |_, cx| {
            let Some(dir) = goatty_config::themes::themes_dir() else {
                toast::show(cx, Toast::error("Can't find the themes folder").body("Neither $XDG_CONFIG_HOME nor $HOME is set."));
                return;
            };
            if let Err(e) = std::fs::create_dir_all(&dir) {
                toast::show(cx, Toast::error(format!("Couldn't create {}", dir.display())).body(e.to_string()));
                return;
            }
            cx.open_with_system(&dir);
            toast::show(cx, Toast::info("Opening the themes folder").body(dir.display().to_string()).key("themes.open_folder"));
        })
        .keywords(&["directory", "files"]),
        Command::action("ssh.connect", "SSH", "Connect to a host…", "connect_to_host")
            .keywords(&["remote", "server", "hosts", "goattyd", "open"]),
        Command::action("shell.reload", "Shell", "Reload running shells", "reload_shells")
            .keywords(&["restart", "refresh", "zsh", "apply", "source"]),
        Command::action("terminal.copy", "Terminal", "Copy the selection", "copy").keywords(&["clipboard"]),
        Command::action("terminal.paste", "Terminal", "Paste", "paste").keywords(&["clipboard"]),
        Command::action("terminal.scroll_page_up", "Terminal", "Scroll back one screen", "scroll_page_up"),
        Command::action("terminal.scroll_page_down", "Terminal", "Scroll forward one screen", "scroll_page_down"),
        Command::action("view.zoom_in", "View", "Bigger text", "increase_font_size").keywords(&["zoom in", "font size", "larger"]),
        Command::action("view.zoom_out", "View", "Smaller text", "decrease_font_size").keywords(&["zoom out", "font size"]),
        Command::action("view.zoom_reset", "View", "Reset text size", "reset_font_size").keywords(&["zoom", "font size", "default"]),
        Command::action("tab.new", "Tabs", "New tab", "new_tab").keywords(&["open", "create", "session"]),
        Command::action("tab.close", "Tabs", "Close the tab", "close_tab").keywords(&["kill", "exit"]),
        Command::action("tab.next", "Tabs", "Next tab", "next_tab").keywords(&["switch"]),
        Command::action("tab.previous", "Tabs", "Previous tab", "previous_tab").keywords(&["switch", "back"]),
        Command::action("tab.move_left", "Tabs", "Move the tab left", "move_tab_left").keywords(&["reorder"]),
        Command::action("tab.move_right", "Tabs", "Move the tab right", "move_tab_right").keywords(&["reorder"]),
        Command::action("tab.rename", "Tabs", "Rename the tab", "rename_tab").keywords(&["title", "name", "label"]),
        Command::action("tab.select_1", "Tabs", "Go to tab 1", "select_tab_1").keywords(&["first"]),
        Command::action("tab.select_2", "Tabs", "Go to tab 2", "select_tab_2"),
        Command::action("tab.select_3", "Tabs", "Go to tab 3", "select_tab_3"),
        Command::action("tab.select_4", "Tabs", "Go to tab 4", "select_tab_4"),
        Command::action("tab.select_5", "Tabs", "Go to tab 5", "select_tab_5"),
        Command::action("tab.select_6", "Tabs", "Go to tab 6", "select_tab_6"),
        Command::action("tab.select_7", "Tabs", "Go to tab 7", "select_tab_7"),
        Command::action("tab.select_8", "Tabs", "Go to tab 8", "select_tab_8"),
        Command::action("tab.select_last", "Tabs", "Go to the last tab", "select_last_tab").keywords(&["end"]),
        Command::action("pane.split_right", "Panes", "Split right", "split_right").keywords(&["vertical", "side by side", "new pane"]),
        Command::action("pane.split_down", "Panes", "Split down", "split_down").keywords(&["horizontal", "below", "new pane"]),
        Command::action("pane.close", "Panes", "Close the pane", "close_pane").keywords(&["kill", "exit"]),
        Command::action("pane.focus_left", "Panes", "Focus the pane on the left", "focus_pane_left").keywords(&["move", "go"]),
        Command::action("pane.focus_right", "Panes", "Focus the pane on the right", "focus_pane_right").keywords(&["move", "go"]),
        Command::action("pane.focus_up", "Panes", "Focus the pane above", "focus_pane_up").keywords(&["move", "go"]),
        Command::action("pane.focus_down", "Panes", "Focus the pane below", "focus_pane_down").keywords(&["move", "go"]),
        Command::action("pane.resize_left", "Panes", "Move the divider left", "resize_pane_left").keywords(&["resize", "size"]),
        Command::action("pane.resize_right", "Panes", "Move the divider right", "resize_pane_right").keywords(&["resize", "size"]),
        Command::action("pane.resize_up", "Panes", "Move the divider up", "resize_pane_up").keywords(&["resize", "size"]),
        Command::action("pane.resize_down", "Panes", "Move the divider down", "resize_pane_down").keywords(&["resize", "size"]),
        Command::action("pane.zoom", "Panes", "Zoom the pane (or back to the split)", "toggle_pane_zoom").keywords(&[
            "maximize",
            "fullscreen",
            "fill",
        ]),
        Command::action("pane.equalize", "Panes", "Make all panes the same size", "equalize_panes").keywords(&["balance", "even", "reset"]),
        Command::handler("notifications.dismiss_all", "Notifications", "Dismiss all", |_, cx| toast::dismiss_all(cx))
            .keywords(&["toasts", "clear", "close", "hide"]),
        Command::handler("app.quit", "goatty", "Quit", |_, cx| cx.quit()).keywords(&["exit", "close"]),
    ]
}

/// Fill the registry with the built-ins.
pub fn register_builtins(cx: &mut App) {
    CommandRegistry::extend(cx, builtins());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_ids_are_unique() {
        let all = builtins();
        let mut ids: Vec<&str> = all.iter().map(|c| c.id.as_ref()).collect();
        ids.sort_unstable();
        let n = ids.len();
        ids.dedup();
        assert_eq!(ids.len(), n, "duplicate command ids");
    }

    #[test]
    fn every_bindable_action_is_a_command() {
        // Except the palette itself.
        let all = builtins();
        for a in goatty_config::keys::ACTIONS {
            if matches!(a.name, "command_palette" | "copy_or_interrupt") {
                continue;
            }
            assert!(all.iter().any(|c| c.key_action() == Some(a.name)), "{} has no palette command", a.name);
        }
    }

    #[test]
    fn plugins_register_and_unregister() {
        let mut reg = CommandRegistry::default();
        for c in builtins() {
            reg.register(c);
        }
        let n = reg.commands().len();
        let docker = Source::Plugin("docker".into());
        let mut c = Command::handler("docker.ps", "Docker", "List containers", |_, _| {});
        c.source = docker.clone();
        assert!(!reg.register(c));
        let mut c = Command::handler("docker.ps", "Docker", "List running containers", |_, _| {});
        c.source = docker.clone();
        assert!(reg.register(c), "same id replaces");
        assert_eq!(reg.commands().len(), n + 1);
        assert_eq!(reg.get("docker.ps").unwrap().label(), "Docker: List running containers");
        reg.unregister_source(&docker);
        assert_eq!(reg.commands().len(), n);
    }
}
