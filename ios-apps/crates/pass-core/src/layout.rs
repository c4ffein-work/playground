//! The layout of a password store on disk, as `pass` defines it: one `<name>.gpg`
//! per entry, `.gpg-id` files naming the recipients of a directory and of everything
//! below it, the first line of an entry being the password.

use std::path::{Path, PathBuf};

use rand::Rng;
use zeroize::Zeroizing;

use crate::{Entry, EntryField, PassError};

pub(crate) const GPG_ID: &str = ".gpg-id";

fn io_error(context: &str, e: std::io::Error) -> PassError {
    PassError::Io {
        message: format!("{context}: {e}"),
    }
}

/// Checks an entry name and returns it as `pass` would use it: `Email/work`.
///
/// Stricter than `pass`, which only refuses `..`: a name cannot be absolute, have an
/// empty or hidden component (`.git/config` is not an entry), or hold control characters.
pub(crate) fn clean_name(name: &str) -> Result<String, PassError> {
    let invalid = |why: &str| PassError::InvalidName {
        message: format!("{name:?} is not a valid entry name: {why}"),
    };
    let trimmed = name.trim_end_matches('/');
    if trimmed.is_empty() {
        return Err(invalid("it is empty"));
    }
    if trimmed.chars().any(|c| c.is_control() || c == '\\') {
        return Err(invalid("control characters and backslashes are not allowed"));
    }
    for component in trimmed.split('/') {
        if component.is_empty() {
            return Err(invalid("a part of the path is empty"));
        }
        if component.starts_with('.') {
            return Err(invalid("no part of the path may start with a dot"));
        }
        if component.trim() != component {
            return Err(invalid("no part of the path may start or end with a space"));
        }
    }
    Ok(trimmed.to_string())
}

/// `Email/work` → `Email/work.gpg`, relative to the store.
pub(crate) fn file_of(name: &str) -> String {
    format!("{name}.gpg")
}

/// Every entry, sorted: `Email/work`, `bank`... Hidden files and directories are not
/// entries (`.git`, `.extensions`, `.gpg-id`), a symbolic link to a directory is not followed.
pub(crate) fn list(root: &Path) -> Result<Vec<String>, PassError> {
    fn walk(dir: &Path, prefix: &str, out: &mut Vec<String>) -> std::io::Result<()> {
        for item in std::fs::read_dir(dir)? {
            let item = item?;
            let Ok(file_name) = item.file_name().into_string() else {
                continue;
            };
            if file_name.starts_with('.') {
                continue;
            }
            let kind = item.file_type()?;
            if kind.is_dir() {
                walk(&item.path(), &format!("{prefix}{file_name}/"), out)?;
            } else if let Some(name) = file_name.strip_suffix(".gpg") {
                if !name.is_empty() && item.path().is_file() {
                    out.push(format!("{prefix}{name}"));
                }
            }
        }
        Ok(())
    }
    let mut names = Vec::new();
    walk(root, "", &mut names).map_err(|e| io_error("cannot read the store", e))?;
    names.sort();
    Ok(names)
}

/// The entries whose name holds every word of `query`, whatever the case.
/// (`pass find` matches any of its words: on a phone, each word typed narrows the list.)
pub(crate) fn search(names: &[String], query: &str) -> Vec<String> {
    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    names
        .iter()
        .filter(|name| {
            let name = name.to_lowercase();
            words.iter().all(|word| name.contains(word))
        })
        .cloned()
        .collect()
}

/// The ids in a `.gpg-id` file: one per line, `#` starts a comment.
pub(crate) fn parse_gpg_id(content: &str) -> Vec<String> {
    content
        .lines()
        .map(|line| line.split('#').next().unwrap_or("").trim())
        .filter(|id| !id.is_empty())
        .map(str::to_string)
        .collect()
}

/// The `.gpg-id` that governs an entry: the one in its directory, or in the nearest
/// directory above. Returned relative to the store (`.gpg-id`, `work/.gpg-id`).
pub(crate) fn governing_gpg_id(root: &Path, name: &str) -> Result<String, PassError> {
    let mut components: Vec<&str> = name.split('/').collect();
    components.pop();
    loop {
        let relative = if components.is_empty() {
            GPG_ID.to_string()
        } else {
            format!("{}/{GPG_ID}", components.join("/"))
        };
        if root.join(&relative).is_file() {
            return Ok(relative);
        }
        if components.pop().is_none() {
            return Err(PassError::NotInitialized);
        }
    }
}

pub(crate) fn read(root: &Path, relative: &str) -> Result<Vec<u8>, PassError> {
    std::fs::read(root.join(relative)).map_err(|e| io_error(&format!("cannot read {relative}"), e))
}

/// Writes a file of the store so that it is there whole or not at all, creating its directories.
pub(crate) fn write(root: &Path, relative: &str, content: &[u8]) -> Result<(), PassError> {
    let path = root.join(relative);
    let context = format!("cannot write {relative}");
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| io_error(&context, e))?;
    }
    let mut partial = path.clone().into_os_string();
    partial.push(format!(".tmp.{}", pinned_http::random_hex(6)));
    let partial = PathBuf::from(partial);
    std::fs::write(&partial, content)
        .and_then(|()| std::fs::rename(&partial, &path))
        .map_err(|e| {
            let _ = std::fs::remove_file(&partial);
            io_error(&context, e)
        })
}

/// Removes a file of the store, and the directories it leaves empty.
pub(crate) fn remove(root: &Path, relative: &str) -> Result<(), PassError> {
    let path = root.join(relative);
    match std::fs::remove_file(&path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(io_error(&format!("cannot remove {relative}"), e)),
    }
    let mut dir = path.parent();
    while let Some(current) = dir {
        // Fails on the first directory that still holds something: that is the stop condition.
        if current == root || std::fs::remove_dir(current).is_err() {
            break;
        }
        dir = current.parent();
    }
    Ok(())
}

/// Reads decrypted content the way `pass` users lay it out: the password on the
/// first line, then anything. `key: value` lines are fields, `otpauth://` lines are
/// one-time-password secrets (the `pass-otp` convention), wherever they are.
pub(crate) fn parse_entry(plaintext: &[u8]) -> Entry {
    let text = Zeroizing::new(String::from_utf8_lossy(plaintext).into_owned());
    let (password, extra) = match text.split_once('\n') {
        Some((first, rest)) => (first, rest),
        None => (text.as_str(), ""),
    };
    let password = password.strip_suffix('\r').unwrap_or(password);
    let mut fields = Vec::new();
    let mut otp_uris = Vec::new();
    for line in text.lines().map(str::trim) {
        if line.starts_with("otpauth://") {
            otp_uris.push(line.to_string());
        }
    }
    for line in extra.lines().map(str::trim) {
        if line.starts_with("otpauth://") {
            continue;
        }
        if let Some((name, value)) = line.split_once(':') {
            let (name, value) = (name.trim(), value.trim());
            // `https://example.org` is a line of notes, not a field named https.
            if !name.is_empty() && !value.is_empty() && !value.starts_with("//") {
                fields.push(EntryField {
                    name: name.to_string(),
                    value: value.to_string(),
                });
            }
        }
    }
    Entry {
        password: if password.starts_with("otpauth://") {
            String::new()
        } else {
            password.to_string()
        },
        extra: extra.to_string(),
        fields,
        otp_uris,
    }
}

/// `content` with its first line replaced: what `pass generate --in-place` does.
pub(crate) fn replace_first_line(content: &str, password: &str) -> Zeroizing<String> {
    Zeroizing::new(match content.split_once('\n') {
        Some((_, rest)) => format!("{password}\n{rest}"),
        None => format!("{password}\n"),
    })
}

/// The characters `pass generate` draws from: `[:punct:][:alnum:]`, or `[:alnum:]` with `--no-symbols`.
fn character_set(symbols: bool) -> Vec<u8> {
    (b'!'..=b'~').filter(|c| symbols || c.is_ascii_alphanumeric()).collect()
}

pub(crate) const MAX_GENERATED_LENGTH: u32 = 1024;

/// A random password, from the system's generator, every character equally likely.
pub(crate) fn generate_password(length: u32, symbols: bool) -> Result<Zeroizing<String>, PassError> {
    if length == 0 || length > MAX_GENERATED_LENGTH {
        return Err(PassError::InvalidName {
            message: format!("the length of a generated password is 1 to {MAX_GENERATED_LENGTH}"),
        });
    }
    let characters = character_set(symbols);
    let mut rng = rand::rngs::OsRng;
    let mut password = Zeroizing::new(String::with_capacity(length as usize));
    for _ in 0..length {
        password.push(characters[rng.gen_range(0..characters.len())] as char);
    }
    Ok(password)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_stay_inside_the_store() {
        assert_eq!(clean_name("Email/work").unwrap(), "Email/work");
        assert_eq!(clean_name("Email/work/").unwrap(), "Email/work");
        assert_eq!(clean_name("été/mot de passe").unwrap(), "été/mot de passe");
        for bad in [
            "",
            "/",
            "/etc/passwd",
            "..",
            "../x",
            "a/../b",
            "a//b",
            ".git/config",
            "a/.gpg-id",
            "a/./b",
            "a\nb",
            "a\\b",
            " a",
            "a /b",
            "a\0b",
        ] {
            assert!(matches!(clean_name(bad), Err(PassError::InvalidName { .. })), "{bad:?}");
        }
    }

    #[test]
    fn gpg_id_lines() {
        assert_eq!(
            parse_gpg_id("ABCD1234 # laptop\n\n  alice@example.org  \n# nobody\r\n0xDEADBEEF\n"),
            ["ABCD1234", "alice@example.org", "0xDEADBEEF"]
        );
        // pass.py writes it with no final newline.
        assert_eq!(parse_gpg_id("A\nB"), ["A", "B"]);
        assert!(parse_gpg_id("").is_empty());
    }

    #[test]
    fn the_nearest_gpg_id_governs() {
        let root = std::env::temp_dir().join(format!("pass-layout-{}", pinned_http::random_hex(8)));
        std::fs::create_dir_all(root.join("work/deep")).unwrap();
        assert!(matches!(governing_gpg_id(&root, "a"), Err(PassError::NotInitialized)));
        std::fs::write(root.join(".gpg-id"), "A").unwrap();
        std::fs::write(root.join("work/.gpg-id"), "B").unwrap();
        assert_eq!(governing_gpg_id(&root, "a").unwrap(), ".gpg-id");
        assert_eq!(governing_gpg_id(&root, "home/a").unwrap(), ".gpg-id");
        assert_eq!(governing_gpg_id(&root, "work/a").unwrap(), "work/.gpg-id");
        assert_eq!(governing_gpg_id(&root, "work/deep/er/a").unwrap(), "work/.gpg-id");
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn listing_and_search() {
        let root = std::env::temp_dir().join(format!("pass-layout-{}", pinned_http::random_hex(8)));
        for file in [
            "bank.gpg",
            "Email/work.gpg",
            "Email/Perso.gpg",
            ".git/objects/x.gpg",
            ".extensions/y.gpg",
            "notes.txt",
            ".gpg",
            ".hidden.gpg",
        ] {
            write(&root, file, b"x").unwrap();
        }
        let names = list(&root).unwrap();
        assert_eq!(names, ["Email/Perso", "Email/work", "bank"]);
        assert_eq!(search(&names, "email"), ["Email/Perso", "Email/work"]);
        assert_eq!(search(&names, "PERSO email"), ["Email/Perso"]);
        assert_eq!(search(&names, "  "), names);
        assert!(search(&names, "email bank").is_empty());

        remove(&root, "Email/work.gpg").unwrap();
        assert!(root.join("Email").is_dir(), "still holds Perso");
        remove(&root, "Email/Perso.gpg").unwrap();
        assert!(!root.join("Email").exists(), "an emptied directory goes");
        assert!(root.is_dir());
        remove(&root, "Email/Perso.gpg").unwrap();
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn entries_are_read_as_pass_users_write_them() {
        let entry = parse_entry(
            b"hunter2\nlogin: alice\nurl: https://example.org/login\nhttps://example.org\n\
              otpauth://totp/Example:alice?secret=JBSWY3DPEHPK3PXP\nfree text\n",
        );
        assert_eq!(entry.password, "hunter2");
        assert_eq!(
            entry.fields,
            [
                EntryField {
                    name: "login".into(),
                    value: "alice".into()
                },
                EntryField {
                    name: "url".into(),
                    value: "https://example.org/login".into()
                },
            ]
        );
        assert_eq!(entry.otp_uris, ["otpauth://totp/Example:alice?secret=JBSWY3DPEHPK3PXP"]);
        assert!(entry.extra.starts_with("login: alice\n"));
        assert!(entry.extra.ends_with("free text\n"));

        // pass.py's `insert` writes no final newline, gpg on Windows may write \r\n.
        assert_eq!(parse_entry(b"hunter2").password, "hunter2");
        assert_eq!(parse_entry(b"hunter2").extra, "");
        assert_eq!(parse_entry(b"hunter2\r\nlogin: a\r\n").password, "hunter2");
        assert_eq!(parse_entry(b"").password, "");
        // An entry that is only an OTP secret has no password.
        let otp_only = parse_entry(b"otpauth://totp/x?secret=JBSWY3DPEHPK3PXP\n");
        assert_eq!(otp_only.password, "");
        assert_eq!(otp_only.otp_uris.len(), 1);
    }

    #[test]
    fn generated_passwords() {
        let with_symbols = generate_password(400, true).unwrap();
        assert_eq!(with_symbols.len(), 400);
        assert!(with_symbols.bytes().all(|c| c.is_ascii_graphic()));
        assert!(with_symbols.bytes().any(|c| c.is_ascii_punctuation()));
        let plain = generate_password(400, false).unwrap();
        assert!(plain.bytes().all(|c| c.is_ascii_alphanumeric()));
        assert_ne!(
            *generate_password(25, true).unwrap(),
            *generate_password(25, true).unwrap()
        );
        assert!(generate_password(0, true).is_err());
        assert!(generate_password(MAX_GENERATED_LENGTH + 1, true).is_err());

        assert_eq!(*replace_first_line("old\nlogin: a\n", "new"), "new\nlogin: a\n");
        assert_eq!(*replace_first_line("old", "new"), "new\n");
    }
}
