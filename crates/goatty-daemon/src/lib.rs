//! goattyd, the goatty session daemon, as a library.
//!
//! Owns every PTY and a headless, authoritative terminal state per session.
//! Clients connect over a Unix socket, directly or through `goattyd proxy`
//! (which is what `ssh host goattyd proxy` runs). They get a VT snapshot,
//! then a live byte stream. Closing a client detaches; the shell keeps running.
//!
//! * [`server`]: socket setup, accept loop, session registry,
//! * `client`: one connection's protocol loop,
//! * `session`: PTY + terminal + subscribers with flow control,
//! * `layouts`: each window's tabs and splits, restored by the next window,
//! * `settings`: host settings, generations and rollback over the protocol,
//! * [`proxy`]: stdio ⇄ socket relay for remote transports.
//!
//! The `goattyd` binary is a thin CLI over this crate.

mod client;
mod layouts;
pub mod proxy;
pub mod server;
mod session;
mod settings;

pub use proxy::proxy;
pub use server::serve;
