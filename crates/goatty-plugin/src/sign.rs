//! Ed25519 signatures over registry indexes.
//!
//! Keys and signatures are hex. A secret key file holds the 32-byte seed
//! as hex (comment lines start with `#`); keep it offline and readable
//! only by you. The public key goes into the registry's `registry.toml`
//! and into the clients that trust the registry.

use std::io::Read;
use std::path::Path;

use anyhow::{Context, Result, bail};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};

/// `index.json.sig`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Detached {
    /// The public key that made the signature (hex).
    pub key: String,
    /// Ed25519 signature over the exact bytes of `index.json` (hex).
    pub signature: String,
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex<const N: usize>(s: &str) -> Result<[u8; N]> {
    let s = s.trim();
    if s.len() != N * 2 {
        bail!("expected {} hex digits, got {}", N * 2, s.len());
    }
    let mut out = [0u8; N];
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).context("not hex")?;
    }
    Ok(out)
}

/// A new secret key from the OS's random source. Returns (secret hex, public hex).
pub fn keygen() -> Result<(String, String)> {
    let mut seed = [0u8; 32];
    std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut seed)).context("read /dev/urandom")?;
    let key = SigningKey::from_bytes(&seed);
    Ok((hex(&seed), hex(key.verifying_key().as_bytes())))
}

/// Write a new secret key to `path` (mode 0600, refusing to overwrite).
/// Returns the public key.
pub fn keygen_file(path: &Path) -> Result<String> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let (secret, public) = keygen()?;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .with_context(|| format!("create {} (it must not exist yet)", path.display()))?;
    writeln!(f, "# goatty registry signing key (SECRET). Public key: {public}")?;
    writeln!(f, "{secret}")?;
    Ok(public)
}

fn secret_from_text(text: &str) -> Result<SigningKey> {
    let line = text.lines().map(str::trim).find(|l| !l.is_empty() && !l.starts_with('#')).context("no key in the file")?;
    Ok(SigningKey::from_bytes(&unhex::<32>(line)?))
}

/// Load a secret key file.
pub fn load_secret(path: &Path) -> Result<SigningKey> {
    let text = std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    secret_from_text(&text).with_context(|| format!("{} is not a goatty signing key", path.display()))
}

/// The public key (hex) of a secret key.
pub fn public_of(key: &SigningKey) -> String {
    hex(key.verifying_key().as_bytes())
}

pub fn sign(key: &SigningKey, data: &[u8]) -> Detached {
    Detached { key: public_of(key), signature: hex(&key.sign(data).to_bytes()) }
}

/// Check `sig` over `data`, made by one of `trusted` (hex public keys).
/// Returns the key that signed.
pub fn verify(data: &[u8], sig: &Detached, trusted: &[String]) -> Result<String> {
    let signer = sig.key.trim().to_ascii_lowercase();
    if !trusted.iter().any(|k| k.trim().eq_ignore_ascii_case(&signer)) {
        bail!("signed by key {signer}, which isn't trusted for this registry");
    }
    let key = VerifyingKey::from_bytes(&unhex::<32>(&signer)?).context("invalid public key")?;
    let signature = Signature::from_bytes(&unhex::<64>(&sig.signature)?);
    key.verify(data, &signature).map_err(|_| anyhow::anyhow!("bad signature: the index was changed after it was signed"))?;
    Ok(signer)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sign_and_verify() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("k.key");
        let public = keygen_file(&path).unwrap();
        assert!(keygen_file(&path).is_err(), "never overwrites a key");
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        let key = load_secret(&path).unwrap();
        assert_eq!(public_of(&key), public);

        let sig = sign(&key, b"index");
        assert_eq!(verify(b"index", &sig, std::slice::from_ref(&public)).unwrap(), public);
        assert!(verify(b"index!", &sig, std::slice::from_ref(&public)).unwrap_err().to_string().contains("bad signature"));
        let other = keygen().unwrap().1;
        assert!(verify(b"index", &sig, &[other]).unwrap_err().to_string().contains("isn't trusted"));
        let forged = Detached { key: public.clone(), signature: "00".repeat(64) };
        assert!(verify(b"index", &forged, &[public]).is_err());
    }
}
