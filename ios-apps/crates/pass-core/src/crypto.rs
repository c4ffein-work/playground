//! OpenPGP, with rPGP: what `gpg -d` and `gpg -e -r ...` do for `pass`.
//!
//! Reads what GnuPG writes, including its own OCB packet (type 20), which GnuPG 2.4
//! uses by default between keys it generated itself. Writes the oldest format every
//! implementation reads: a v3 session key per recipient and SEIPD v1 with AES-256,
//! uncompressed, as `pass` asks of gpg (`--compress-algo=none`).

use std::io::Cursor;

use pgp::composed::{
    ArmorOptions, DecryptionOptions, Deserializable, DetachedSignature, Esk, Message, MessageBuilder, SignedPublicKey,
    SignedPublicSubKey, SignedSecretKey, TheRing,
};
use pgp::crypto::hash::HashAlgorithm;
use pgp::crypto::sym::SymmetricKeyAlgorithm;
use pgp::packet::{Signature, SignatureType};
use pgp::types::{KeyDetails, Password, Timestamp};
use zeroize::Zeroizing;

use crate::PassError;

fn key_error(context: &str, e: impl std::fmt::Display) -> PassError {
    PassError::InvalidKey {
        message: format!("{context}: {e}"),
    }
}

fn crypto_error(context: &str, e: impl std::fmt::Display) -> PassError {
    PassError::Crypto {
        message: format!("{context}: {e}"),
    }
}

fn upper_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02X}")).collect()
}

fn now() -> u32 {
    Timestamp::now().as_secs()
}

/// The newest of `signatures` that is of type `typ`.
fn latest(signatures: &[Signature], typ: SignatureType) -> Option<&Signature> {
    signatures
        .iter()
        .filter(|s| s.typ() == Some(typ))
        .max_by_key(|s| s.created().map(Timestamp::as_secs).unwrap_or(0))
}

/// Whether a key made at `created` and bound by `binding` is past its expiration.
fn expired(created: Timestamp, binding: &Signature) -> bool {
    match binding.key_expiration_time() {
        Some(validity) if validity.as_secs() != 0 => {
            u64::from(created.as_secs()) + u64::from(validity.as_secs()) < u64::from(now())
        }
        _ => false,
    }
}

/// A recipient: someone's public key, as `gpg --export` gives it.
#[derive(Debug, Clone)]
pub(crate) struct Certificate {
    key: SignedPublicKey,
}

/// The key of a certificate to encrypt to.
enum EncryptionTarget<'a> {
    Primary(&'a SignedPublicKey),
    Subkey(&'a SignedPublicSubKey),
}

impl Certificate {
    /// Every certificate in `text`: armored (one block or several, as gpg exports a keyring) or binary.
    /// Each must carry valid self-signatures: a recipient is not taken on the word of whoever sent the file.
    pub(crate) fn parse(text: &[u8]) -> Result<Vec<Self>, PassError> {
        let (keys, _) = SignedPublicKey::from_reader_many(Cursor::new(text))
            .map_err(|e| key_error("not an OpenPGP public key", e))?;
        let mut found = Vec::new();
        for key in keys {
            let key = key.map_err(|e| key_error("not an OpenPGP public key", e))?;
            key.verify_bindings()
                .map_err(|e| key_error("the public key's self-signatures do not verify", e))?;
            found.push(Self { key });
        }
        if found.is_empty() {
            return Err(key_error("not an OpenPGP public key", "no key in it"));
        }
        Ok(found)
    }

    pub(crate) fn fingerprint(&self) -> String {
        upper_hex(self.key.fingerprint().as_bytes())
    }

    /// `Name <email>` of every identity.
    pub(crate) fn user_ids(&self) -> Vec<String> {
        self.key
            .details
            .users
            .iter()
            .map(|user| String::from_utf8_lossy(user.id.id()).into_owned())
            .collect()
    }

    pub(crate) fn algorithm(&self) -> String {
        format!("{:?}", self.key.algorithm())
    }

    /// The fingerprints of the primary key and of every subkey.
    fn fingerprints(&self) -> Vec<String> {
        std::iter::once(self.fingerprint())
            .chain(
                self.key
                    .public_subkeys
                    .iter()
                    .map(|sub| upper_hex(sub.key.fingerprint().as_bytes())),
            )
            .collect()
    }

    /// Whether a line of `.gpg-id` designates this key, the ways gpg reads `-r`:
    /// a fingerprint, a long or short key id (of the key or of a subkey, with or
    /// without `0x`, a trailing `!` ignored), an email address, or part of a user id.
    pub(crate) fn matches(&self, gpg_id: &str) -> bool {
        let wanted = gpg_id.trim().trim_end_matches('!');
        let hex: String = wanted
            .strip_prefix("0x")
            .or_else(|| wanted.strip_prefix("0X"))
            .unwrap_or(wanted)
            .chars()
            .filter(|c| !c.is_whitespace())
            .map(|c| c.to_ascii_uppercase())
            .collect();
        if matches!(hex.len(), 8 | 16 | 40 | 64) && hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return self
                .fingerprints()
                .iter()
                .any(|fingerprint| fingerprint.ends_with(&hex));
        }
        if wanted.is_empty() {
            return false;
        }
        let wanted = wanted.to_lowercase();
        self.user_ids().iter().any(|user_id| {
            let user_id = user_id.to_lowercase();
            match wanted.strip_prefix('<').and_then(|w| w.strip_suffix('>')) {
                // `<a@b>`: that address exactly.
                Some(email) => user_id.ends_with(&format!("<{email}>")) || user_id == email,
                None => user_id.contains(&wanted),
            }
        })
    }

    /// The key gpg would encrypt to: the newest valid encryption subkey, or the primary
    /// key when it is the one flagged for encryption. A revoked or expired key is refused.
    fn encryption_target(&self) -> Result<EncryptionTarget<'_>, PassError> {
        let refused = |why: &str| PassError::InvalidKey {
            message: format!("the key {} {why}", self.fingerprint()),
        };
        let details = &self.key.details;
        if !details.revocation_signatures.is_empty() {
            return Err(refused("is revoked"));
        }
        let self_signature = details
            .users
            .iter()
            .flat_map(|user| user.signatures.iter())
            .chain(details.direct_signatures.iter())
            .max_by_key(|s| s.created().map(Timestamp::as_secs).unwrap_or(0));
        if self_signature.is_some_and(|s| expired(self.key.created_at(), s)) {
            return Err(refused("has expired"));
        }

        let mut subkeys: Vec<(&SignedPublicSubKey, u32)> = Vec::new();
        for sub in &self.key.public_subkeys {
            let Some(binding) = latest(&sub.signatures, SignatureType::SubkeyBinding) else {
                continue;
            };
            let flags = binding.key_flags();
            let usable = sub.key.algorithm().can_encrypt()
                && (flags.encrypt_comms() || flags.encrypt_storage())
                && latest(&sub.signatures, SignatureType::SubkeyRevocation).is_none()
                && !expired(sub.key.created_at(), binding);
            if usable {
                subkeys.push((sub, sub.key.created_at().as_secs()));
            }
        }
        if let Some((sub, _)) = subkeys.into_iter().max_by_key(|(_, created)| *created) {
            return Ok(EncryptionTarget::Subkey(sub));
        }
        let primary_encrypts = self_signature.is_some_and(|s| {
            let flags = s.key_flags();
            flags.encrypt_comms() || flags.encrypt_storage()
        });
        if primary_encrypts && self.key.algorithm().can_encrypt() {
            return Ok(EncryptionTarget::Primary(&self.key));
        }
        Err(refused("has no usable encryption key"))
    }

    /// The long id of the key a message to this certificate is encrypted to: what
    /// `gpg --list-only` shows for a file, and what tells whether it must be re-encrypted.
    pub(crate) fn encryption_key_id(&self) -> Result<String, PassError> {
        Ok(match self.encryption_target()? {
            EncryptionTarget::Primary(key) => upper_hex(key.legacy_key_id().as_ref()),
            EncryptionTarget::Subkey(sub) => upper_hex(sub.key.legacy_key_id().as_ref()),
        })
    }
}

/// Encrypts to every recipient. The result is what `gpg -e` writes: binary, not armored.
pub(crate) fn encrypt(plaintext: &[u8], recipients: &[&Certificate]) -> Result<Vec<u8>, PassError> {
    if recipients.is_empty() {
        return Err(crypto_error("cannot encrypt", "no recipient"));
    }
    let mut rng = rand::thread_rng();
    let mut builder =
        MessageBuilder::from_bytes("", plaintext.to_vec()).seipd_v1(&mut rng, SymmetricKeyAlgorithm::AES256);
    for recipient in recipients {
        match recipient.encryption_target()? {
            EncryptionTarget::Primary(key) => builder.encrypt_to_key(&mut rng, key),
            EncryptionTarget::Subkey(sub) => builder.encrypt_to_key(&mut rng, sub),
        }
        .map_err(|e| crypto_error("cannot encrypt", e))?;
    }
    builder.to_vec(&mut rng).map_err(|e| crypto_error("cannot encrypt", e))
}

/// The long ids of the keys a message is encrypted to, sorted. Needs no secret key.
pub(crate) fn recipient_key_ids(ciphertext: &[u8]) -> Result<Vec<String>, PassError> {
    let (message, _) =
        Message::from_reader(Cursor::new(ciphertext)).map_err(|e| crypto_error("not an OpenPGP message", e))?;
    let Message::Encrypted { esk, .. } = &message else {
        return Err(crypto_error("not an OpenPGP message", "it is not encrypted"));
    };
    let mut ids: Vec<String> = esk
        .iter()
        .filter_map(|esk| match esk {
            Esk::PublicKeyEncryptedSessionKey(pkesk) => pkesk.id().ok().map(|id| upper_hex(id.as_ref())),
            Esk::SymKeyEncryptedSessionKey(_) => None,
        })
        .collect();
    ids.sort();
    Ok(ids)
}

/// A secret key with its passphrase taken off, held in memory only.
///
/// The passphrase is used once, here, and not kept: deriving the key from it is made
/// slow on purpose, and would be paid on every entry opened. The secret numbers are
/// zeroed when this is dropped (rPGP's key types do it).
pub(crate) struct SecretIdentity {
    key: SignedSecretKey,
    /// The parts of `key` that opened need none anymore.
    no_passphrase: Password,
}

/// Parses an armored (or binary) secret key, as `gpg --export-secret-keys --armor` writes it.
pub(crate) fn parse_secret_key(text: &[u8]) -> Result<SignedSecretKey, PassError> {
    let not_secret = |e: &dyn std::fmt::Display| key_error("not an OpenPGP secret key", e);
    let (mut keys, _) = SignedSecretKey::from_reader_many(Cursor::new(text)).map_err(|e| not_secret(&e))?;
    let key = keys
        .next()
        .ok_or_else(|| not_secret(&"no key in it"))?
        .map_err(|e| not_secret(&e))?;
    if keys.next().is_some() {
        return Err(key_error(
            "more than one secret key",
            "export only the one that decrypts the store",
        ));
    }
    Ok(key)
}

/// The public half of a secret key, as a recipient. Needs no passphrase.
pub(crate) fn certificate_of(key: &SignedSecretKey) -> Certificate {
    Certificate {
        key: key.to_public_key(),
    }
}

/// Whether any part of the secret key is protected by a passphrase.
pub(crate) fn needs_passphrase(key: &SignedSecretKey) -> bool {
    key.primary_key.secret_params().is_encrypted()
        || key
            .secret_subkeys
            .iter()
            .any(|sub| sub.key.secret_params().is_encrypted())
}

pub(crate) fn public_key_armored(key: &SignedSecretKey) -> Result<String, PassError> {
    key.to_public_key()
        .to_armored_string(ArmorOptions::default())
        .map_err(|e| key_error("cannot export the public key", e))
}

impl SecretIdentity {
    /// Checks that the passphrase unlocks a key that can decrypt, so that a wrong
    /// passphrase is found now and not on the first entry opened.
    pub(crate) fn unlock(mut key: SignedSecretKey, passphrase: Zeroizing<String>) -> Result<Self, PassError> {
        let passphrase = Password::from(passphrase.as_str());
        // Each part is unlocked on its own: with `gpg --export-secret-subkeys` the primary
        // key is a stub with no secret in it, and stays one.
        let mut can_decrypt = false;
        let mut unlocked = false;
        for sub in &mut key.secret_subkeys {
            let opened = sub.key.remove_password(&passphrase).is_ok();
            if sub.key.public_key().algorithm().can_encrypt() {
                can_decrypt = true;
                unlocked |= opened;
            }
        }
        let opened = key.primary_key.remove_password(&passphrase).is_ok();
        if key.primary_key.public_key().algorithm().can_encrypt() {
            can_decrypt = true;
            unlocked |= opened;
        }
        if !can_decrypt {
            return Err(key_error(
                "this secret key cannot decrypt",
                "it has no encryption key with its secret part",
            ));
        }
        if !unlocked {
            return Err(PassError::WrongPassphrase);
        }
        Ok(Self {
            key,
            no_passphrase: Password::empty(),
        })
    }

    pub(crate) fn certificate(&self) -> Certificate {
        certificate_of(&self.key)
    }

    pub(crate) fn public_key_armored(&self) -> Result<String, PassError> {
        public_key_armored(&self.key)
    }

    /// Decrypts a `.gpg` file (binary or armored). The plaintext is zeroed when dropped.
    pub(crate) fn decrypt(&self, ciphertext: &[u8]) -> Result<Zeroizing<Vec<u8>>, PassError> {
        let (message, _) =
            Message::from_reader(Cursor::new(ciphertext)).map_err(|e| crypto_error("not an OpenPGP message", e))?;
        let ring = TheRing {
            secret_keys: vec![&self.key],
            key_passwords: vec![&self.no_passphrase],
            decrypt_options: DecryptionOptions::new().enable_gnupg_aead(),
            ..Default::default()
        };
        let (mut message, _) = message.decrypt_the_ring(ring, true).map_err(|e| match e {
            pgp::errors::Error::MissingKey => PassError::NotForThisKey,
            e => crypto_error("cannot decrypt", e),
        })?;
        if message.is_compressed() {
            message = message.decompress().map_err(|e| crypto_error("cannot decompress", e))?;
        }
        message
            .as_data_vec()
            .map(Zeroizing::new)
            .map_err(|e| crypto_error("cannot decrypt", e))
    }

    /// An armored detached signature over `data`, as `gpg --detach-sign --armor` makes
    /// (what git puts in a commit's `gpgsig` header). `None` when no signing key of this
    /// identity has its secret part here.
    pub(crate) fn sign(&self, data: &[u8]) -> Result<Option<String>, PassError> {
        let rng = rand::thread_rng();
        let unlocks = |result: pgp::errors::Result<pgp::errors::Result<()>>| matches!(result, Ok(Ok(())));
        let subkey = self.key.secret_subkeys.iter().find(|sub| {
            sub.key.public_key().algorithm().can_sign()
                && signs(&sub.signatures)
                && unlocks(sub.key.unlock(&self.no_passphrase, |_, _| Ok(())))
        });
        let primary = &self.key.primary_key;
        let signature = if let Some(sub) = subkey {
            DetachedSignature::sign_binary_data(rng, &sub.key, &self.no_passphrase, HashAlgorithm::Sha512, data)
        } else if primary.public_key().algorithm().can_sign()
            && unlocks(primary.unlock(&self.no_passphrase, |_, _| Ok(())))
        {
            DetachedSignature::sign_binary_data(rng, primary, &self.no_passphrase, HashAlgorithm::Sha512, data)
        } else {
            return Ok(None);
        };
        signature
            .and_then(|signature| signature.to_armored_string(ArmorOptions::default()))
            .map(Some)
            .map_err(|e| crypto_error("cannot sign", e))
    }
}

/// Whether a subkey is bound for signing, and not revoked.
fn signs(signatures: &[Signature]) -> bool {
    latest(signatures, SignatureType::SubkeyBinding).is_some_and(|binding| binding.key_flags().sign())
        && latest(signatures, SignatureType::SubkeyRevocation).is_none()
}

impl Certificate {
    /// Whether `signature` (detached, armored or binary) is a valid signature of `data`
    /// by the primary key or a signing subkey of this certificate.
    pub(crate) fn verifies(&self, signature: &[u8], data: &[u8]) -> bool {
        let Ok((signatures, _)) = DetachedSignature::from_reader_many(Cursor::new(signature)) else {
            return false;
        };
        signatures.flatten().any(|signature| {
            signature.verify(&self.key, data).is_ok()
                || self
                    .key
                    .public_subkeys
                    .iter()
                    .any(|sub| signs(&sub.signatures) && signature.verify(sub, data).is_ok())
        })
    }
}
