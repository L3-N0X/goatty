use super::*;

fn ids(pieces: &[Piece]) -> Vec<String> {
    let mut out = Vec::new();
    for p in pieces {
        match p {
            Piece::Text(t, _) => out.push(format!("'{t}'")),
            Piece::Module(m) => out.push(m.id.to_owned()),
            Piece::Group(g) => out.push(format!("({})", ids(g).join(" "))),
            Piece::Fill => out.push("FILL".into()),
            Piece::LineBreak => out.push("BR".into()),
            Piece::Character => out.push("CHAR".into()),
        }
    }
    out
}

/// Only these modules on (plus the directory).
fn only(p: &mut PromptSettings, on: &[&str]) {
    p.show = ModuleSet::Essential;
    p.duration_threshold_ms = 0;
    for m in MODULES {
        p.set_module_on(m, on.contains(&m.id));
    }
}

#[test]
fn catalog_is_consistent() {
    let mut seen = std::collections::HashSet::new();
    for m in MODULES {
        assert!(seen.insert(m.id), "{} twice", m.id);
        assert!(PromptColor::parse(m.color).is_some(), "{}: color {}", m.id, m.color);
        if let Some(lead) = m.segment {
            let at = |id: &str| MODULES.iter().position(|x| x.id == id).unwrap();
            assert!(at(lead) < at(m.id), "{}: its lead {lead} comes first", m.id);
        }
        assert!(!m.content.is_empty(), "{}", m.id);
    }
    assert!(module("directory").unwrap().always);
    assert_eq!(MODULES[0].id, "os", "the OS icon leads, as in p10k");
    for id in ESSENTIAL {
        assert!(module(id).is_some(), "{id}");
    }
}

#[test]
fn colors_parse_and_print() {
    assert_eq!(PromptColor::parse("cyan"), Some(PromptColor::Ansi(6)));
    assert_eq!(PromptColor::parse("Bright-Blue"), Some(PromptColor::Ansi(12)));
    assert_eq!(PromptColor::parse("magenta"), Some(PromptColor::Ansi(5)));
    assert_eq!(PromptColor::parse("9"), Some(PromptColor::Ansi(9)));
    assert_eq!(PromptColor::parse("236"), Some(PromptColor::Indexed(236)));
    assert_eq!(PromptColor::parse("#FFAFF3"), Some(PromptColor::Rgb(0xff, 0xaf, 0xf3)));
    for bad in ["", "256", "#fff", "grey", "bright-", "#gggggg"] {
        assert_eq!(PromptColor::parse(bad), None, "{bad}");
    }
    assert_eq!(PromptColor::Ansi(13).name(), "bright-purple");
    assert_eq!(PromptColor::Rgb(1, 2, 255).name(), "#0102ff");
    assert_eq!(PromptColor::Ansi(13).zsh(), "13");
    assert_eq!(xterm_rgb(16), (0, 0, 0));
    assert_eq!(xterm_rgb(208), (255, 135, 0));
    assert_eq!(xterm_rgb(236), (48, 48, 48));
}

#[test]
fn symbols_come_from_the_presets() {
    let rust = module("rust").unwrap();
    assert_eq!(rust.symbol(Symbols::Emoji), "🦀 ");
    assert_eq!(rust.symbol(Symbols::Nerd), "\u{f1617} ");
    assert_eq!(rust.symbol(Symbols::Text), "rs ");
    assert_eq!(rust.symbol(Symbols::None), "");
    assert_eq!(module("status").unwrap().symbol(Symbols::Text), "x", "styling markup is dropped");
    let mut p = PromptSettings::default();
    p.edit_module("rust", |c| c.symbol = Some("R ".into()));
    assert_eq!(p.module_symbol(rust), "R ");
    assert_eq!(p.character_symbols(), ["❯", "❯", "❮"]);
    p.symbols = Symbols::Text;
    assert_eq!(p.character_symbols(), [">", "x", "<"]);
    p.character.success = "λ".into();
    assert_eq!(p.character_symbols()[0], "λ");
}

#[test]
fn module_switches_stay_sparse() {
    let mut p = PromptSettings::default();
    let rust = module("rust").unwrap();
    let time = module("time").unwrap();
    assert!(p.module_on(rust) && !p.module_on(time));
    p.set_module_on(rust, false);
    p.set_module_on(time, true);
    assert!(!p.module_on(rust) && p.module_on(time));
    p.set_module_on(rust, true);
    p.set_module_on(time, false);
    assert!(p.modules.is_empty(), "{:?}", p.modules);
    p.show = ModuleSet::Essential;
    assert!(!p.module_on(rust) && p.module_on(module("git_status").unwrap()));
    assert!(p.module_on(module("directory").unwrap()), "always on");
    p.set_module_on(module("cmd_duration").unwrap(), false);
    assert_eq!(p.duration_threshold_ms, 0, "the duration's switch is its threshold");
}

#[test]
fn lean_one_line_puts_the_right_side_in_right_format() {
    let mut p = PromptSettings { style: PromptStyle::Lean, two_line: false, ..Default::default() };
    only(&mut p, &["git_branch", "time"]);
    let l = p.layout();
    assert_eq!(ids(&l.left), ["directory", "git_branch", "CHAR"]);
    assert_eq!(ids(&l.right), ["time"]);
}

#[test]
fn two_lines_fill_toward_the_right_side_and_frame_it() {
    let mut p = PromptSettings { style: PromptStyle::Lean, frame: Frame::Full, connection: Connection::Dotted, ..Default::default() };
    only(&mut p, &["time"]);
    let l = p.layout();
    assert_eq!(ids(&l.left), ["'╭─'", "' '", "directory", "FILL", "' '", "time", "'─╮'", "BR", "'╰─'", "CHAR"]);
    assert_eq!(ids(&l.right), ["'─╯'"]);
    // Nothing on the right and no frame: no fill.
    p.frame = Frame::None;
    p.set_module_on(module("time").unwrap(), false);
    assert_eq!(ids(&p.layout().left), ["directory", "BR", "CHAR"]);
}

#[test]
fn classic_separators_sit_between_shown_segments() {
    let mut p = PromptSettings { style: PromptStyle::Classic, two_line: false, ..Default::default() };
    only(&mut p, &["os", "git_branch", "git_status", "rust"]);
    let l = p.layout();
    let sep = format!("' {}'", Separator::Angled.glyph(Side::Left));
    let close = format!("'{}'", Edge::Sharp.glyph(Side::Right));
    assert_eq!(
        ids(&l.left),
        [format!("(os ({sep} directory) ({sep} git_branch git_status) ({sep} rust) ' ' {close})"), "' '".into(), "CHAR".into()]
    );
    let Piece::Group(bar) = &l.left[0] else { panic!() };
    let Piece::Group(after) = &bar[2] else { panic!() };
    assert_eq!(after[0], Piece::Text(sep.trim_matches('\'').into(), Style::on("bright-black", "236")));
    assert_eq!(p.look(module("rust").unwrap()).style, Style::on("red", "236"));
}

#[test]
fn rainbow_segments_close_themselves_in_their_lead_color() {
    let mut p = PromptSettings { style: PromptStyle::Rainbow, heads: Edge::Round, tails: Edge::Round, ..Default::default() };
    only(&mut p, &["git_branch", "git_status", "time"]);
    p.edit_module("git_branch", |c| c.color = Some("green".into()));
    let l = p.layout();
    let (o, c) = (Edge::Round.glyph(Side::Left), Edge::Round.glyph(Side::Right));
    assert_eq!(ids(&l.left)[1], format!("('{o}' git_branch git_status ' ' '{c}')"));
    assert_eq!(ids(&l.left)[2], "FILL");
    let status = p.look(module("git_status").unwrap());
    assert_eq!(status.style, Style::on("black", "green"), "git status sits on the branch's color");
    assert_eq!((status.lead, status.trail), (" ", ""));
}

#[test]
fn moved_modules_form_their_own_segments() {
    let mut p = PromptSettings { style: PromptStyle::Rainbow, ..Default::default() };
    only(&mut p, &["git_branch", "git_status"]);
    p.edit_module("git_status", |c| c.side = Some(Side::Right));
    let l = p.layout();
    let flat = ids(&l.left).join(" ");
    assert!(flat.contains("git_branch ' '"), "{flat}");
    assert!(flat.contains("FILL (") && flat.contains("git_status"), "{flat}");
}

#[test]
fn directory_preview_follows_the_settings() {
    let mut p = PromptSettings::default();
    assert_eq!(p.preview_directory().0, "…/goatty/src", "as Starship shows it inside a repository");
    p.directory.truncation_length = 1;
    assert_eq!(p.preview_directory().0, "…/src");
    p.directory.abbreviate = 1;
    assert_eq!(p.preview_directory().0, "~/c/g/src");
    p.directory = DirectoryPrompt { truncate_to_repo: false, truncation_length: 0, home_symbol: "🏠".into(), ..Default::default() };
    assert_eq!(p.preview_directory().0, "🏠/code/goatty/src");
    p.directory.repo_root = true;
    assert_eq!(p.preview_directory(), ("🏠/code/".into(), "goatty".into(), "/src".into()));
    p.directory.substitutions.push(Substitution { from: "🏠/code".into(), to: "C".into() });
    assert_eq!(p.preview_directory().0, "C/goatty/src");
}

#[test]
fn validation_cleans_up() {
    let mut p = PromptSettings::default();
    p.directory.truncation_length = 99;
    p.directory.home_symbol = "a\nb".into();
    p.character.success_color = "nope".into();
    p.directory.substitutions.push(Substitution::default());
    p.edit_module("rust", |c| c.symbol = Some("x".repeat(40)));
    let mut issues = Vec::new();
    p.validate(&mut issues);
    assert_eq!(issues.len(), 5, "{issues:?}");
    assert_eq!(p.directory.truncation_length, DirectoryPrompt::MAX_LENGTH);
    assert_eq!(p.directory.home_symbol, "ab");
    assert_eq!(p.character.success_color, "green");
    assert!(p.directory.substitutions.is_empty());
    assert_eq!(p.modules["rust"].symbol.as_ref().unwrap().len(), MAX_TEXT);
}
