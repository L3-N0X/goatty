//! goatty's `starship.toml`, generated from the prompt settings
//! (`prompt` in `host.json`, [`goatty_config::prompt`]).
//!
//! The top-level `format`/`right_format` come from
//! [`PromptSettings::layout`](goatty_config::PromptSettings::layout), the
//! same description the settings window previews. Each module gets the
//! symbol preset, its symbol and color, and (except in Starship's own look)
//! a format that draws it in the chosen style.

use goatty_config::HostSettings;
use goatty_config::prompt::{Look, MODULES, Module, Piece, PromptColor, PromptSettings, PromptStyle, Style, Symbols};
use toml::{Table, Value};

const HEADER: &str = "\
# goatty's Starship prompt, generated from the prompt settings in host.json.
# Used unless you prefer your own ~/.config/starship.toml (the Starship plugin's
# prefer_user_config). Rewritten by goatty: change the prompt in goatty's
# settings window (Prompt), or copy this file to ~/.config/starship.toml to
# take it further by hand.
";

pub fn render(s: &HostSettings) -> String {
    let p = &s.prompt;
    let layout = p.layout();
    let mut root = Table::new();
    root.insert("$schema".into(), "https://starship.rs/config-schema.json".into());
    root.insert("format".into(), format(&layout.left).into());
    root.insert("right_format".into(), format(&layout.right).into());
    root.insert("add_newline".into(), p.newline_before.into());
    // Never let a slow module (a huge git repo) hold up the prompt for long.
    root.insert("command_timeout".into(), 500.into());
    if p.symbols == Symbols::Text {
        root.insert("continuation_prompt".into(), "[.](bright-black) ".into());
    }
    for m in MODULES {
        root.insert(m.id.into(), Value::Table(module_table(p, m)));
    }
    let [ok, err, vi] = p.character_symbols();
    let c = &p.character;
    let mut character = Table::new();
    character.insert("success_symbol".into(), format!("[{}](bold {})", escape(ok), c.success_color).into());
    character.insert("error_symbol".into(), format!("[{}](bold {})", escape(err), c.error_color).into());
    character.insert("vimcmd_symbol".into(), format!("[{}](bold {})", escape(vi), c.success_color).into());
    root.insert("character".into(), Value::Table(character));
    let mut fill = Table::new();
    fill.insert("symbol".into(), p.connection.symbol().into());
    fill.insert("style".into(), p.colors.frame.clone().into());
    root.insert("fill".into(), Value::Table(fill));
    format!("{HEADER}{}", toml::to_string(&root).expect("a TOML table serializes"))
}

/// Starship format syntax for `pieces`.
fn format(pieces: &[Piece]) -> String {
    pieces
        .iter()
        .map(|p| match p {
            Piece::Text(t, style) => styled(&escape(t), style),
            Piece::Module(m) => format!("${}", m.id),
            Piece::Group(g) => format!("({})", format(g)),
            Piece::Fill => "$fill".into(),
            Piece::LineBreak => "$line_break".into(),
            Piece::Character => "$character".into(),
        })
        .collect()
}

fn styled(text: &str, style: &Style) -> String {
    let s = style.starship();
    if s.is_empty() { text.to_owned() } else { format!("[{text}]({s})") }
}

/// Literal text in a format string: Starship's special characters escaped.
fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        if matches!(ch, '\\' | '[' | ']' | '(' | ')' | '$') {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

/// `key` may be dotted (`symbols.Arch` → `[os.symbols] Arch`).
fn insert_path(t: &mut Table, key: &str, value: Value) {
    match key.split_once('.') {
        Some((head, rest)) => {
            let sub = t.entry(head).or_insert_with(|| Value::Table(Table::new()));
            if let Value::Table(sub) = sub {
                insert_path(sub, rest, value);
            }
        }
        None => {
            t.insert(key.into(), value);
        }
    }
}

/// Starship's own style for `m` with the user's color, keeping its weight.
fn rich_style(m: &Module, color: &str) -> String {
    m.style
        .split_whitespace()
        .map(|w| if w == m.color || w.strip_prefix("fg:") == Some(m.color) { color } else { w })
        .collect::<Vec<_>>()
        .join(" ")
}

fn module_table(p: &PromptSettings, m: &Module) -> Table {
    let mut t = Table::new();
    if !p.module_on(m) {
        t.insert("disabled".into(), true.into());
        return t;
    }
    if !m.on {
        t.insert("disabled".into(), false.into());
    }
    for (k, v) in m.preset(p.symbols) {
        insert_path(&mut t, k, (*v).into());
    }
    if p.symbols == Symbols::None && m.has_symbol() {
        t.insert("symbol".into(), "".into());
    }
    let cfg = p.modules.get(m.id);
    if let Some(sym) = cfg.and_then(|c| c.symbol.as_deref()) {
        t.insert("symbol".into(), sym.into());
    }
    let color = p.module_color(m);
    let look = (p.style != PromptStyle::Rich).then(|| p.look(m));
    match &look {
        None => {
            let key = match m.id {
                "username" => Some("style_user"),
                "battery" | "git_metrics" | "fossil_metrics" => None,
                _ => Some("style"),
            };
            if let Some(key) = key.filter(|_| cfg.and_then(|c| c.color.as_deref()).is_some()) {
                t.insert(key.into(), rich_style(m, color).into());
            }
        }
        // The style goes into the format itself: not every module has a
        // `style` key (username, battery, the line counts).
        Some(look) => {
            t.insert("format".into(), format!("([{}{}]({}){})", look.lead, m.content, look.style.starship(), look.trail).into());
        }
    }
    match m.id {
        "directory" => directory(p, &mut t, color, look.as_ref()),
        "cmd_duration" => {
            t.insert("min_time".into(), i64::from(p.duration_threshold_ms).into());
        }
        _ => {}
    }
    t
}

fn directory(p: &PromptSettings, t: &mut Table, color: &str, look: Option<&Look>) {
    let d = &p.directory;
    t.insert("truncation_length".into(), i64::from(d.truncation_length).into());
    t.insert("truncate_to_repo".into(), d.truncate_to_repo.into());
    t.insert("fish_style_pwd_dir_length".into(), i64::from(d.abbreviate).into());
    t.insert("truncation_symbol".into(), d.truncation_symbol.clone().into());
    t.insert("home_symbol".into(), d.home_symbol.clone().into());
    if !d.read_only {
        t.insert("read_only".into(), "".into());
    } else if p.symbols == Symbols::Emoji {
        t.insert("read_only".into(), " 🔒".into());
    }
    if !d.substitutions.is_empty() {
        let subs: Table = d.substitutions.iter().map(|s| (s.from.clone(), Value::from(s.to.clone()))).collect();
        t.insert("substitutions".into(), Value::Table(subs));
    }
    let Some(look) = look else {
        if d.repo_root {
            t.insert("before_repo_root_style".into(), color.into());
            t.insert("repo_root_style".into(), format!("bold {color}").into());
        }
        return;
    };
    let read_only = match p.style {
        PromptStyle::Lean => Style::fg("red"),
        PromptStyle::Classic => Style::on("red", &p.colors.segment),
        _ => look.style.clone(),
    };
    t.insert("style".into(), look.style.starship().into());
    t.insert("read_only_style".into(), read_only.starship().into());
    let (lead, trail) = (look.lead, look.trail);
    t.insert("format".into(), format!("[{lead}$path]($style)[$read_only]($read_only_style){trail}").into());
    if d.repo_root {
        let plain = look.style.clone().bold(false);
        t.insert("style".into(), plain.starship().into());
        t.insert("before_repo_root_style".into(), plain.starship().into());
        t.insert("repo_root_style".into(), look.style.clone().bold(true).starship().into());
        t.insert(
            "repo_root_format".into(),
            format!(
                "[{lead}$before_root_path]($before_repo_root_style)[$repo_root]($repo_root_style)[$path]($style)[$read_only]($read_only_style){trail}"
            )
            .into(),
        );
    }
}

/// A zsh prompt string for the prompt character: what a transient prompt
/// shrinks to (`%B%F{2}❯%f%b `).
pub fn transient_prompt(p: &PromptSettings, failed: bool) -> String {
    let [ok, err, _] = p.character_symbols();
    let (sym, color) = if failed { (err, &p.character.error_color) } else { (ok, &p.character.success_color) };
    let color = PromptColor::parse(color).map(PromptColor::zsh).unwrap_or_else(|| "2".into());
    format!("%B%F{{{color}}}{}%f%b ", sym.replace('%', "%%"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use goatty_config::prompt::{Connection, Edge, Frame, ModuleSet, Substitution};

    fn parse(s: &HostSettings) -> Table {
        let text = render(s);
        assert!(text.starts_with("# goatty"));
        text.parse::<Table>().unwrap_or_else(|e| panic!("{e}\n{text}"))
    }

    fn get<'a>(t: &'a Table, path: &str) -> &'a Value {
        path.split('.')
            .try_fold(None::<&Value>, |v, k| match v {
                None => t.get(k).map(Some),
                Some(Value::Table(sub)) => sub.get(k).map(Some),
                _ => None,
            })
            .flatten()
            .unwrap_or_else(|| panic!("no {path}"))
    }

    fn str_at<'a>(t: &'a Table, path: &str) -> &'a str {
        get(t, path).as_str().unwrap_or_else(|| panic!("{path} isn't a string"))
    }

    #[test]
    fn default_is_starships_look_with_nerd_symbols() {
        let t = parse(&HostSettings::default());
        assert_eq!(get(&t, "add_newline").as_bool(), Some(false));
        let f = str_at(&t, "format");
        assert!(f.starts_with("$username$hostname$singularity$directory$vcsh$git_branch"), "only modules that are on: {f}");
        assert!(f.ends_with("$line_break$character"), "{f}");
        assert!(!f.contains("$time"), "off by default: {f}");
        assert_eq!(str_at(&t, "right_format"), "");
        assert_eq!(str_at(&t, "rust.symbol"), "\u{f1617} ");
        assert!(t["rust"].get("format").is_none(), "Starship's own format");
        assert_eq!(get(&t, "os.disabled").as_bool(), Some(true));
        assert_eq!(get(&t, "cmd_duration.min_time").as_integer(), Some(2000));
        assert_eq!(get(&t, "directory.truncation_length").as_integer(), Some(4));
        assert_eq!(str_at(&t, "character.success_symbol"), "[❯](bold green)");
    }

    #[test]
    fn classic_bar_with_frame() {
        let mut s = HostSettings::default();
        let p = &mut s.prompt;
        p.style = PromptStyle::Classic;
        p.frame = Frame::Full;
        p.connection = Connection::Solid;
        p.heads = Edge::Round;
        p.edit_module("time", |c| c.enabled = Some(true));
        p.edit_module("os", |c| c.enabled = Some(true));
        let t = parse(&s);
        assert_eq!(str_at(&t, "os.symbols.Arch"), "\u{f303} ");
        let f = str_at(&t, "format");
        assert!(
            f.starts_with("[╭─](fg:bright-black)($os([ \u{e0b1}](fg:bright-black bg:236)$username)"),
            "the OS icon always shows, so it anchors the separators: {f}"
        );
        assert!(f.contains("([ \u{e0b1}](fg:bright-black bg:236)$git_branch$git_commit"), "{f}");
        assert!(f.contains("$fill ([\u{e0b6}](fg:236)$time[ ](bg:236))"), "{f}");
        assert!(f.ends_with("[─╮](fg:bright-black)$line_break[╰─](fg:bright-black)$character"), "{f}");
        assert_eq!(str_at(&t, "right_format"), "[─╯](fg:bright-black)");
        assert_eq!(str_at(&t, "rust.format"), "([ $symbol($version)](fg:red bg:236))");
        assert_eq!(str_at(&t, "directory.style"), "bold fg:cyan bg:236");
        assert_eq!(str_at(&t, "directory.read_only_style"), "fg:red bg:236");
        assert_eq!(str_at(&t, "fill.symbol"), "─");
        assert_eq!(get(&t, "time.disabled").as_bool(), Some(false));
    }

    #[test]
    fn module_settings_reach_the_tables() {
        let mut s = HostSettings::default();
        let p = &mut s.prompt;
        p.symbols = Symbols::None;
        p.edit_module("rust", |c| c.symbol = Some("R ".into()));
        p.edit_module("git_branch", |c| c.color = Some("#ff8800".into()));
        p.edit_module("python", |c| c.enabled = Some(false));
        p.directory.read_only = false;
        p.directory.repo_root = true;
        p.directory.substitutions.push(Substitution { from: "~/coding".into(), to: "C".into() });
        p.character.success = "[ok]".into();
        let t = parse(&s);
        assert_eq!(str_at(&t, "rust.symbol"), "R ");
        assert_eq!(str_at(&t, "golang.symbol"), "");
        assert_eq!(str_at(&t, "git_branch.style"), "bold #ff8800");
        assert_eq!(get(&t, "python.disabled").as_bool(), Some(true));
        assert_eq!(str_at(&t, "directory.read_only"), "");
        assert_eq!(str_at(&t, "directory.repo_root_style"), "bold cyan");
        assert_eq!(str_at(&t, "directory.substitutions.~/coding"), "C");
        assert_eq!(str_at(&t, "character.success_symbol"), r"[\[ok\]](bold green)");
    }

    #[test]
    fn essentials_only() {
        let mut s = HostSettings::default();
        s.prompt.show = ModuleSet::Essential;
        s.prompt.style = PromptStyle::Lean;
        let t = parse(&s);
        assert_eq!(str_at(&t, "format"), "$directory$git_branch$git_state$git_status$cmd_duration$line_break$character");
        assert_eq!(get(&t, "rust.disabled").as_bool(), Some(true));
        assert_eq!(str_at(&t, "git_status.format"), "([$all_status$ahead_behind](fg:red) )");
    }

    #[test]
    fn transient_prompt_is_zsh_prompt_code() {
        let mut p = PromptSettings::default();
        assert_eq!(transient_prompt(&p, false), "%B%F{2}❯%f%b ");
        p.character.error = "100%".into();
        p.character.error_color = "#ff0000".into();
        assert_eq!(transient_prompt(&p, true), "%B%F{#ff0000}100%%%f%b ");
    }

    /// Every style and option combination is a config the pinned Starship
    /// reads without complaint (when a `starship` is on PATH).
    #[test]
    fn starship_accepts_every_style() {
        let Ok(out) = std::process::Command::new("starship").arg("--version").output() else {
            eprintln!("no starship on PATH: skipped");
            return;
        };
        if !out.status.success() {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let config = tmp.path().join("starship.toml");
        for style in PromptStyle::ALL {
            for symbols in Symbols::ALL {
                for (two_line, frame) in [(true, Frame::Full), (false, Frame::None), (true, Frame::Left)] {
                    let mut s = HostSettings::default();
                    let p = &mut s.prompt;
                    (p.style, p.symbols, p.two_line, p.frame, p.connection) = (style, symbols, two_line, frame, Connection::Dotted);
                    p.directory.repo_root = true;
                    p.edit_module("time", |c| c.enabled = Some(true));
                    p.edit_module("status", |c| c.enabled = Some(true));
                    std::fs::write(&config, render(&s)).unwrap();
                    let out = std::process::Command::new("starship")
                        .args(["prompt", "--status=1", "--cmd-duration=5000", "--terminal-width=100"])
                        .current_dir(env!("CARGO_MANIFEST_DIR"))
                        .env("STARSHIP_CONFIG", &config)
                        .env("STARSHIP_CACHE", tmp.path())
                        .env("STARSHIP_LOG", "warn")
                        .output()
                        .unwrap();
                    let err = String::from_utf8_lossy(&out.stderr);
                    assert!(out.status.success() && !err.contains("WARN") && !err.contains("ERROR"), "{style:?} {symbols:?}: {err}");
                    assert!(!out.stdout.is_empty());
                }
            }
        }
    }
}
