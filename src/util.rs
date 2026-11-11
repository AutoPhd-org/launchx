//! Small helpers: random tokens, HMAC, time, process checks.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use sha2::{Digest, Sha256};

/// A random URL-safe token with 256 bits of entropy.
pub fn token() -> String {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).expect("the operating system's random source failed");
    URL_SAFE_NO_PAD.encode(bytes)
}

pub fn hmac_sha256(key: &[u8], message: &[u8]) -> [u8; 32] {
    let mut block = [0u8; 64];
    if key.len() > 64 {
        block[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        block[..key.len()].copy_from_slice(key);
    }
    let mut inner = [0x36u8; 64];
    let mut outer = [0x5cu8; 64];
    for i in 0..64 {
        inner[i] ^= block[i];
        outer[i] ^= block[i];
    }
    let inner_hash = Sha256::new().chain_update(inner).chain_update(message).finalize();
    Sha256::new().chain_update(outer).chain_update(inner_hash).finalize().into()
}

pub fn sign(key: &str, message: &str) -> String {
    URL_SAFE_NO_PAD.encode(hmac_sha256(key.as_bytes(), message.as_bytes()))
}

/// Compare secrets without leaking where they differ.
pub fn same(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

pub fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

pub fn unix_now() -> i64 {
    chrono::Utc::now().timestamp()
}

pub fn pid_alive(pid: u32) -> bool {
    if pid == 0 || pid > i32::MAX as u32 {
        return false;
    }
    // SAFETY: signal 0 only checks that the process exists.
    let result = unsafe { libc::kill(pid as i32, 0) };
    result == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

pub fn signal(pid: u32, signal: i32) {
    if pid > 0 && pid <= i32::MAX as u32 {
        // SAFETY: plain kill(2) on a pid we started.
        unsafe { libc::kill(pid as i32, signal) };
    }
}

/// Signal every process in the group led by `pgid`.
pub fn signal_group(pgid: u32, signal: i32) {
    if pgid > 1 && pgid <= i32::MAX as u32 {
        // SAFETY: plain kill(2) on a process group we created.
        unsafe { libc::kill(-(pgid as i32), signal) };
    }
}

pub fn process_group(pid: u32) -> Option<u32> {
    if pid == 0 || pid > i32::MAX as u32 {
        return None;
    }
    // SAFETY: getpgid(2) only reads.
    let group = unsafe { libc::getpgid(pid as i32) };
    (group > 0).then_some(group as u32)
}

/// Escape text for HTML.
pub fn html(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\'', "&#39;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hmac_matches_rfc_4231_case_2() {
        let mac = hmac_sha256(b"Jefe", b"what do ya want for nothing?");
        let hex: String = mac.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(hex, "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843");
    }

    #[test]
    fn tokens_are_distinct_and_url_safe() {
        let (a, b) = (token(), token());
        assert_ne!(a, b);
        assert_eq!(a.len(), 43);
        assert!(a.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'));
        assert!(same(&a, &a.clone()) && !same(&a, &b) && !same(&a, "short"));
    }
}
