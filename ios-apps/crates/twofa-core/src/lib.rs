//! Two-factor codes (TOTP and HOTP) and the keychain file of
//! <https://github.com/c4ffein/2fa> and <https://github.com/rsc/2fa>,
//! exposed to Swift through UniFFI.
//!
//! Nothing here keeps state, reads a clock, touches a file or opens a socket:
//! every function takes the text of the keychain file (and the time, when it
//! matters) and returns codes or the new text. Storing that text is the caller's job.
//!
//! The file is one key per line, fields separated by one space:
//!
//! ```text
//! name digits BASE32KEY                         time-based (TOTP, SHA-1, 30 seconds)
//! name digits BASE32KEY 00000000000000000042    counter-based (HOTP): the last counter used, 20 digits
//! ```
//!
//! Lines are only ever appended, removed whole, or changed in place: what this
//! crate does not understand in a file goes through untouched.

use std::collections::BTreeMap;
use std::ops::Range;

use ring::hmac;

uniffi::setup_scaffolding!();

/// The only time step the file format can express.
const PERIOD: u64 = 30;
/// A counter is written as exactly this many digits, so that it can be replaced in place.
const COUNTER_LEN: usize = 20;

/// One key of the keychain. Never carries the secret.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct KeyRow {
    pub name: String,
    /// 6, 7 or 8.
    pub digits: u8,
    /// `None` for a time-based key. For a counter-based one, the last counter used:
    /// the next code is made from this plus one.
    pub counter: Option<u64>,
    /// The code at the given time, for a time-based key. A counter-based key has
    /// none until `next_hotp` is asked for one.
    pub code: Option<String>,
}

/// A line that is neither empty nor a key: kept in the file, shown nowhere.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct SkippedLine {
    /// From 1.
    pub line: u32,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct Keychain {
    /// Sorted by name, as `2fa` lists them. A name defined twice is its last definition.
    pub keys: Vec<KeyRow>,
    pub skipped: Vec<SkippedLine>,
}

/// A new counter-based code, and the keychain that remembers it was given.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct HotpStep {
    pub code: String,
    /// The counter the code was made from.
    pub counter: u64,
    /// Store this before showing the code: a code shown twice is a code refused.
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct ImportOutcome {
    /// The merged keychain.
    pub text: String,
    /// Names that were not there.
    pub added: Vec<String>,
    /// Counter-based keys already there, whose counter was behind the imported one.
    pub advanced: Vec<String>,
    /// Already there, the same.
    pub unchanged: Vec<String>,
    /// Already there with another secret, size or kind: the one already there is kept.
    pub conflicts: Vec<String>,
    /// Lines of the imported text that are not keys.
    pub skipped: Vec<SkippedLine>,
}

/// What an `otpauth://` URI (the content of an authenticator QR code) says.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct OtpauthKey {
    /// A name `add_key` accepts, made from the issuer and the account. Meant to be edited.
    pub suggested_name: String,
    pub issuer: Option<String>,
    pub account: String,
    /// Base32, upper case, padded: as it goes in the file.
    pub secret: String,
    pub digits: u8,
    /// `None` for a time-based key, the starting counter for a counter-based one.
    pub counter: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error, uniffi::Error)]
pub enum TwofaError {
    #[error("{message}")]
    InvalidName { message: String },
    #[error("the key is not valid base32 (letters A to Z and digits 2 to 7)")]
    InvalidSecret,
    #[error("codes are 6, 7 or 8 digits")]
    InvalidDigits,
    #[error("there is already a key named {name}")]
    DuplicateName { name: String },
    #[error("no such key {name}")]
    NoSuchKey { name: String },
    #[error("{name} is a time-based key: it has no counter")]
    NotCounterBased { name: String },
    #[error("the counter of {name} cannot go further")]
    CounterExhausted { name: String },
    #[error("not a usable otpauth:// URI: {message}")]
    InvalidUri { message: String },
    #[error("not supported: {message}")]
    Unsupported { message: String },
}

fn invalid_uri(message: &str) -> TwofaError {
    TwofaError::InvalidUri {
        message: message.to_string(),
    }
}

fn unsupported(message: impl Into<String>) -> TwofaError {
    TwofaError::Unsupported {
        message: message.into(),
    }
}

// Codes

/// RFC 4226, as `2fa` computes it: HMAC-SHA-1, dynamic truncation, zero-padded.
pub fn hotp(secret: &[u8], counter: u64, digits: u8) -> String {
    let key = hmac::Key::new(hmac::HMAC_SHA1_FOR_LEGACY_USE_ONLY, secret);
    let digest = hmac::sign(&key, &counter.to_be_bytes());
    let digest = digest.as_ref();
    let at = (digest[digest.len() - 1] & 0x0f) as usize;
    let value = u32::from_be_bytes([digest[at], digest[at + 1], digest[at + 2], digest[at + 3]]) & 0x7fff_ffff;
    let code = value % 10u32.pow(u32::from(digits.min(8)));
    format!("{code:0width$}", width = usize::from(digits))
}

/// RFC 6238 with SHA-1 and a 30 second step. `unix_time` is in seconds.
pub fn totp(secret: &[u8], unix_time: u64, digits: u8) -> String {
    hotp(secret, unix_time / PERIOD, digits)
}

/// How long the time-based codes of `unix_time` stay valid: 30 down to 1.
#[uniffi::export]
pub fn seconds_remaining(unix_time: u64) -> u32 {
    (PERIOD - unix_time % PERIOD) as u32
}

// Base32

/// Python's `b32decode(key.upper())`, which is what `2fa` reads a key with:
/// padded to a multiple of 8 with `=`, either case, nothing else.
fn decode_base32(key: &str) -> Option<Vec<u8>> {
    let bytes = key.as_bytes();
    let data = bytes.iter().rposition(|&b| b != b'=').map_or(0, |last| last + 1);
    if !bytes.len().is_multiple_of(8) || ![0, 1, 3, 4, 6].contains(&(bytes.len() - data)) {
        return None;
    }
    let mut decoded = Vec::with_capacity(data * 5 / 8);
    let (mut acc, mut bits) = (0u32, 0);
    for &b in &bytes[..data] {
        let value = match b.to_ascii_uppercase() {
            c @ b'A'..=b'Z' => c - b'A',
            c @ b'2'..=b'7' => c - b'2' + 26,
            _ => return None,
        };
        acc = (acc << 5) | u32::from(value);
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            decoded.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Some(decoded)
}

fn encode_base32(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
    let mut encoded = String::with_capacity(bytes.len().div_ceil(5) * 8);
    for chunk in bytes.chunks(5) {
        let mut block = [0u8; 8];
        block[3..3 + chunk.len()].copy_from_slice(chunk);
        let acc = u64::from_be_bytes(block);
        let chars = (chunk.len() * 8).div_ceil(5);
        for i in 0..8 {
            if i < chars {
                encoded.push(ALPHABET[(acc >> (35 - 5 * i)) as usize & 31] as char);
            } else {
                encoded.push('=');
            }
        }
    }
    encoded
}

/// A key as typed or scanned (spaces, lower case, padding or not) into what goes
/// in the file: upper case and padded, which both `2fa` implementations read.
fn normalize_secret(secret: &str) -> Result<String, TwofaError> {
    let mut key: String = secret
        .chars()
        .filter(|c| !c.is_whitespace())
        .map(|c| c.to_ascii_uppercase())
        .collect();
    key.truncate(key.trim_end_matches('=').len());
    while !key.len().is_multiple_of(8) {
        key.push('=');
    }
    match decode_base32(&key) {
        Some(decoded) if !decoded.is_empty() => Ok(key),
        _ => Err(TwofaError::InvalidSecret),
    }
}

// The file

struct Entry<'a> {
    name: &'a str,
    digits: u8,
    secret: Vec<u8>,
    /// Where the 20 digits are in the text, and what they say.
    counter: Option<(usize, u64)>,
}

impl Entry<'_> {
    fn same_key(&self, other: &Entry) -> bool {
        self.secret == other.secret && self.digits == other.digits && self.counter.is_some() == other.counter.is_some()
    }
}

enum Content<'a> {
    Blank,
    Key(Entry<'a>),
    Skipped(&'static str),
}

struct Line<'a> {
    /// In the text, with its line break when it has one.
    range: Range<usize>,
    content: Content<'a>,
}

/// One line, read as `2fa` reads it. `start` is where it begins in the text.
fn parse_line(line: &str, start: usize) -> Content<'_> {
    if line.is_empty() {
        return Content::Blank;
    }
    let fields: Vec<&str> = line.split(' ').collect();
    if fields.len() < 3 || fields[0].is_empty() || !matches!(fields[1], "6" | "7" | "8") {
        return Content::Skipped("not a key: expected `name digits key`, separated by one space");
    }
    let Some(secret) = decode_base32(fields[2]) else {
        return Content::Skipped("malformed key");
    };
    let counter = match fields.len() {
        3 => None,
        4 if fields[3].len() == COUNTER_LEN && fields[3].bytes().all(|b| b.is_ascii_digit()) => {
            // Twenty digits can say more than the 8 bytes a counter is hashed as.
            match fields[3].parse::<u64>() {
                Ok(value) => Some((start + line.len() - COUNTER_LEN, value)),
                Err(_) => return Content::Skipped("counter out of range"),
            }
        }
        4 => return Content::Skipped("malformed counter: expected 20 digits"),
        _ => return Content::Skipped("too many fields"),
    };
    Content::Key(Entry {
        name: fields[0],
        digits: fields[1].as_bytes()[0] - b'0',
        secret,
        counter,
    })
}

fn parse(text: &str) -> Vec<Line<'_>> {
    let mut start = 0;
    text.split('\n')
        .map(|line| {
            let end = (start + line.len() + 1).min(text.len());
            let parsed = Line {
                range: start..end,
                content: parse_line(line, start),
            };
            start += line.len() + 1;
            parsed
        })
        .collect()
}

/// The keys by name: sorted, and the last definition of a name wins, as in `2fa`.
fn keys<'a, 'b>(lines: &'b [Line<'a>]) -> BTreeMap<&'a str, &'b Entry<'a>> {
    lines
        .iter()
        .filter_map(|line| match &line.content {
            Content::Key(entry) => Some((entry.name, entry)),
            _ => None,
        })
        .collect()
}

fn skipped(lines: &[Line]) -> Vec<SkippedLine> {
    lines
        .iter()
        .enumerate()
        .filter_map(|(index, line)| match line.content {
            Content::Skipped(reason) => Some(SkippedLine {
                line: index as u32 + 1,
                reason: reason.to_string(),
            }),
            _ => None,
        })
        .collect()
}

/// What `2fa` itself accepts as a key name on its command line, so that every key
/// added here can be asked for there.
fn check_name(name: &str) -> Result<(), TwofaError> {
    let message = if name.is_empty() {
        "the name is empty"
    } else if !name.bytes().all(|b| b.is_ascii_alphanumeric() || b"-/=".contains(&b)) {
        "a name is made of letters, digits, - / and = (no space, no accent)"
    } else if name.starts_with('-') {
        "a name cannot start with - (2fa would read it as an option)"
    } else {
        return Ok(());
    };
    Err(TwofaError::InvalidName {
        message: message.to_string(),
    })
}

fn counter_text(counter: u64) -> String {
    format!("{counter:0width$}", width = COUNTER_LEN)
}

fn key_line(name: &str, digits: u8, key: &str, counter: Option<u64>) -> String {
    match counter {
        Some(counter) => format!("{name} {digits} {key} {}\n", counter_text(counter)),
        None => format!("{name} {digits} {key}\n"),
    }
}

/// `text`, ready to have a line appended. (`2fa -add` appends as is: after a file
/// that does not end with a line break, its new key and the last one are both lost.)
fn with_line_break(text: &str) -> String {
    let mut text = text.to_string();
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    text
}

/// The keys of a keychain file, with the codes of the time-based ones at `unix_time` (seconds).
#[uniffi::export]
pub fn read_keychain(text: String, unix_time: u64) -> Keychain {
    let lines = parse(&text);
    let keys = keys(&lines)
        .into_values()
        .map(|entry| KeyRow {
            name: entry.name.to_string(),
            digits: entry.digits,
            counter: entry.counter.map(|(_, value)| value),
            code: match entry.counter {
                None => Some(totp(&entry.secret, unix_time, entry.digits)),
                Some(_) => None,
            },
        })
        .collect();
    Keychain {
        keys,
        skipped: skipped(&lines),
    }
}

/// Adds a key at the end, as `2fa -add` does, and returns the new text.
/// `secret` is base32, in either case, with or without spaces and padding.
/// `counter` is `None` for a time-based key; for a counter-based one, the last
/// counter used (0 for a new key: its first code is made from 1, as with `2fa -add -hotp`).
#[uniffi::export]
pub fn add_key(
    text: String,
    name: String,
    digits: u8,
    secret: String,
    counter: Option<u64>,
) -> Result<String, TwofaError> {
    check_name(&name)?;
    if !(6..=8).contains(&digits) {
        return Err(TwofaError::InvalidDigits);
    }
    let key = normalize_secret(&secret)?;
    if keys(&parse(&text)).contains_key(name.as_str()) {
        return Err(TwofaError::DuplicateName { name });
    }
    Ok(with_line_break(&text) + &key_line(&name, digits, &key, counter))
}

/// Removes every line that defines `name` (an earlier one would otherwise come back).
#[uniffi::export]
pub fn remove_key(text: String, name: String) -> Result<String, TwofaError> {
    let lines = parse(&text);
    let mut kept = String::with_capacity(text.len());
    for line in &lines {
        if !matches!(&line.content, Content::Key(entry) if entry.name == name) {
            kept.push_str(&text[line.range.clone()]);
        }
    }
    if kept.len() == text.len() {
        return Err(TwofaError::NoSuchKey { name });
    }
    Ok(kept)
}

/// Renames a key where it is: its secret and its counter do not move.
#[uniffi::export]
pub fn rename_key(text: String, name: String, new_name: String) -> Result<String, TwofaError> {
    check_name(&new_name)?;
    let lines = parse(&text);
    let known = keys(&lines);
    if !known.contains_key(name.as_str()) {
        return Err(TwofaError::NoSuchKey { name });
    }
    if name == new_name {
        return Ok(text);
    }
    if known.contains_key(new_name.as_str()) {
        return Err(TwofaError::DuplicateName { name: new_name });
    }
    let mut renamed = String::with_capacity(text.len() + new_name.len());
    for line in &lines {
        match &line.content {
            Content::Key(entry) if entry.name == name => {
                renamed.push_str(&new_name);
                renamed.push_str(&text[line.range.start + name.len()..line.range.end]);
            }
            _ => renamed.push_str(&text[line.range.clone()]),
        }
    }
    Ok(renamed)
}

/// The next code of a counter-based key: the counter goes up by one, in place,
/// and the code is made from the new value, exactly as `2fa name` does.
#[uniffi::export]
pub fn next_hotp(text: String, name: String) -> Result<HotpStep, TwofaError> {
    let lines = parse(&text);
    let Some(entry) = keys(&lines).get(name.as_str()).copied() else {
        return Err(TwofaError::NoSuchKey { name });
    };
    let Some((offset, last)) = entry.counter else {
        return Err(TwofaError::NotCounterBased { name });
    };
    let Some(counter) = last.checked_add(1) else {
        return Err(TwofaError::CounterExhausted { name });
    };
    let code = hotp(&entry.secret, counter, entry.digits);
    let mut updated = text.clone();
    updated.replace_range(offset..offset + COUNTER_LEN, &counter_text(counter));
    Ok(HotpStep {
        code,
        counter,
        text: updated,
    })
}

/// Merges the keys of another keychain file into `text`. Nothing already there is
/// lost: a key of the same name is kept as it is, except that a counter never goes
/// back (the further of the two is the one the server expects next).
/// Into an empty keychain, `incoming` is taken byte for byte.
#[uniffi::export]
pub fn import_keychain(text: String, incoming: String) -> ImportOutcome {
    let incoming_lines = parse(&incoming);
    let incoming_keys = keys(&incoming_lines);
    let mut outcome = ImportOutcome {
        text: String::new(),
        added: Vec::new(),
        advanced: Vec::new(),
        unchanged: Vec::new(),
        conflicts: Vec::new(),
        skipped: skipped(&incoming_lines),
    };
    if text.trim().is_empty() {
        outcome.added = incoming_keys.keys().map(|name| name.to_string()).collect();
        outcome.text = incoming.clone();
        return outcome;
    }

    let lines = parse(&text);
    let known = keys(&lines);
    let mut merged = with_line_break(&text);
    for (name, theirs) in incoming_keys {
        let Some(ours) = known.get(name) else {
            // Written again rather than copied: whatever case it was in, it decoded.
            let key = encode_base32(&theirs.secret);
            let counter = theirs.counter.map(|(_, value)| value);
            merged.push_str(&key_line(name, theirs.digits, &key, counter));
            outcome.added.push(name.to_string());
            continue;
        };
        if !ours.same_key(theirs) {
            outcome.conflicts.push(name.to_string());
            continue;
        }
        match (ours.counter, theirs.counter) {
            (Some((offset, mine)), Some((_, further))) if further > mine => {
                // Same length, and before anything appended: the offset still holds.
                merged.replace_range(offset..offset + COUNTER_LEN, &counter_text(further));
                outcome.advanced.push(name.to_string());
            }
            _ => outcome.unchanged.push(name.to_string()),
        }
    }
    outcome.text = merged;
    outcome
}

// otpauth://

/// `%41` → `A`. With `plus_is_space`, `+` → space (query values, as most generators write them).
fn percent_decode(s: &str, plus_is_space: bool) -> Result<String, TwofaError> {
    let bytes = s.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' => {
                let hex = s
                    .get(i + 1..i + 3)
                    .filter(|hex| hex.bytes().all(|b| b.is_ascii_hexdigit()));
                let byte = hex.and_then(|hex| u8::from_str_radix(hex, 16).ok());
                decoded.push(byte.ok_or_else(|| invalid_uri("bad percent-encoding"))?);
                i += 2;
            }
            b'+' if plus_is_space => decoded.push(b' '),
            b => decoded.push(b),
        }
        i += 1;
    }
    String::from_utf8(decoded).map_err(|_| invalid_uri("not UTF-8"))
}

/// Something `check_name` accepts, out of anything.
fn suggest_name(issuer: Option<&str>, account: &str) -> String {
    let full = match issuer {
        Some(issuer) if !account.is_empty() => format!("{issuer}-{account}"),
        Some(issuer) => issuer.to_string(),
        None => account.to_string(),
    };
    let mut name = String::new();
    for c in full.chars() {
        if c.is_ascii_alphanumeric() || c == '/' || c == '=' {
            name.push(c);
        } else if !name.is_empty() && !name.ends_with('-') {
            name.push('-');
        }
    }
    let name = name.trim_end_matches('-');
    if name.is_empty() {
        "key".to_string()
    } else {
        name.to_string()
    }
}

/// Reads an `otpauth://` URI (what an authenticator QR code contains), following
/// <https://github.com/google/google-authenticator/wiki/Key-Uri-Format>.
///
/// The file format has no place for an algorithm or a period: anything but SHA-1
/// and 30 seconds is refused here, rather than accepted and given wrong codes.
/// So are other code sizes, Steam's encoder, and `otpauth-migration://` exports.
#[uniffi::export]
pub fn parse_otpauth(uri: String) -> Result<OtpauthKey, TwofaError> {
    let no_scheme = || invalid_uri("no otpauth:// at the start");
    let (scheme, rest) = uri.trim().split_once("://").ok_or_else(no_scheme)?;
    match scheme.to_ascii_lowercase().as_str() {
        "otpauth" => {}
        "otpauth-migration" => {
            return Err(unsupported(
                "this QR code is a Google Authenticator export of several keys at once: \
                 scan the QR code of each service instead",
            ))
        }
        _ => return Err(no_scheme()),
    }
    let rest = rest.split_once('#').map_or(rest, |(before, _)| before);
    let (path, query) = rest.split_once('?').unwrap_or((rest, ""));
    let (kind, label) = path.split_once('/').unwrap_or((path, ""));
    let counter_based = match kind.to_ascii_lowercase().as_str() {
        "totp" => false,
        "hotp" => true,
        other => return Err(unsupported(format!("keys of type `{other}` (only totp and hotp)"))),
    };

    let mut params: BTreeMap<String, String> = BTreeMap::new();
    for pair in query.split('&').filter(|pair| !pair.is_empty()) {
        let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
        let name = percent_decode(name, true)?.to_ascii_lowercase();
        if params.insert(name.clone(), percent_decode(value, true)?).is_some() {
            return Err(invalid_uri(&format!("`{name}` is given twice")));
        }
    }
    let param = |name: &str| {
        params
            .get(name)
            .map(|value| value.trim())
            .filter(|value| !value.is_empty())
    };

    let secret = normalize_secret(param("secret").ok_or_else(|| invalid_uri("it has no secret"))?)?;
    if let Some(algorithm) = param("algorithm") {
        if !algorithm.eq_ignore_ascii_case("SHA1") {
            return Err(unsupported(format!("the {algorithm} algorithm (only SHA1)")));
        }
    }
    let digits = match param("digits") {
        None | Some("6") => 6,
        Some("7") => 7,
        Some("8") => 8,
        Some(other) => return Err(unsupported(format!("codes of {other} digits (only 6, 7 or 8)"))),
    };
    if let Some(encoder) = param("encoder") {
        return Err(unsupported(format!("the {encoder} code format")));
    }
    let counter = if counter_based {
        // Required by the format, left out by some: a new key starts at 0.
        let counter = param("counter").unwrap_or("0");
        let is_number = counter.bytes().all(|b| b.is_ascii_digit());
        let counter = counter.parse::<u64>().ok().filter(|_| is_number);
        Some(counter.ok_or_else(|| invalid_uri("the counter is not a number"))?)
    } else {
        if let Some(period) = param("period") {
            if period != "30" {
                return Err(unsupported(format!("a period of {period} seconds (only 30)")));
            }
        }
        None
    };

    // The label is `issuer:account` or `account`; the issuer may also, or only, be a parameter.
    let label = percent_decode(label, false)?;
    let (label_issuer, account) = match label.split_once(':') {
        Some((issuer, account)) => (Some(issuer.trim()), account.trim()),
        None => (None, label.trim()),
    };
    let issuer = label_issuer.filter(|issuer| !issuer.is_empty()).or(param("issuer"));
    Ok(OtpauthKey {
        suggested_name: suggest_name(issuer, account),
        issuer: issuer.map(str::to_string),
        account: account.to_string(),
        secret,
        digits,
        counter,
    })
}

/// The message of an error, for people. (The Swift `localizedDescription` of a
/// generated error is its case name, not this.)
#[uniffi::export]
pub fn error_message(error: TwofaError) -> String {
    error.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The secret of the RFC test vectors, and its base32.
    const RFC_SECRET: &[u8] = b"12345678901234567890";
    const RFC_KEY: &str = "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ";
    /// The key of 2fa's own tests: `Hello!\xde\xad\xbe\xef`.
    const HELLO: &str = "JBSWY3DPEHPK3PXP";

    fn names(keychain: &Keychain) -> Vec<&str> {
        keychain.keys.iter().map(|key| key.name.as_str()).collect()
    }

    fn add(text: &str, name: &str, digits: u8, secret: &str, counter: Option<u64>) -> Result<String, TwofaError> {
        add_key(text.into(), name.into(), digits, secret.into(), counter)
    }

    #[test]
    fn hotp_matches_rfc_4226() {
        let expected = [
            "755224", "287082", "359152", "969429", "338314", "254676", "287922", "162583", "399871", "520489",
        ];
        for (counter, code) in expected.iter().enumerate() {
            assert_eq!(hotp(RFC_SECRET, counter as u64, 6), *code, "counter {counter}");
        }
    }

    #[test]
    fn totp_matches_rfc_6238() {
        // Appendix B, the SHA-1 rows.
        for (time, code) in [
            (59, "94287082"),
            (1111111109, "07081804"),
            (1111111111, "14050471"),
            (1234567890, "89005924"),
            (2000000000, "69279037"),
            (20000000000, "65353130"),
        ] {
            assert_eq!(totp(RFC_SECRET, time, 8), code, "time {time}");
        }
        // The same instant at the other sizes is the same number, cut shorter.
        assert_eq!(totp(RFC_SECRET, 1111111109, 7), "7081804");
        assert_eq!(totp(RFC_SECRET, 1111111109, 6), "081804");
        // 2fa's own test: second 59 is counter 1.
        assert_eq!(totp(RFC_SECRET, 59, 6), "287082");
    }

    #[test]
    fn the_step_is_thirty_seconds() {
        assert_eq!(totp(b"test_secret", 0, 6), totp(b"test_secret", 29, 6));
        assert_ne!(totp(RFC_SECRET, 29, 6), totp(RFC_SECRET, 30, 6));
        assert_eq!(seconds_remaining(0), 30);
        assert_eq!(seconds_remaining(29), 1);
        assert_eq!(seconds_remaining(30), 30);
        assert_eq!(seconds_remaining(1111111109), 1);
    }

    #[test]
    fn base32_is_read_as_python_reads_it() {
        assert_eq!(decode_base32(HELLO).unwrap(), b"Hello!\xde\xad\xbe\xef");
        assert_eq!(decode_base32("jbswy3dpehpk3pxp").unwrap(), b"Hello!\xde\xad\xbe\xef");
        assert_eq!(decode_base32(RFC_KEY).unwrap(), RFC_SECRET);
        assert_eq!(decode_base32("").unwrap(), b"");
        // RFC 4648 vectors: every amount of padding.
        for (encoded, decoded) in [
            ("MY======", "f"),
            ("MZXQ====", "fo"),
            ("MZXW6===", "foo"),
            ("MZXW6YQ=", "foob"),
            ("MZXW6YTB", "fooba"),
            ("MZXW6YTBOI======", "foobar"),
        ] {
            assert_eq!(decode_base32(encoded).unwrap(), decoded.as_bytes(), "{encoded}");
            assert_eq!(encode_base32(decoded.as_bytes()), encoded);
        }
        for bad in [
            "INVALID1",         // 1 is not in the alphabet
            "MZXW6",            // not padded
            "MZXW6YT=========", // too much padding
            "MZ=W6YTB",         // padding in the middle
            "MZXW6Y==",         // no such amount of padding
            "MZXW6YTB\r",
        ] {
            assert!(decode_base32(bad).is_none(), "{bad:?}");
        }
    }

    #[test]
    fn a_typed_secret_is_cleaned_up() {
        assert_eq!(normalize_secret("jbsw y3dp\tehpk 3pxp\n").unwrap(), HELLO);
        assert_eq!(normalize_secret("mzxw6").unwrap(), "MZXW6===");
        assert_eq!(normalize_secret("MZXW6===").unwrap(), "MZXW6===");
        assert_eq!(normalize_secret("MZXW6=").unwrap(), "MZXW6===");
        for bad in ["", "   ", "====", "INVALID1", "MZX", "é"] {
            assert_eq!(normalize_secret(bad), Err(TwofaError::InvalidSecret), "{bad:?}");
        }
    }

    #[test]
    fn a_keychain_is_read_as_2fa_reads_it() {
        let text = [
            format!("zeta 6 {HELLO}"),
            String::new(),
            format!("counter 7 {RFC_KEY} 00000000000000000004"),
            format!("eight 8 {RFC_KEY}"),
            "lower 6 jbswy3dpehpk3pxp".to_string(),
            "malformed line".to_string(),
            "bad 6 INVALID1".to_string(),
            format!("nine 9 {HELLO}"),
            format!("two  6 {HELLO}"),
            format!("short 6 {HELLO} 123"),
            format!("signed 6 {HELLO} +0000000000000000001"),
            format!("huge 6 {HELLO} 99999999999999999999"),
            format!("extra 6 {HELLO} 00000000000000000000 more"),
            format!("crlf 6 {HELLO}\r"),
            format!("last 6 {HELLO}"),
        ]
        .join("\n");
        let keychain = read_keychain(text, 1111111109);
        assert_eq!(names(&keychain), ["counter", "eight", "last", "lower", "zeta"]);
        let key = |name: &str| keychain.keys.iter().find(|key| key.name == name).unwrap();
        assert_eq!(key("eight").code.as_deref(), Some("07081804"));
        assert_eq!(key("eight").counter, None);
        assert_eq!(key("counter").code, None);
        assert_eq!(key("counter").counter, Some(4));
        assert_eq!(key("counter").digits, 7);
        assert_eq!(key("lower").code, key("zeta").code);
        let skipped: Vec<(u32, &str)> = keychain.skipped.iter().map(|s| (s.line, s.reason.as_str())).collect();
        let not_a_key = "not a key: expected `name digits key`, separated by one space";
        assert_eq!(
            skipped,
            [
                (6, not_a_key),
                (7, "malformed key"),
                (8, not_a_key),
                (9, not_a_key),
                (10, "malformed counter: expected 20 digits"),
                (11, "malformed counter: expected 20 digits"),
                (12, "counter out of range"),
                (13, "too many fields"),
                (14, "malformed key"),
            ]
        );
        assert_eq!(read_keychain(String::new(), 0).keys, []);
    }

    #[test]
    fn a_name_defined_twice_is_its_last_definition() {
        let text = format!("a 6 {HELLO}\na 8 {RFC_KEY}\n");
        let keychain = read_keychain(text.clone(), 59);
        assert_eq!(keychain.keys.len(), 1);
        assert_eq!(keychain.keys[0].code.as_deref(), Some("94287082"));
        // Removing it removes both: the first must not come back.
        assert_eq!(remove_key(text, "a".into()).unwrap(), "");
    }

    #[test]
    fn keys_are_added_as_2fa_adds_them() {
        let text = add("", "github", 6, "jbsw y3dp ehpk 3pxp", None).unwrap();
        assert_eq!(text, "github 6 JBSWY3DPEHPK3PXP\n");
        let text = add(&text, "aws/prod", 8, HELLO, None).unwrap();
        let text = add(&text, "bank", 7, HELLO, Some(0)).unwrap();
        assert_eq!(
            text,
            "github 6 JBSWY3DPEHPK3PXP\naws/prod 8 JBSWY3DPEHPK3PXP\nbank 7 JBSWY3DPEHPK3PXP 00000000000000000000\n"
        );
        // A secret that is not a multiple of 8 characters is padded, as rsc/2fa writes it.
        assert_eq!(add("", "a", 6, "mzxw6", None).unwrap(), "a 6 MZXW6===\n");
        // The line break 2fa forgets.
        assert_eq!(
            add(&format!("a 6 {HELLO}"), "b", 6, HELLO, None).unwrap(),
            format!("a 6 {HELLO}\nb 6 {HELLO}\n")
        );

        assert_eq!(
            add(&text, "bank", 6, HELLO, None),
            Err(TwofaError::DuplicateName { name: "bank".into() })
        );
        assert_eq!(add(&text, "a", 5, HELLO, None), Err(TwofaError::InvalidDigits));
        assert_eq!(add(&text, "a", 9, HELLO, None), Err(TwofaError::InvalidDigits));
        assert_eq!(add(&text, "a", 6, "nope!", None), Err(TwofaError::InvalidSecret));
        for bad in ["", "a b", "a\nb 6 AAAAAAAA", "-add", "été", "a.b", "a:b"] {
            let result = add(&text, bad, 6, HELLO, None);
            assert!(matches!(result, Err(TwofaError::InvalidName { .. })), "{bad:?}");
        }
    }

    #[test]
    fn the_counter_goes_up_in_place() {
        let before = format!("a 6 {HELLO}\nhotp 6 {RFC_KEY} 00000000000000000000\nz 6 {HELLO}\n");
        let first = next_hotp(before.clone(), "hotp".into()).unwrap();
        // A new key gives the code of counter 1, not 0: as 2fa does.
        assert_eq!((first.code.as_str(), first.counter), ("287082", 1));
        assert_eq!(
            first.text,
            before.replace("00000000000000000000", "00000000000000000001")
        );
        let second = next_hotp(first.text, "hotp".into()).unwrap();
        assert_eq!((second.code.as_str(), second.counter), ("359152", 2));
        assert_eq!(read_keychain(second.text, 0).keys[1].counter, Some(2));

        assert_eq!(
            next_hotp(before.clone(), "a".into()),
            Err(TwofaError::NotCounterBased { name: "a".into() })
        );
        assert_eq!(
            next_hotp(before, "b".into()),
            Err(TwofaError::NoSuchKey { name: "b".into() })
        );

        // The largest counter there is: used once, then no further.
        let last = format!("hotp 8 {RFC_KEY} {}", u64::MAX - 1);
        let step = next_hotp(last, "hotp".into()).unwrap();
        assert_eq!(step.counter, u64::MAX);
        assert_eq!(step.text, format!("hotp 8 {RFC_KEY} {}", u64::MAX));
        assert_eq!(
            next_hotp(step.text, "hotp".into()),
            Err(TwofaError::CounterExhausted { name: "hotp".into() })
        );
    }

    #[test]
    fn remove_and_rename_leave_the_rest_alone() {
        let text = format!("a 6 {HELLO}\n# a note\nhotp 6 {RFC_KEY} 00000000000000000007\n\nz 8 {HELLO}");
        assert_eq!(
            remove_key(text.clone(), "a".into()).unwrap(),
            format!("# a note\nhotp 6 {RFC_KEY} 00000000000000000007\n\nz 8 {HELLO}")
        );
        assert_eq!(
            remove_key(text.clone(), "z".into()).unwrap(),
            format!("a 6 {HELLO}\n# a note\nhotp 6 {RFC_KEY} 00000000000000000007\n\n")
        );
        assert_eq!(
            remove_key(text.clone(), "#".into()),
            Err(TwofaError::NoSuchKey { name: "#".into() })
        );

        let renamed = rename_key(text.clone(), "hotp".into(), "bank/main".into()).unwrap();
        assert_eq!(renamed, text.replace("hotp", "bank/main"));
        // The counter is found where it now is.
        assert_eq!(next_hotp(renamed, "bank/main".into()).unwrap().counter, 8);
        assert_eq!(rename_key(text.clone(), "a".into(), "a".into()).unwrap(), text);
        assert_eq!(
            rename_key(text.clone(), "a".into(), "z".into()),
            Err(TwofaError::DuplicateName { name: "z".into() })
        );
        assert_eq!(
            rename_key(text.clone(), "b".into(), "c".into()),
            Err(TwofaError::NoSuchKey { name: "b".into() })
        );
        let result = rename_key(text, "a".into(), "a b".into());
        assert!(matches!(result, Err(TwofaError::InvalidName { .. })));
    }

    #[test]
    fn import_into_nothing_is_byte_for_byte() {
        let file = format!("b 6 jbswy3dpehpk3pxp\n# kept\r\na 6 {RFC_KEY} 00000000000000000003");
        let outcome = import_keychain(" \n".into(), file.clone());
        assert_eq!(outcome.text, file);
        assert_eq!(outcome.added, ["a", "b"]);
        assert_eq!(outcome.skipped.len(), 1);
        assert_eq!(outcome.skipped[0].line, 2);
    }

    #[test]
    fn import_merges_and_loses_nothing() {
        let ours = [
            format!("same 6 {HELLO}"),
            format!("behind 6 {RFC_KEY} 00000000000000000002"),
            format!("ahead 6 {RFC_KEY} 00000000000000000009"),
            format!("other 6 {HELLO}"),
            format!("kind 6 {HELLO}"),
        ]
        .join("\n");
        let theirs = [
            "same 6 jbswy3dpehpk3pxp".to_string(),
            format!("behind 6 {RFC_KEY} 00000000000000000005"),
            format!("ahead 6 {RFC_KEY} 00000000000000000001"),
            format!("other 6 {RFC_KEY}"),
            format!("kind 6 {HELLO} 00000000000000000000"),
            "new 8 mzxw6===".to_string(),
            format!("newhotp 7 {HELLO} 00000000000000000042"),
            "junk".to_string(),
        ]
        .join("\n");
        let outcome = import_keychain(ours, theirs);
        let expected = [
            format!("same 6 {HELLO}"),
            format!("behind 6 {RFC_KEY} 00000000000000000005"),
            format!("ahead 6 {RFC_KEY} 00000000000000000009"),
            format!("other 6 {HELLO}"),
            format!("kind 6 {HELLO}"),
            "new 8 MZXW6===".to_string(),
            format!("newhotp 7 {HELLO} 00000000000000000042\n"),
        ]
        .join("\n");
        assert_eq!(outcome.text, expected);
        assert_eq!(outcome.added, ["new", "newhotp"]);
        assert_eq!(outcome.advanced, ["behind"]);
        assert_eq!(outcome.unchanged, ["ahead", "same"]);
        assert_eq!(outcome.conflicts, ["kind", "other"]);
        assert_eq!(outcome.skipped.len(), 1);
        // Importing the result again changes nothing.
        let again = import_keychain(outcome.text.clone(), outcome.text.clone());
        assert_eq!(again.text, outcome.text);
        assert!(again.added.is_empty() && again.advanced.is_empty() && again.conflicts.is_empty());
    }

    #[test]
    fn otpauth_uris_are_read() {
        // The example of the Key Uri Format page.
        let uri = "otpauth://totp/ACME%20Co:john.doe@email.com?secret=HXDMVJECJJWSRB3HWIZR4IFUGFTMXBOZ\
                   &issuer=ACME%20Co&algorithm=SHA1&digits=6&period=30";
        let key = parse_otpauth(uri.into()).unwrap();
        assert_eq!(
            key,
            OtpauthKey {
                suggested_name: "ACME-Co-john-doe-email-com".into(),
                issuer: Some("ACME Co".into()),
                account: "john.doe@email.com".into(),
                secret: "HXDMVJECJJWSRB3HWIZR4IFUGFTMXBOZ".into(),
                digits: 6,
                counter: None,
            }
        );
        // What it suggests is a name that can be added.
        assert!(add("", &key.suggested_name, key.digits, &key.secret, key.counter).is_ok());

        // Lower case, unpadded, no issuer in the label, `+` for a space, unknown parameters.
        let uri = " OTPAUTH://TOTP/alice?SECRET=mzxw6&issuer=My+Bank&image=https%3A%2F%2Fx%2Fy.png\n";
        let key = parse_otpauth(uri.into()).unwrap();
        assert_eq!((key.secret.as_str(), key.digits), ("MZXW6===", 6));
        assert_eq!(key.issuer.as_deref(), Some("My Bank"));
        assert_eq!(key.suggested_name, "My-Bank-alice");

        let uri = "otpauth://hotp/Bank%3A%20%C3%A9lodie?secret=JBSWY3DPEHPK3PXP&counter=41&digits=8";
        let key = parse_otpauth(uri.into()).unwrap();
        assert_eq!((key.digits, key.counter), (8, Some(41)));
        assert_eq!((key.issuer.as_deref(), key.account.as_str()), (Some("Bank"), "élodie"));
        assert_eq!(key.suggested_name, "Bank-lodie");

        let key = parse_otpauth("otpauth://hotp/a?secret=JBSWY3DPEHPK3PXP".into()).unwrap();
        assert_eq!(key.counter, Some(0));
        let key = parse_otpauth("otpauth://totp/?secret=JBSWY3DPEHPK3PXP".into()).unwrap();
        assert_eq!(key.suggested_name, "key");
    }

    #[test]
    fn what_the_file_cannot_say_is_refused() {
        let base = "otpauth://totp/a?secret=JBSWY3DPEHPK3PXP";
        for (uri, message) in [
            (
                format!("{base}&algorithm=SHA256"),
                "not supported: the SHA256 algorithm (only SHA1)",
            ),
            (
                format!("{base}&algorithm=sha512"),
                "not supported: the sha512 algorithm (only SHA1)",
            ),
            (
                format!("{base}&period=60"),
                "not supported: a period of 60 seconds (only 30)",
            ),
            (
                format!("{base}&digits=5"),
                "not supported: codes of 5 digits (only 6, 7 or 8)",
            ),
            (
                format!("{base}&digits=10"),
                "not supported: codes of 10 digits (only 6, 7 or 8)",
            ),
            (format!("{base}&encoder=steam"), "not supported: the steam code format"),
            (
                "otpauth://yaotp/a?secret=JBSWY3DPEHPK3PXP".to_string(),
                "not supported: keys of type `yaotp` (only totp and hotp)",
            ),
        ] {
            let error = parse_otpauth(uri.clone()).unwrap_err();
            assert!(matches!(error, TwofaError::Unsupported { .. }), "{uri}");
            assert_eq!(error_message(error), message);
        }
        let migration = parse_otpauth("otpauth-migration://offline?data=CjEKCkhlbGxvId6tvu8".into());
        assert!(matches!(migration, Err(TwofaError::Unsupported { .. })));
        // SHA1 and 30 seconds, said out loud, are fine.
        assert!(parse_otpauth(format!("{base}&algorithm=sha1&period=30")).is_ok());

        for (uri, message) in [
            (
                "https://example.com/?secret=JBSWY3DPEHPK3PXP",
                "no otpauth:// at the start",
            ),
            ("JBSWY3DPEHPK3PXP", "no otpauth:// at the start"),
            ("otpauth://totp/a", "it has no secret"),
            ("otpauth://totp/a?secret=", "it has no secret"),
            (
                "otpauth://totp/a?secret=JBSWY3DPEHPK3PXP&secret=MZXW6===",
                "`secret` is given twice",
            ),
            ("otpauth://totp/a%zz?secret=JBSWY3DPEHPK3PXP", "bad percent-encoding"),
            ("otpauth://totp/a%+f?secret=JBSWY3DPEHPK3PXP", "bad percent-encoding"),
            ("otpauth://totp/a%f?secret=JBSWY3DPEHPK3PXP", "bad percent-encoding"),
            ("otpauth://totp/a%ff?secret=JBSWY3DPEHPK3PXP", "not UTF-8"),
            (
                "otpauth://hotp/a?secret=JBSWY3DPEHPK3PXP&counter=-1",
                "the counter is not a number",
            ),
            (
                "otpauth://hotp/a?secret=JBSWY3DPEHPK3PXP&counter=%2B1",
                "the counter is not a number",
            ),
        ] {
            assert_eq!(parse_otpauth(uri.into()), Err(invalid_uri(message)), "{uri}");
        }
        assert_eq!(
            parse_otpauth("otpauth://totp/a?secret=not-base32".into()),
            Err(TwofaError::InvalidSecret)
        );
    }

    #[test]
    fn errors_have_a_message_for_people() {
        assert_eq!(
            error_message(TwofaError::NoSuchKey { name: "x".into() }),
            "no such key x"
        );
        assert_eq!(
            error_message(parse_otpauth("nope".into()).unwrap_err()),
            "not a usable otpauth:// URI: no otpauth:// at the start"
        );
    }
}
