//! Against a real companion server: `COMPANION_PY=/path/to/companion.py cargo test -- --ignored`
//! (`make e2e` does it). Each test starts its own server, on its own port and state dir.

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use companion_core::{CompanionClient, CompanionError, ProgressListener, ServerConfig};

const ADMIN_ID: &str = "e2e-admin";
const ADMIN_SECRET: &str = "0123456789abcdef0123456789abcdef";

struct Server {
    child: Child,
    url: String,
    dir: PathBuf,
}

impl Server {
    fn start() -> Self {
        Self::start_with(&[("COMPANION_RATE_LIMIT_MAX", "1000")])
    }

    fn start_with(env: &[(&str, &str)]) -> Self {
        let script = std::env::var("COMPANION_PY").expect("set COMPANION_PY to the path of companion.py");
        let dir = std::env::temp_dir().join(format!("companion-e2e-{}", pinned_http::random_hex(8)));
        std::fs::create_dir_all(&dir).unwrap();
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        // The launcher only matters where there is no IPv6: see its docstring.
        let child = Command::new("python3")
            .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/run_companion.py"))
            .arg(&script)
            .args(["server", "--port", &port.to_string()])
            .env("COMPANION_CONFIG", dir.join("no-config.json"))
            .env("COMPANION_STATE_DIR", dir.join("state"))
            .env("COMPANION_BOOTSTRAP_ADMIN", format!("{ADMIN_ID}:{ADMIN_SECRET}"))
            .envs(env.iter().copied())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("cannot run python3");
        let server = Self {
            child,
            url: format!("http://127.0.0.1:{port}"),
            dir,
        };
        let deadline = Instant::now() + Duration::from_secs(15);
        while ureq::get(&server.url).call().is_err() {
            assert!(Instant::now() < deadline, "the companion server did not start");
            std::thread::sleep(Duration::from_millis(50));
        }
        server
    }

    fn client(&self) -> Arc<CompanionClient> {
        self.client_with_secret(ADMIN_SECRET)
    }

    fn client_with_secret(&self, secret: &str) -> Arc<CompanionClient> {
        CompanionClient::new(ServerConfig {
            url: self.url.clone(),
            client_id: ADMIN_ID.into(),
            client_secret: secret.into(),
            trust: None,
        })
        .unwrap()
    }

    fn path(&self, name: &str) -> String {
        self.dir.join(name).to_string_lossy().into_owned()
    }

    fn write(&self, name: &str, content: &[u8]) -> String {
        std::fs::write(self.dir.join(name), content).unwrap();
        self.path(name)
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[derive(Default)]
struct Recorder {
    calls: AtomicU64,
    last_done: AtomicU64,
    last_total: AtomicU64,
}

impl ProgressListener for Recorder {
    fn on_progress(&self, done: u64, total: u64) {
        assert!(
            done >= self.last_done.swap(done, Ordering::SeqCst),
            "progress goes forward"
        );
        self.last_total.store(total, Ordering::SeqCst);
        self.calls.fetch_add(1, Ordering::SeqCst);
    }
}

#[test]
#[ignore = "needs COMPANION_PY"]
fn upload_list_download() {
    let server = Server::start();
    let client = server.client();
    assert_eq!(client.list_files().unwrap(), vec![]);

    // 3 MB of every byte value, with what a naive multipart parser would trip on.
    let mut content: Vec<u8> = (0..3_000_000u32).map(|i| (i % 251) as u8).collect();
    content.extend_from_slice(b"\r\n--boundary--\r\n\r\n");
    let source = server.write("source.bin", &content);

    let up = Arc::new(Recorder::default());
    let uploaded = client
        .upload_file(source, Some("été \"2026\".pdf".into()), Some(up.clone()))
        .unwrap();
    assert_eq!(uploaded.filename, "été _2026_.pdf");
    assert_eq!(uploaded.size, content.len() as u64);
    assert!(up.calls.load(Ordering::SeqCst) > 1);
    assert_eq!(
        up.last_done.load(Ordering::SeqCst),
        up.last_total.load(Ordering::SeqCst),
        "upload reported to the end"
    );

    let files = client.list_files().unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].id, uploaded.id);
    assert_eq!(files[0].name, "été _2026_.pdf");
    assert_eq!(files[0].size, content.len() as u64);
    assert_eq!(files[0].mimetype, "application/pdf");
    assert_eq!(files[0].uploaded_by, ADMIN_ID);

    let down = Arc::new(Recorder::default());
    let dest = server.path("downloaded.bin");
    let size = client
        .download_file(uploaded.id.clone(), dest.clone(), Some(down.clone()))
        .unwrap();
    assert_eq!(size, content.len() as u64);
    assert!(
        std::fs::read(&dest).unwrap() == content,
        "the file comes back byte for byte"
    );
    assert_eq!(down.last_done.load(Ordering::SeqCst), content.len() as u64);
    assert_eq!(down.last_total.load(Ordering::SeqCst), content.len() as u64);
    assert!(!PathBuf::from(format!("{dest}.part")).exists());
}

#[test]
#[ignore = "needs COMPANION_PY"]
fn upload_names_and_bytes() {
    let server = Server::start();
    let client = server.client();

    // The name of the file itself when none is given.
    let uploaded = client
        .upload_file(server.write("notes.txt", b"hello"), None, None)
        .unwrap();
    assert_eq!((uploaded.filename.as_str(), uploaded.size), ("notes.txt", 5));

    let uploaded = client
        .upload_bytes("pasted.txt".into(), b"from memory".to_vec())
        .unwrap();
    let dest = server.path("pasted.out");
    client.download_file(uploaded.id, dest.clone(), None).unwrap();
    assert_eq!(std::fs::read(dest).unwrap(), b"from memory");

    // An empty file is a file.
    let uploaded = client.upload_bytes("empty".into(), vec![]).unwrap();
    assert_eq!(uploaded.size, 0);
    assert_eq!(
        client
            .download_file(uploaded.id, server.path("empty.out"), None)
            .unwrap(),
        0
    );

    assert_eq!(client.list_files().unwrap().len(), 3);
    assert!(matches!(
        client.upload_file(server.path("missing"), None, None),
        Err(CompanionError::Io { .. })
    ));
}

#[test]
#[ignore = "needs COMPANION_PY"]
fn preview() {
    let server = Server::start();
    let client = server.client();

    let nothing = client.preview_current().unwrap();
    assert_eq!((nothing.file_id, nothing.filename, nothing.timestamp), (None, None, 0));

    let slides = client.upload_bytes("slides.pdf".into(), b"%PDF".to_vec()).unwrap();
    let photo = client.upload_bytes("photo.png".into(), b"png".to_vec()).unwrap();

    let first = client.set_preview(slides.id.clone()).unwrap();
    let state = client.preview_current().unwrap();
    assert_eq!(state.file_id.as_deref(), Some(slides.id.as_str()));
    assert_eq!(state.filename.as_deref(), Some("slides.pdf"));
    assert_eq!(state.mimetype.as_deref(), Some("application/pdf"));
    assert_eq!(state.timestamp, first);

    let second = client.set_preview(photo.id.clone()).unwrap();
    assert!(second > first);
    assert_eq!(client.preview_current().unwrap().filename.as_deref(), Some("photo.png"));

    assert_eq!(
        client.set_preview("00000000-0000-0000-0000-000000000000".into()),
        Err(CompanionError::NotFound)
    );
}

#[test]
#[ignore = "needs COMPANION_PY"]
fn pad() {
    let server = Server::start();
    let client = server.client();
    let empty = client.get_pad().unwrap();
    assert_eq!((empty.content.as_str(), empty.timestamp), ("", 0));

    let text = "line 1\nligne 2 — é\n\"quoted\" \\ 🙂";
    let first = client.set_pad(text.into()).unwrap();
    let pad = client.get_pad().unwrap();
    assert_eq!((pad.content.as_str(), pad.timestamp), (text, first));

    let second = client.set_pad(String::new()).unwrap();
    assert!(second > first);
    assert_eq!(client.get_pad().unwrap().content, "");
}

#[test]
#[ignore = "needs COMPANION_PY"]
fn errors_are_told_apart() {
    let server = Server::start();
    let client = server.client();

    let wrong = server.client_with_secret("ffff");
    assert_eq!(wrong.list_files(), Err(CompanionError::Unauthorized));
    assert_eq!(wrong.set_pad("x".into()), Err(CompanionError::Unauthorized));

    let dest = server.path("nothing.out");
    let unknown = "00000000-0000-0000-0000-000000000000".to_string();
    assert_eq!(
        client.download_file(unknown, dest.clone(), None),
        Err(CompanionError::NotFound)
    );
    assert_eq!(
        client.download_file("../api/files".into(), dest.clone(), None),
        Err(CompanionError::NotFound)
    );
    assert!(!PathBuf::from(&dest).exists() && !PathBuf::from(format!("{dest}.part")).exists());

    // A rotation that was started and not completed blocks the client: say so.
    let started = ureq::post(&format!("{}/api/token/start-client-rotation", server.url))
        .set("Authorization", &format!("Bearer {ADMIN_ID}:{ADMIN_SECRET}"))
        .call()
        .unwrap();
    assert_eq!(started.status(), 200);
    assert_eq!(client.list_files(), Err(CompanionError::RotationInProgress));

    let url = server.url.clone();
    drop(server);
    let gone = CompanionClient::new(ServerConfig {
        url,
        client_id: ADMIN_ID.into(),
        client_secret: ADMIN_SECRET.into(),
        trust: None,
    })
    .unwrap();
    assert!(matches!(gone.list_files(), Err(CompanionError::Network { .. })));
}

#[test]
#[ignore = "needs COMPANION_PY"]
fn rate_limit_and_size_limit() {
    let server = Server::start_with(&[
        ("COMPANION_RATE_LIMIT_MAX", "2"),
        ("COMPANION_MAX_REQUEST_BODY", "1024"),
    ]);
    let client = server.client();
    assert!(matches!(
        client.upload_bytes("big".into(), vec![0; 4096]),
        Err(CompanionError::TooLarge { .. })
    ));
    client.set_pad("1".into()).unwrap();
    assert_eq!(client.set_pad("2".into()), Err(CompanionError::RateLimited));
    assert_eq!(client.get_pad().unwrap().content, "1", "reads are not rate limited");
}
