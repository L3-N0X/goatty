//! ssh's prompts in the GUI (`SSH_ASKPASS`).
//!
//! When goatty runs ssh ([`crate::SshTarget::askpass`]), it sets
//! `SSH_ASKPASS` to its own binary, `SSH_ASKPASS_REQUIRE=force` (OpenSSH ≥
//! 8.4) so ssh uses it even when goatty was started from a terminal, and
//! [`ENV`]`=1` so the binary knows it's being asked a question rather than
//! started as a terminal. ssh runs `$SSH_ASKPASS "<prompt>"` and reads the
//! answer from its stdout; for yes/no questions it looks at the exit status.
//!
//! This module only decodes the request; the app shows the dialog.

/// Set (to `1`) in the environment of the ssh processes goatty starts.
/// Internal plumbing between goatty and itself, not a setting.
pub const ENV: &str = "GOATTY_ASKPASS";

/// What ssh wants to know.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AskKind {
    /// A passphrase or password: answer with the text.
    Secret,
    /// "Are you sure you want to continue connecting (yes/no/[fingerprint])?":
    /// answer `yes` or `no`.
    HostKey,
    /// A yes/no question (`SSH_ASKPASS_PROMPT=confirm`): exit 0 for yes.
    Confirm,
    /// Information only (`SSH_ASKPASS_PROMPT=none`, e.g. "touch your security
    /// key"): ssh closes it when done.
    Notice,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AskRequest {
    pub prompt: String,
    pub kind: AskKind,
}

/// The request when this process was started as ssh's askpass program.
pub fn request_from_env() -> Option<AskRequest> {
    if std::env::var_os(ENV).is_none_or(|v| v != "1") {
        return None;
    }
    let prompt = std::env::args().nth(1).unwrap_or_default();
    let hint = std::env::var("SSH_ASKPASS_PROMPT").ok();
    Some(AskRequest { kind: classify(&prompt, hint.as_deref()), prompt })
}

pub fn classify(prompt: &str, hint: Option<&str>) -> AskKind {
    match hint {
        Some("confirm") => AskKind::Confirm,
        Some("none") => AskKind::Notice,
        _ if prompt.contains("(yes/no") => AskKind::HostKey,
        _ => AskKind::Secret,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompts_are_classified() {
        assert_eq!(classify("Enter passphrase for key '/home/u/.ssh/id_ed25519': ", None), AskKind::Secret);
        assert_eq!(classify("u@box's password: ", None), AskKind::Secret);
        let host_key = "The authenticity of host 'box (10.0.0.2)' can't be established.\n\
                        ED25519 key fingerprint is SHA256:abc.\n\
                        Are you sure you want to continue connecting (yes/no/[fingerprint])? ";
        assert_eq!(classify(host_key, None), AskKind::HostKey);
        assert_eq!(classify("Allow use of key?", Some("confirm")), AskKind::Confirm);
        assert_eq!(classify("Confirm user presence for key ED25519-SK", Some("none")), AskKind::Notice);
    }
}
