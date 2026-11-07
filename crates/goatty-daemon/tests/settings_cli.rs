//! `goattyd settings …` against a sandboxed `$HOME`.

use std::path::Path;
use std::process::{Command, Output};

fn goattyd(home: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_goattyd"))
        .args(args)
        .env("HOME", home)
        .env_remove("XDG_CONFIG_HOME")
        .env_remove("XDG_DATA_HOME")
        .env_remove("GOATTY_SHELL")
        .env_remove("GOATTY_SHELL_MANAGED")
        .env_remove("GOATTY_ZSH_USER_RC")
        .env("GOATTY_SOCKET", home.join("none.sock"))
        .output()
        .unwrap()
}

fn out(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn err(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

#[test]
fn set_show_validate_unset() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    let file = home.join(".config/goatty/host.json");

    let o = goattyd(home, &["settings", "path"]);
    assert_eq!(out(&o).trim(), file.display().to_string());
    assert_eq!(out(&goattyd(home, &["settings", "show"])), "{\n  \"version\": 1\n}\n");

    let o = goattyd(home, &["settings", "set", "history.size", "5000"]);
    assert!(o.status.success(), "{}", err(&o));
    assert!(out(&o).contains("history.size = 5000 (generation 1"), "{}", out(&o));
    assert!(std::fs::read_to_string(&file).unwrap().contains("5000"));
    assert!(home.join(".local/share/goatty/current/zsh/settings.zsh").is_file());

    let o = goattyd(home, &["settings", "set", "prompt.style", "minimal"]);
    assert!(o.status.success(), "{}", err(&o));
    assert_eq!(out(&goattyd(home, &["settings", "show", "prompt.style"])).trim(), "\"minimal\"");
    assert!(out(&goattyd(home, &["settings", "show", "--all"])).contains("\"duration_threshold_ms\": 2000"));

    for (key, value) in [("history.size", "lots"), ("prompt.style", "fancy"), ("history.nope", "1")] {
        let o = goattyd(home, &["settings", "set", key, value]);
        assert!(!o.status.success(), "{key}={value} should fail");
        assert!(!err(&o).is_empty());
    }
    assert!(std::fs::read_to_string(&file).unwrap().contains("5000"), "a failed set changes nothing");

    let o = goattyd(home, &["settings", "validate"]);
    assert!(o.status.success(), "{}", err(&o));
    let broken = home.join("broken.json");
    std::fs::write(&broken, r#"{"history": {"size": "x"}, "extra": 1}"#).unwrap();
    let o = goattyd(home, &["settings", "validate", broken.to_str().unwrap()]);
    assert!(!o.status.success());
    assert!(err(&o).contains("history.size") && err(&o).contains("extra"), "{}", err(&o));

    let o = goattyd(home, &["settings", "unset", "history.size"]);
    assert!(o.status.success(), "{}", err(&o));
    assert!(!std::fs::read_to_string(&file).unwrap().contains("5000"));
    assert!(out(&goattyd(home, &["shell", "generations"])).lines().count() >= 3);
}
