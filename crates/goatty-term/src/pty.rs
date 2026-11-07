//! PTY spawning on top of `portable-pty`, kept behind goatty's own types so
//! the backend can be swapped (e.g. `rustix`-based Unix PTYs) later.

use std::io::{Read, Write};
use std::path::PathBuf;

use anyhow::{Context, Result};
use portable_pty::{CommandBuilder, MasterPty, PtySize, native_pty_system};

use crate::TermSize;

#[derive(Debug, Clone)]
pub struct ShellCommand {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub env: Vec<(String, String)>,
}

impl ShellCommand {
    /// `program args` with extra `env`, started in `$HOME`. Which shell to
    /// run is `goatty-shell`'s decision (`Launcher::launch`).
    pub fn new(program: String, args: Vec<String>, env: Vec<(String, String)>) -> Self {
        Self { program, args, cwd: std::env::var_os("HOME").map(PathBuf::from), env }
    }
}

fn pty_size(s: TermSize) -> PtySize {
    PtySize {
        rows: s.rows.max(1),
        cols: s.cols.max(1),
        pixel_width: s.cols.saturating_mul(s.cell_width),
        pixel_height: s.rows.saturating_mul(s.cell_height),
    }
}

/// A running child process attached to a PTY.
pub struct Pty {
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    child: Box<dyn portable_pty::Child + Send + Sync>,
}

impl Pty {
    /// Spawn `cmd`. Returns the PTY plus a blocking reader for its output,
    /// which the caller should drive from a dedicated thread.
    pub fn spawn(cmd: &ShellCommand, size: TermSize) -> Result<(Self, Box<dyn Read + Send>)> {
        let sys = native_pty_system();
        let pair = sys.openpty(pty_size(size)).context("openpty")?;
        let mut b = CommandBuilder::new(&cmd.program);
        b.args(&cmd.args);
        if let Some(cwd) = &cmd.cwd {
            b.cwd(cwd);
        }
        b.env("TERM", "xterm-256color");
        b.env("COLORTERM", "truecolor");
        b.env("TERM_PROGRAM", "goatty");
        b.env("TERM_PROGRAM_VERSION", env!("CARGO_PKG_VERSION"));
        for (k, v) in &cmd.env {
            b.env(k, v);
        }
        let child = pair.slave.spawn_command(b).with_context(|| format!("spawn {}", cmd.program))?;
        drop(pair.slave);
        let reader = pair.master.try_clone_reader()?;
        let writer = pair.master.take_writer()?;
        Ok((Self { master: pair.master, writer, child }, reader))
    }

    pub fn write(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        self.writer.write_all(bytes)?;
        self.writer.flush()
    }

    pub fn resize(&self, size: TermSize) -> Result<()> {
        self.master.resize(pty_size(size))
    }

    pub fn pid(&self) -> Option<u32> {
        self.child.process_id()
    }

    pub fn try_wait(&mut self) -> Option<u32> {
        self.child.try_wait().ok().flatten().map(|s| s.exit_code())
    }

    pub fn kill(&mut self) {
        let _ = self.child.kill();
    }

    /// Foreground process group's working directory is a later feature;
    /// for now report the shell's cwd via /proc (Linux).
    pub fn cwd(&self) -> Option<PathBuf> {
        let pid = self.pid()?;
        std::fs::read_link(format!("/proc/{pid}/cwd")).ok()
    }
}
