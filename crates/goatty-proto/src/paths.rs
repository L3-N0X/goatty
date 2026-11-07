//! Where the daemon socket lives on a host.

use std::path::PathBuf;

pub fn runtime_dir() -> PathBuf {
    let base = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join(format!("goatty-{}", user_id())));
    base.join("goatty")
}

pub fn socket_path() -> PathBuf {
    if let Some(p) = std::env::var_os("GOATTY_SOCKET") {
        return PathBuf::from(p);
    }
    runtime_dir().join("goattyd.sock")
}

fn user_id() -> String {
    std::env::var("UID").or_else(|_| std::env::var("USER")).unwrap_or_else(|_| "user".into())
}
