//! Against the real `2fa`: `TWOFA_PY=/path/to/2fa.py cargo test -p twofa-core -- --ignored`.
//! Each test has its own keychain file, which `2fa.py` is pointed at with `CONFIG_2FA_SECRETS`
//! (an absolute path there replaces the home directory it is otherwise relative to).

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use twofa_core::{add_key, next_hotp, read_keychain};

const HELLO: &str = "JBSWY3DPEHPK3PXP";
const RFC_KEY: &str = "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ";

/// A directory of its own, with the path of a keychain file in it. Removed when dropped.
struct Home(PathBuf);

impl Home {
    fn new(test: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("twofa-cross-check-{}-{test}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn file(&self) -> PathBuf {
        self.0.join("keychain")
    }

    fn read(&self) -> String {
        std::fs::read_to_string(self.file()).unwrap()
    }

    /// Runs `2fa.py <args>`, answering its prompt with `input`. Returns what it printed.
    fn run(&self, args: &[&str], input: &str) -> String {
        let script = std::env::var("TWOFA_PY").expect("set TWOFA_PY to the path of 2fa.py");
        assert!(Path::new(&script).is_file(), "no 2fa.py at {script}");
        let mut child = Command::new("python3")
            .arg(&script)
            .args(args)
            .env("HOME", &self.0)
            .env("CONFIG_2FA_SECRETS", self.file())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("cannot run python3");
        child.stdin.take().unwrap().write_all(input.as_bytes()).unwrap();
        let output = child.wait_with_output().unwrap();
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(output.status.success(), "2fa.py {args:?} failed: {stderr}");
        String::from_utf8(output.stdout).unwrap()
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs()
}

/// `2fa.py` reads the clock itself: runs it until it started and ended within one
/// 30 second step, and returns its output with a time of that step.
fn within_one_step(run: impl Fn() -> String) -> (String, u64) {
    for _ in 0..3 {
        let before = now();
        let output = run();
        if before / 30 == now() / 30 {
            return (output, before);
        }
    }
    panic!("three runs in a row crossed a 30 second boundary");
}

#[test]
#[ignore = "needs 2fa.py: set TWOFA_PY"]
fn python_reads_what_rust_wrote() {
    let home = Home::new("rust-wrote");
    let mut text = String::new();
    for (name, digits, secret, counter) in [
        ("six", 6, HELLO, None),
        ("seven", 7, "gezd gnbv gy3t qojq gezd gnbv gy3t qojq", None),
        ("eight", 8, RFC_KEY, None),
        ("unpadded", 6, "mzxw6ytboi", None), // 2fa.py reads it because it is written padded
        ("aws/prod=1", 6, HELLO, None),
        ("counter", 6, RFC_KEY, Some(0)),
        ("counter8", 8, HELLO, Some(41)),
    ] {
        text = add_key(text, name.into(), digits, secret.into(), counter).unwrap();
    }
    std::fs::write(home.file(), &text).unwrap();

    // `2fa.py` with no argument: `code name` for every key, sorted by name, dashes for the counter-based.
    let (output, time) = within_one_step(|| home.run(&[], ""));
    let theirs: Vec<(String, String)> = output
        .lines()
        .map(|line| {
            let (code, name) = line.split_once(' ').unwrap();
            (name.trim().to_string(), code.to_string())
        })
        .collect();
    let ours: Vec<(String, String)> = read_keychain(text.clone(), time)
        .keys
        .into_iter()
        .map(|key| {
            let code = key.code.unwrap_or_else(|| "-".repeat(key.digits.into()));
            (key.name, code)
        })
        .collect();
    assert_eq!(ours.len(), 7);
    assert_eq!(ours, theirs);

    // Asking for one key, time-based.
    let (output, time) = within_one_step(|| home.run(&["aws/prod=1"], ""));
    let keychain = read_keychain(text.clone(), time);
    let key = keychain.keys.iter().find(|key| key.name == "aws/prod=1").unwrap();
    assert_eq!(key.code.as_deref(), Some(output.trim()));

    // Counter-based: the same codes, and the same file after each, byte for byte.
    for name in ["counter", "counter8", "counter"] {
        let step = next_hotp(text, name.into()).unwrap();
        assert_eq!(home.run(&[name], "").trim(), step.code, "{name}");
        assert_eq!(home.read(), step.text, "{name}");
        text = step.text;
    }
}

#[test]
#[ignore = "needs 2fa.py: set TWOFA_PY"]
fn rust_reads_what_python_wrote() {
    let home = Home::new("python-wrote");
    home.run(&["-add", "six"], "jbsw y3dp ehpk 3pxp\n");
    home.run(&["-add", "-7", "seven"], &format!("{RFC_KEY}\n"));
    home.run(&["-add", "-8", "eight"], &format!("{HELLO}\n"));
    home.run(&["-add", "-hotp", "counter"], &format!("{RFC_KEY}\n"));
    home.run(&["-add", "-hotp", "-8", "counter8"], &format!("{HELLO}\n"));
    let text = home.read();

    // Adding the same keys here writes the same file.
    let mut ours = String::new();
    for (name, digits, secret, counter) in [
        ("six", 6, HELLO, None),
        ("seven", 7, RFC_KEY, None),
        ("eight", 8, HELLO, None),
        ("counter", 6, RFC_KEY, Some(0)),
        ("counter8", 8, HELLO, Some(0)),
    ] {
        ours = add_key(ours, name.into(), digits, secret.into(), counter).unwrap();
    }
    assert_eq!(ours, text);

    let keychain = read_keychain(text.clone(), 0);
    assert!(keychain.skipped.is_empty());
    let read: Vec<(&str, u8, Option<u64>)> = keychain
        .keys
        .iter()
        .map(|key| (key.name.as_str(), key.digits, key.counter))
        .collect();
    assert_eq!(
        read,
        [
            ("counter", 6, Some(0)),
            ("counter8", 8, Some(0)),
            ("eight", 8, None),
            ("seven", 7, None),
            ("six", 6, None),
        ]
    );

    for name in ["six", "seven", "eight"] {
        let (output, time) = within_one_step(|| home.run(&[name], ""));
        let keychain = read_keychain(text.clone(), time);
        let key = keychain.keys.iter().find(|key| key.name == name).unwrap();
        assert_eq!(key.code.as_deref(), Some(output.trim()), "{name}");
    }

    // The two take turns on one counter: 2fa.py, here, 2fa.py.
    let first = home.run(&["counter"], "");
    assert_eq!(first.trim(), "287082"); // RFC 4226, counter 1
    let step = next_hotp(home.read(), "counter".into()).unwrap();
    assert_eq!((step.code.as_str(), step.counter), ("359152", 2));
    std::fs::write(home.file(), &step.text).unwrap();
    assert_eq!(home.run(&["counter"], "").trim(), "969429");
    assert_eq!(next_hotp(home.read(), "counter".into()).unwrap().counter, 4);
}
