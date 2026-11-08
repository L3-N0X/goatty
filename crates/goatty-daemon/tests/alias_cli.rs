//! `goattyd alias …` against a sandboxed `$HOME` (#88): the user's
//! aliases end up in host.json and the generated `aliases.zsh`; a local
//! plugin's aliases are listed under it and can be switched off.

use std::path::Path;
use std::process::{Command, Output};

fn goattyd(home: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_goattyd"))
        .args(args)
        .env("HOME", home)
        .env("PATH", "/usr/bin:/bin")
        .env_remove("XDG_CONFIG_HOME")
        .env_remove("XDG_DATA_HOME")
        .env_remove("XDG_CACHE_HOME")
        .env_remove("GOATTY_SHELL")
        .env_remove("GOATTY_SHELL_MANAGED")
        .env_remove("GOATTY_ZSH_USER_RC")
        .env("GOATTY_SOCKET", home.join("none.sock"))
        .output()
        .unwrap()
}

#[track_caller]
fn ok(home: &Path, args: &[&str]) -> String {
    let o = goattyd(home, args);
    assert!(o.status.success(), "goattyd {args:?}: {}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr));
    String::from_utf8_lossy(&o.stdout).into_owned()
}

#[track_caller]
fn fails(home: &Path, args: &[&str]) -> String {
    let o = goattyd(home, args);
    assert!(!o.status.success(), "goattyd {args:?} should fail");
    String::from_utf8_lossy(&o.stderr).into_owned()
}

#[test]
fn add_list_switch_edit_remove() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    let generated = || std::fs::read_to_string(home.join(".local/share/goatty/current/zsh/aliases.zsh")).unwrap();
    let plugins = || std::fs::read_to_string(home.join(".local/share/goatty/current/zsh/plugins.zsh")).unwrap();

    assert!(ok(home, &["alias", "list"]).contains("No aliases yet"));
    let out = ok(home, &["alias", "add", "gst", "git", "status", "--group", "Git", "-d", "Short status"]);
    assert!(out.contains("Applied (generation 1)"), "{out}");
    ok(home, &["alias", "add", "-g", "git", "glo", "--", "git", "log", "--oneline"]);
    ok(home, &["alias", "add", "q", r#"print -r -- "it's" | less"#]);
    assert!(generated().contains("# git\nalias -- glo='git log --oneline'\nalias -- gst='git status'\n"), "{}", generated());
    assert!(generated().contains(r#"alias -- q='print -r -- "it'\''s" | less'"#), "{}", generated());

    assert!(fails(home, &["alias", "add", "gst", "x"]).contains("already exists"));
    assert!(fails(home, &["alias", "add", "a b", "x"]).contains("can't contain spaces"));
    assert!(fails(home, &["alias", "remove", "nope"]).contains("no alias"));

    // A local plugin with aliases.
    let dir = home.join("kitty");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("plugin.toml"),
        "schema = 1\nid = \"kitty\"\nname = \"Kitty\"\nversion = \"1.0.0\"\nsummary = \"s\"\npublisher = \"tests\"\n\
         [requires]\ncommands = [\"cat\"]\n\
         [[alias]]\nname = \"meow\"\ncommand = \"cat\"\ndescription = \"Purrs\"\n\
         [[alias]]\nname = \"q\"\ncommand = \"cat\"\nargs = [\"-n\"]\n",
    )
    .unwrap();
    ok(home, &["plugin", "install", "--path", dir.to_str().unwrap(), "--yes"]);
    let list = ok(home, &["alias", "list"]);
    assert!(list.contains("git\n  + glo   git log --oneline\n  + gst   git status   # Short status\n"), "{list}");
    assert!(list.contains("Kitty (plugin kitty)\n  + meow  cat   # Purrs\n  - q     cat -n   (replaced by your alias"), "{list}");
    assert!(list.contains("  + q     print -r -- \"it's\" | less   (replaces Kitty's alias)"), "{list}");

    // Switching off: a plugin alias by name, a group of yours.
    ok(home, &["alias", "disable", "meow"]);
    assert!(!plugins().contains("alias -- meow="), "{}", plugins());
    assert!(ok(home, &["alias", "list"]).contains("  - meow  cat   # Purrs   (switched off)"));
    ok(home, &["alias", "disable", "--group", "GIT"]);
    assert!(generated().contains("# off: glo\n# off: gst\n"), "{}", generated());
    ok(home, &["alias", "enable", "--plugin", "kitty"]);
    assert!(plugins().contains("alias -- meow='cat'"), "{}", plugins());
    assert!(fails(home, &["alias", "disable", "--plugin", "kitty", "woof"]).contains("has no alias \"woof\""));

    // Editing and removing.
    ok(home, &["alias", "edit", "gst", "--name", "gs", "--command", "git status --short", "--group", ""]);
    let host = std::fs::read_to_string(home.join(".config/goatty/host.json")).unwrap();
    assert!(host.contains("\"gs\": {\n      \"command\": \"git status --short\",\n      \"description\": \"Short status\",\n      \"enabled\": false\n    }"), "{host}");
    assert!(fails(home, &["alias", "edit", "meow", "-c", "x"]).contains("only be switched off"));
    ok(home, &["alias", "rm", "gs", "q"]);
    let list = ok(home, &["alias", "list"]);
    assert!(!list.contains("gs ") && list.contains("  + q     cat -n"), "the plugin's q is back: {list}");
}
