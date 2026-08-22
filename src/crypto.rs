//! Passphrase encryption for exported bundles.
//!
//! A bundle carries private keys, so it has to survive being emailed to
//! yourself or left in a Downloads folder: Argon2id turns the passphrase into
//! a key (slow on purpose — the passphrase is the only secret), XChaCha20-
//! Poly1305 seals the payload, and the KDF parameters travel in an
//! authenticated header so today's files still open after the costs are
//! raised. The result is base64-armored so it survives copy/paste.

use anyhow::{anyhow, bail, Context, Result};
use argon2::{Algorithm, Argon2, Params, Version};
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use chacha20poly1305::aead::rand_core::RngCore;
use chacha20poly1305::aead::{Aead, OsRng, Payload};
use chacha20poly1305::{Key, KeyInit, XChaCha20Poly1305, XNonce};
use zeroize::Zeroize;

const MAGIC: &[u8; 8] = b"NMTBNDL\x01";
const SALT_LEN: usize = 16;
const NONCE_LEN: usize = 24;
const KEY_LEN: usize = 32;
/// magic + three u32 costs + salt + nonce.
const HEADER_LEN: usize = MAGIC.len() + 12 + SALT_LEN + NONCE_LEN;

// Argon2id at ~64 MiB / 3 passes: a few hundred milliseconds here, but it puts
// a hard floor under offline guessing if the file ever leaks.
const M_COST: u32 = 65_536; // KiB
const T_COST: u32 = 3;
const P_COST: u32 = 1;
/// Refuse to derive from parameters that would let us allocate the machine
/// out of memory on a hostile file.
const MAX_M_COST: u32 = 4 * 1024 * 1024; // 4 GiB

pub const ARMOR_BEGIN: &str = "-----BEGIN NO-MORE-TERMIUS BUNDLE-----";
pub const ARMOR_END: &str = "-----END NO-MORE-TERMIUS BUNDLE-----";

/// Shortest passphrase we will encrypt with. Low enough not to be annoying,
/// high enough that Argon2id's cost actually buys something.
pub const MIN_PASSPHRASE: usize = 8;

/// Encrypt `plaintext` under `passphrase`, returning the armored file body.
pub fn seal(passphrase: &str, plaintext: &[u8]) -> Result<String> {
    let mut salt = [0u8; SALT_LEN];
    let mut nonce = [0u8; NONCE_LEN];
    OsRng.fill_bytes(&mut salt);
    OsRng.fill_bytes(&mut nonce);

    let header = header_bytes(M_COST, T_COST, P_COST, &salt, &nonce);
    let mut key = derive_key(passphrase, &salt, M_COST, T_COST, P_COST)?;
    let sealed = XChaCha20Poly1305::new(Key::from_slice(&key))
        .encrypt(
            XNonce::from_slice(&nonce),
            Payload {
                msg: plaintext,
                aad: &header,
            },
        )
        .map_err(|_| anyhow!("encrypting the bundle failed"))?;
    key.zeroize();

    let mut blob = header;
    blob.extend_from_slice(&sealed);
    Ok(armor(&blob))
}

/// Decrypt an armored bundle. A wrong passphrase and a tampered file are the
/// same error on purpose — the tag cannot tell them apart.
pub fn open(passphrase: &str, armored: &str) -> Result<Vec<u8>> {
    let blob = dearmor(armored)?;
    if blob.len() < HEADER_LEN || &blob[..MAGIC.len()] != MAGIC {
        bail!("not a no-more-termius bundle");
    }
    let (header, sealed) = blob.split_at(HEADER_LEN);
    let mut cursor = MAGIC.len();
    let mut next_u32 = || {
        let v = u32::from_le_bytes(header[cursor..cursor + 4].try_into().unwrap());
        cursor += 4;
        v
    };
    let (m_cost, t_cost, p_cost) = (next_u32(), next_u32(), next_u32());
    let salt = &header[cursor..cursor + SALT_LEN];
    let nonce = &header[cursor + SALT_LEN..];

    let mut key = derive_key(passphrase, salt, m_cost, t_cost, p_cost)?;
    let plain = XChaCha20Poly1305::new(Key::from_slice(&key))
        .decrypt(
            XNonce::from_slice(nonce),
            Payload {
                msg: sealed,
                aad: header,
            },
        )
        .map_err(|_| anyhow!("wrong passphrase, or the file was modified"));
    key.zeroize();
    plain
}

fn header_bytes(m: u32, t: u32, p: u32, salt: &[u8], nonce: &[u8]) -> Vec<u8> {
    let mut header = Vec::with_capacity(HEADER_LEN);
    header.extend_from_slice(MAGIC);
    for cost in [m, t, p] {
        header.extend_from_slice(&cost.to_le_bytes());
    }
    header.extend_from_slice(salt);
    header.extend_from_slice(nonce);
    header
}

fn derive_key(passphrase: &str, salt: &[u8], m: u32, t: u32, p: u32) -> Result<[u8; KEY_LEN]> {
    if m > MAX_M_COST {
        bail!("bundle asks for {m} KiB of memory to open — refusing");
    }
    let params = Params::new(m, t, p, Some(KEY_LEN))
        .map_err(|e| anyhow!("bad key-derivation parameters: {e}"))?;
    let mut key = [0u8; KEY_LEN];
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into(passphrase.as_bytes(), salt, &mut key)
        .map_err(|e| anyhow!("deriving the key failed: {e}"))?;
    Ok(key)
}

fn armor(blob: &[u8]) -> String {
    let body = B64.encode(blob);
    let mut out = String::with_capacity(body.len() + body.len() / 64 + 96);
    out.push_str(ARMOR_BEGIN);
    out.push('\n');
    for chunk in body.as_bytes().chunks(64) {
        out.push_str(std::str::from_utf8(chunk).expect("base64 is ascii"));
        out.push('\n');
    }
    out.push_str(ARMOR_END);
    out.push('\n');
    out
}

fn dearmor(armored: &str) -> Result<Vec<u8>> {
    let mut lines = armored.lines().map(str::trim).filter(|l| !l.is_empty());
    if lines.next() != Some(ARMOR_BEGIN) {
        bail!("missing bundle header line — is this the right file?");
    }
    let body: String = lines.take_while(|&l| l != ARMOR_END).collect();
    B64.decode(body).context("bundle body is not valid base64")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Keep the tests fast: the real costs are exercised once, in
    /// `round_trip_at_production_cost`.
    fn cheap_seal(passphrase: &str, plaintext: &[u8]) -> String {
        let (m, t, p) = (64, 1, 1);
        let (salt, nonce) = ([7u8; SALT_LEN], [9u8; NONCE_LEN]);
        let header = header_bytes(m, t, p, &salt, &nonce);
        let key = derive_key(passphrase, &salt, m, t, p).unwrap();
        let sealed = XChaCha20Poly1305::new(Key::from_slice(&key))
            .encrypt(
                XNonce::from_slice(&nonce),
                Payload {
                    msg: plaintext,
                    aad: &header,
                },
            )
            .unwrap();
        let mut blob = header;
        blob.extend_from_slice(&sealed);
        armor(&blob)
    }

    #[test]
    fn round_trip_at_production_cost() {
        let armored = seal("correct horse battery", b"secret payload").unwrap();
        assert!(armored.starts_with(ARMOR_BEGIN));
        assert!(armored.trim_end().ends_with(ARMOR_END));
        assert_eq!(
            open("correct horse battery", &armored).unwrap(),
            b"secret payload"
        );
    }

    #[test]
    fn plaintext_never_appears_in_the_file() {
        let armored = cheap_seal("pw", b"BEGIN OPENSSH PRIVATE KEY");
        assert!(!armored.contains("OPENSSH"));
    }

    #[test]
    fn wrong_passphrase_is_rejected() {
        let armored = cheap_seal("right", b"payload");
        let err = open("wrong", &armored).unwrap_err().to_string();
        assert!(err.contains("wrong passphrase"), "{err}");
    }

    #[test]
    fn tampering_with_the_ciphertext_is_rejected() {
        let armored = cheap_seal("pw", b"payload");
        let mut blob = dearmor(&armored).unwrap();
        *blob.last_mut().unwrap() ^= 1;
        assert!(open("pw", &armor(&blob)).is_err());
    }

    /// The costs live outside the ciphertext, so they have to be covered by
    /// the tag — otherwise they are a free knob for an attacker to turn.
    #[test]
    fn tampering_with_the_header_is_rejected() {
        let armored = cheap_seal("pw", b"payload");
        let mut blob = dearmor(&armored).unwrap();
        blob[MAGIC.len() + 4] ^= 1; // t_cost
        assert!(open("pw", &armor(&blob)).is_err());
    }

    #[test]
    fn absurd_memory_cost_is_refused_before_allocating() {
        let armored = cheap_seal("pw", b"payload");
        let mut blob = dearmor(&armored).unwrap();
        blob[MAGIC.len()..MAGIC.len() + 4].copy_from_slice(&u32::MAX.to_le_bytes());
        let err = open("pw", &armor(&blob)).unwrap_err().to_string();
        assert!(err.contains("refusing"), "{err}");
    }

    #[test]
    fn foreign_files_are_reported_clearly() {
        let err = open("pw", "just some text\n").unwrap_err().to_string();
        assert!(err.contains("missing bundle header"), "{err}");

        let not_ours = armor(b"0123456789012345678901234567890123456789012345678901234567890");
        let err = open("pw", &not_ours).unwrap_err().to_string();
        assert!(err.contains("not a no-more-termius bundle"), "{err}");
    }

    /// Two exports of the same data must not produce the same file: salt and
    /// nonce are fresh every time.
    #[test]
    fn each_export_is_unique() {
        let a = seal("pw", b"payload").unwrap();
        let b = seal("pw", b"payload").unwrap();
        assert_ne!(a, b);
    }
}
