//! The client against the mock of the Qonto API (`mock/mod.rs`), over plain http on this machine.
//! Each test starts its own server. Nothing here, or anywhere in the tests, talks to the real API.

mod mock;

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use bank_core::{AttachOutcome, BankClient, BankConfig, BankError, ProgressListener, TransactionFilter};
use mock::{Mock, State, UploadFault, JPG, MAIN_ACCOUNT, OTHER_ACCOUNT, PDF, PNG, SECRET, SLUG};

fn client_with_secret(server: &Mock, secret: &str) -> Arc<BankClient> {
    let config = BankConfig {
        organization_slug: SLUG.into(),
        secret_key: secret.into(),
        api_pins: vec![],
        attachments_pins: vec![],
    };
    BankClient::with_base_url(config, server.url.clone(), None).unwrap()
}

fn start() -> (Mock, Arc<BankClient>) {
    let server = mock::serve(State::sample());
    let client = client_with_secret(&server, SECRET);
    (server, client)
}

/// A directory of its own for a test, removed at the end.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("bank-core-{}", pinned_http::random_hex(8)));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn path(&self, name: &str) -> String {
        self.0.join(name).to_string_lossy().into_owned()
    }

    fn write(&self, name: &str, content: &[u8]) -> String {
        std::fs::write(self.0.join(name), content).unwrap();
        self.path(name)
    }

    /// Where attachments are downloaded.
    fn cache(&self) -> String {
        self.path("attachments")
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
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

fn labels(transactions: &[bank_core::Transaction]) -> Vec<&str> {
    transactions.iter().map(|t| t.label.as_str()).collect()
}

fn uuid(n: u32) -> String {
    mock::transaction_uuid(n)
}

#[test]
fn accounts_with_the_request_the_cli_makes() {
    let (server, client) = start();
    let accounts = client.accounts().unwrap();
    assert_eq!(accounts.len(), 2);
    assert_eq!(accounts[0].id, MAIN_ACCOUNT);
    assert_eq!(accounts[0].name.as_deref(), Some("Main account"));
    assert_eq!(accounts[0].iban.as_deref(), Some("FR7616958000010000000000000"));
    assert_eq!(accounts[0].currency.as_deref(), Some("EUR"));
    assert_eq!(accounts[0].balance_cents, Some(1_234_567));
    assert_eq!(accounts[0].authorized_balance_cents, Some(1_200_000));
    assert_eq!(accounts[1].id, OTHER_ACCOUNT);
    assert_eq!(accounts[1].iban, None);

    let state = server.state();
    assert_eq!(state.paths("GET"), ["/v2/organization"]);
    let request = &state.requests[0];
    assert_eq!(
        request.header("authorization"),
        Some("test-org-slug:test-secret-key"),
        "the slug and the key, with no scheme"
    );
    assert_eq!(
        request.header("user-agent"),
        Some(""),
        "as the CLI: an empty User-Agent"
    );
}

#[test]
fn wrong_credentials_are_said_as_the_server_says_them() {
    let server = mock::serve(State::sample());
    let error = client_with_secret(&server, "wrong").accounts().unwrap_err();
    assert_eq!(
        error,
        BankError::Http {
            status: 401,
            message: "HTTP 401 Unauthorized: unauthorized: Invalid credentials".into()
        }
    );
    assert_eq!(
        bank_core::error_message(error),
        "HTTP 401 Unauthorized: unauthorized: Invalid credentials"
    );
}

#[test]
fn every_page_is_fetched() {
    let (server, client) = start();
    server.state().page_size = 2;
    let all = client
        .transactions(MAIN_ACCOUNT.into(), TransactionFilter::default())
        .unwrap();
    assert_eq!(
        labels(&all),
        ["Hotel California", "label 4", "label 3", "label 2", "label 1"]
    );
    let query = format!("bank_account_id={MAIN_ACCOUNT}&per_page=100");
    assert_eq!(
        server.state().paths("GET"),
        [1, 2, 3].map(|page| format!("/v2/transactions?{query}&page={page}"))
    );

    let first = client
        .transactions_page(MAIN_ACCOUNT.into(), TransactionFilter::default(), 1)
        .unwrap();
    assert_eq!(labels(&first.transactions), ["Hotel California", "label 4"]);
    assert_eq!((first.next_page, first.total_count), (Some(2), Some(5)));
    let last = client
        .transactions_page(MAIN_ACCOUNT.into(), TransactionFilter::default(), 3)
        .unwrap();
    assert_eq!(labels(&last.transactions), ["label 1"]);
    assert_eq!(last.next_page, None);

    let other = client
        .transactions(OTHER_ACCOUNT.into(), TransactionFilter::default())
        .unwrap();
    assert_eq!(labels(&other), ["label 6"]);
}

#[test]
fn transactions_are_what_the_api_gives() {
    let (_server, client) = start();
    let all = client
        .transactions(MAIN_ACCOUNT.into(), TransactionFilter::default())
        .unwrap();
    let hotel = &all[0];
    assert_eq!(hotel.transaction_id, "test-org-slug-1-transaction-5");
    assert_eq!(hotel.id, uuid(5));
    assert_eq!((hotel.amount_cents, hotel.currency.as_str()), (9150, "EUR"));
    assert_eq!(
        (hotel.local_amount_cents, hotel.local_currency.as_deref()),
        (9999, Some("USD"))
    );
    assert_eq!(hotel.side, "debit");
    assert_eq!(hotel.emitted_at, "2024-06-15T10:30:00.000Z");
    assert_eq!(hotel.operation_type.as_deref(), Some("card"));
    assert_eq!(hotel.card_last_digits.as_deref(), Some("1234"));
    assert_eq!(hotel.attachment_ids, Vec::<String>::new());

    let income = &all[2];
    assert_eq!((income.side.as_str(), income.amount_cents), ("credit", 250_000));
    assert_eq!(income.reference.as_deref(), Some("INV-2024-001"));
    assert_eq!(income.clean_counterparty_name.as_deref(), Some("A Client"));
    assert_eq!(
        (&income.local_currency, &income.note),
        (&None, &None),
        "null is not there"
    );

    assert_eq!(all[1].attachment_ids.len(), 1);
    assert_eq!(all[3].attachment_ids.len(), 2);
}

#[test]
fn filters_reach_the_request_as_the_cli_sends_them() {
    let (server, client) = start();
    let filter = |with_attachments, from: Option<&str>, to: Option<&str>| TransactionFilter {
        with_attachments,
        emitted_at_from: from.map(String::from),
        emitted_at_to: to.map(String::from),
    };
    let list = |filter| client.transactions(MAIN_ACCOUNT.into(), filter).unwrap();

    // no-invoice, only-invoice
    assert_eq!(
        labels(&list(filter(Some(false), None, None))),
        ["Hotel California", "label 3", "label 1"]
    );
    assert_eq!(labels(&list(filter(Some(true), None, None))), ["label 4", "label 2"]);
    // date=2024, date<2024, date>=2024 with no-invoice
    let (from, to) = ("2024-01-01T00:00:00Z", "2024-12-31T23:59:59Z");
    assert_eq!(
        labels(&list(filter(None, Some(from), Some(to)))),
        ["Hotel California", "label 4", "label 3"]
    );
    assert_eq!(
        labels(&list(filter(None, None, Some("2023-12-31T23:59:59Z")))),
        ["label 2", "label 1"]
    );
    assert_eq!(
        labels(&list(filter(Some(false), Some(from), None))),
        ["Hotel California", "label 3"]
    );

    let tail = format!("bank_account_id={MAIN_ACCOUNT}&per_page=100&page=1");
    assert_eq!(
        server.state().paths("GET"),
        [
            format!("/v2/transactions?with_attachments=false&{tail}"),
            format!("/v2/transactions?with_attachments=true&{tail}"),
            format!("/v2/transactions?emitted_at_from={from}&emitted_at_to={to}&{tail}"),
            format!("/v2/transactions?emitted_at_to=2023-12-31T23:59:59Z&{tail}"),
            format!("/v2/transactions?with_attachments=false&emitted_at_from={from}&{tail}"),
        ]
    );

    // What is wrong with a filter is said before anything is sent.
    let sent = server.state().requests.len();
    for bad in [
        filter(None, Some("2024"), None),
        filter(None, Some("2025-01-01T00:00:00Z"), Some(to)),
        filter(None, Some("2024-01-01T00:00:00Z&page=9"), None),
    ] {
        let result = client.transactions(MAIN_ACCOUNT.into(), bad);
        assert!(matches!(result, Err(BankError::InvalidInput { .. })));
    }
    let result = client.transactions("../organization".into(), TransactionFilter::default());
    assert!(matches!(result, Err(BankError::InvalidInput { .. })));
    assert_eq!(server.state().requests.len(), sent);
}

#[test]
fn next_page_must_be_a_later_page() {
    let (server, client) = start();
    for next_page in [serde_json::json!(1), serde_json::json!(0), serde_json::json!("2")] {
        server.state().next_page_override = Some(next_page.clone());
        let error = client
            .transactions(MAIN_ACCOUNT.into(), TransactionFilter::default())
            .unwrap_err();
        assert_eq!(
            bank_core::error_message(error),
            format!("Unexpected answer: Unexpected next page after page 1: {next_page}")
        );
    }
}

#[test]
fn answers_that_are_not_what_is_expected_are_errors() {
    let (server, client) = start();
    // An account that is not there: the error of the API, as it says it.
    let unknown = "99999999-9999-9999-9999-999999999999";
    let error = client
        .transactions(unknown.into(), TransactionFilter::default())
        .unwrap_err();
    assert_eq!(
        error,
        BankError::Http {
            status: 404,
            message: "HTTP 404 Not Found: not_found: Bank account not found".into()
        }
    );
    // A transaction without what is always there.
    server.state().transactions[0] =
        serde_json::json!({"id": uuid(9), "bank_account_id": OTHER_ACCOUNT, "emitted_at": "2024-01-01T00:00:00.000Z"});
    let error = client
        .transactions(OTHER_ACCOUNT.into(), TransactionFilter::default())
        .unwrap_err();
    assert!(matches!(error, BankError::UnexpectedAnswer { .. }), "{error:?}");
}

#[test]
fn a_redirect_is_not_followed() {
    // Somewhere else must never get the credentials: this second server would record them.
    let elsewhere = mock::serve(State::sample());
    let (server, client) = start();
    server.state().redirect_to = Some(format!("{}/v2/organization", elsewhere.url));
    let error = client.accounts().unwrap_err();
    assert_eq!(
        error,
        BankError::Http {
            status: 302,
            message: "HTTP 302 Found".into()
        }
    );
    assert!(elsewhere.state().requests.is_empty());
}

#[test]
fn nothing_answers() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
    drop(listener);
    let config = BankConfig {
        organization_slug: SLUG.into(),
        secret_key: SECRET.into(),
        api_pins: vec![],
        attachments_pins: vec![],
    };
    let error = BankClient::with_base_url(config, url, None)
        .unwrap()
        .accounts()
        .unwrap_err();
    assert!(matches!(error, BankError::Network { .. }), "{error:?}");
    assert!(bank_core::error_message(error).starts_with("Connection error: "));
}

#[test]
fn attachments_are_listed_downloaded_and_hashed() {
    let (server, client) = start();
    let scratch = Scratch::new();
    assert_eq!(client.attachments(uuid(5)).unwrap(), vec![]);
    let attachments = client.attachments(uuid(2)).unwrap();
    assert_eq!(attachments.len(), 2);
    assert_eq!(attachments[0].file_name.as_deref(), Some("file.pdf"));
    assert_eq!(attachments[1].file_content_type.as_deref(), Some("image/png"));
    assert!(attachments[0].url.as_deref().unwrap().starts_with(&server.url));

    let progress = Arc::new(Recorder::default());
    let pdf = client
        .download_attachment(attachments[0].clone(), scratch.cache(), Some(progress.clone()))
        .unwrap();
    assert_eq!(pdf.path, format!("{}/{}.pdf", scratch.cache(), attachments[0].id));
    assert_eq!(std::fs::read(&pdf.path).unwrap(), PDF);
    assert_eq!(
        (pdf.size, pdf.sha256.as_str()),
        (PDF.len() as u64, pinned_http::sha256_hex(PDF).as_str())
    );
    assert_eq!(pdf.mimetype.as_deref(), Some("application/pdf"));
    assert_eq!(progress.last_done.load(Ordering::SeqCst), PDF.len() as u64);
    assert_eq!(progress.last_total.load(Ordering::SeqCst), PDF.len() as u64);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&pdf.path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "financial documents are for this user only");
    }

    let png = client
        .download_attachment(attachments[1].clone(), scratch.cache(), None)
        .unwrap();
    assert!(png.path.ends_with(".png"));
    assert_eq!(png.sha256, pinned_http::sha256_hex(PNG));

    // Already there: hashed again, not fetched again, even when its URL is of no use anymore.
    let mut expired = attachments[0].clone();
    expired.url = None;
    assert_eq!(client.download_attachment(expired, scratch.cache(), None).unwrap(), pdf);

    let state = server.state();
    let downloads: Vec<_> = state.requests.iter().filter(|r| r.path.starts_with("/s3/")).collect();
    assert_eq!(downloads.len(), 2);
    for request in downloads {
        assert_eq!(
            request.header("authorization"),
            None,
            "the storage never gets the credentials"
        );
        assert_eq!(request.header("user-agent"), Some(""));
    }
    let mut left: Vec<_> = std::fs::read_dir(scratch.cache())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    left.sort();
    assert_eq!(
        left,
        [
            format!("{}.pdf", attachments[0].id),
            format!("{}.png", attachments[1].id)
        ],
        "no partial file is left"
    );
}

#[test]
fn a_download_that_fails_leaves_nothing() {
    let (server, client) = start();
    let scratch = Scratch::new();
    let attachment = client.attachments(uuid(4)).unwrap().remove(0);

    server.state().storage_down = true;
    let error = client
        .download_attachment(attachment.clone(), scratch.cache(), None)
        .unwrap_err();
    assert_eq!(
        error,
        BankError::Http {
            status: 403,
            message: "HTTP 403 Forbidden: <?xml version=\"1.0\"?> <Error> <Code>AccessDenied</Code> </Error>".into()
        }
    );
    assert!(!std::path::Path::new(&scratch.cache()).exists());

    // What is not an URL of the kind that is expected is not fetched.
    let sent = server.state().requests.len();
    for url in [
        None,
        Some("file:///etc/passwd"),
        Some("ftp://127.0.0.1/x"),
        Some("/s3/x"),
    ] {
        let mut odd = attachment.clone();
        odd.url = url.map(String::from);
        let error = client.download_attachment(odd, scratch.cache(), None).unwrap_err();
        assert_eq!(bank_core::error_message(error), "Unexpected answer: No download URL");
    }
    assert_eq!(server.state().requests.len(), sent);

    // An id that is not a file name does not leave the directory.
    server.state().storage_down = false;
    let mut hostile = attachment;
    hostile.id = "../../outside".into();
    let local = client.download_attachment(hostile, scratch.cache(), None).unwrap();
    let name = pinned_http::sha256_hex(b"../../outside");
    assert_eq!(local.path, format!("{}/{name}.pdf", scratch.cache()));
}

#[test]
fn a_file_is_attached_once() {
    let (server, client) = start();
    let scratch = Scratch::new();
    // Its name says PDF: what it is, is a PNG.
    let receipt = scratch.write("receipt.pdf", PNG);
    let digest = pinned_http::sha256_hex(PNG);

    let progress = Arc::new(Recorder::default());
    let outcome = client
        .attach_file(uuid(5), receipt.clone(), false, scratch.cache(), Some(progress.clone()))
        .unwrap();
    let AttachOutcome::Uploaded {
        sha256,
        idempotency_key,
    } = outcome
    else {
        panic!("not sent: {outcome:?}");
    };
    assert_eq!(sha256, digest);
    assert!(progress.calls.load(Ordering::SeqCst) >= 1);
    assert_eq!(
        progress.last_done.load(Ordering::SeqCst),
        progress.last_total.load(Ordering::SeqCst),
        "upload reported to the end"
    );
    {
        let state = server.state();
        let stored = state.attachments_of(5);
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].content, PNG);
        assert_eq!(
            (stored[0].file_name.as_str(), stored[0].content_type.as_str()),
            ("file.png", "image/png")
        );
        assert_eq!(stored[0].idempotency_key.as_ref(), Some(&idempotency_key));
        let post = state.requests.iter().find(|r| r.method == "POST").unwrap();
        assert_eq!(post.path, format!("/v2/transactions/{}/attachments", uuid(5)));
        assert_eq!(post.header("authorization"), Some("test-org-slug:test-secret-key"));
        assert_eq!(post.header("user-agent"), Some(""));
        // The body, byte for byte as bank.py's MultiPartForm writes it.
        let boundary = post
            .header("content-type")
            .unwrap()
            .strip_prefix("multipart/form-data; boundary=")
            .unwrap();
        assert_eq!(boundary.len(), 32);
        let mut expected = format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"file.png\"\r\n\
             Content-Type: image/png\r\n\r\n"
        )
        .into_bytes();
        expected.extend_from_slice(PNG);
        expected.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
        assert_eq!(post.body, expected);
    }
    // python3 -c 'from uuid import *; from hashlib import sha256; print(uuid5(NAMESPACE_URL,
    //   "bank:justify:00000000-0000-0000-0000-000000000005:" + sha256(PNG).hexdigest()))'
    assert_eq!(idempotency_key, "14307ea3-8036-5ddc-928b-6c0525385867");

    // Again: Qonto is asked what the transaction has, and has it. Nothing is sent.
    let again = client
        .attach_file(uuid(5), receipt.clone(), false, scratch.cache(), None)
        .unwrap();
    assert_eq!(again, AttachOutcome::AlreadyThere { sha256: digest.clone() });
    assert_eq!(server.state().paths("POST").len(), 1);

    // Another transaction, that has other files but not this one: sent, with another key.
    let other = client
        .attach_file(uuid(4), receipt.clone(), false, scratch.cache(), None)
        .unwrap();
    let AttachOutcome::Uploaded {
        idempotency_key: other_key,
        ..
    } = other
    else {
        panic!("not sent: {other:?}");
    };
    assert_ne!(other_key, idempotency_key);
    assert_eq!(server.state().attachments_of(4).len(), 2);

    // force: sent again without asking anything, as a new attachment.
    let gets = server.state().paths("GET").len();
    let forced = client
        .attach_file(uuid(5), receipt, true, scratch.cache(), None)
        .unwrap();
    let AttachOutcome::Uploaded {
        idempotency_key: forced_key,
        ..
    } = forced
    else {
        panic!("not sent: {forced:?}");
    };
    assert_ne!(forced_key, idempotency_key);
    assert_eq!(server.state().attachments_of(5).len(), 2);
    assert_eq!(server.state().paths("GET").len(), gets);
}

#[test]
fn a_file_that_was_received_without_an_answer_is_not_sent_again() {
    let (server, client) = start();
    let scratch = Scratch::new();
    let receipt = scratch.write("photo", JPG);
    server.state().upload_fault = Some(UploadFault::AttachedButFails);
    let error = client
        .attach_file(uuid(5), receipt.clone(), false, scratch.cache(), None)
        .unwrap_err();
    assert_eq!(
        bank_core::error_message(error),
        "HTTP 500 Internal Server Error: internal_server_error: Something went wrong"
    );
    // Qonto had received it: only the answer was lost.
    let again = client
        .attach_file(uuid(5), receipt, false, scratch.cache(), None)
        .unwrap();
    assert!(matches!(again, AttachOutcome::AlreadyThere { .. }), "{again:?}");
    let state = server.state();
    assert_eq!(state.paths("POST").len(), 1);
    assert_eq!(state.attachments_of(5).len(), 1);
}

#[test]
fn sending_again_after_a_failure_uses_the_same_key() {
    let (server, client) = start();
    let scratch = Scratch::new();
    let receipt = scratch.write("invoice.pdf", b"%PDF-1.4 another invoice");
    let xml = "<Error><Code>SlowDown</Code></Error>".to_string();
    server.state().upload_fault = Some(UploadFault::Answer(500, "application/xml", xml));
    let error = client
        .attach_file(uuid(5), receipt.clone(), false, scratch.cache(), None)
        .unwrap_err();
    assert_eq!(
        bank_core::error_message(error),
        "HTTP 500 Internal Server Error: <Error><Code>SlowDown</Code></Error>"
    );
    assert_eq!(server.state().attachments_of(5).len(), 0);

    let outcome = client
        .attach_file(uuid(5), receipt, false, scratch.cache(), None)
        .unwrap();
    assert!(matches!(outcome, AttachOutcome::Uploaded { .. }));
    let state = server.state();
    let keys: Vec<_> = state
        .requests
        .iter()
        .filter(|r| r.method == "POST")
        .map(|r| r.header("x-qonto-idempotency-key").unwrap().to_string())
        .collect();
    assert_eq!(keys.len(), 2);
    assert_eq!(keys[0], keys[1]);
    assert_eq!(state.attachments_of(5).len(), 1);
}

#[test]
fn nothing_is_sent_when_what_the_transaction_has_is_not_known() {
    let (server, client) = start();
    let scratch = Scratch::new();
    let receipt = scratch.write("invoice.pdf", b"%PDF-1.4 another invoice");

    // Transaction 4 has an attachment, that cannot be downloaded to be compared.
    server.state().storage_down = true;
    let error = client
        .attach_file(uuid(4), receipt.clone(), false, scratch.cache(), None)
        .unwrap_err();
    assert!(matches!(error, BankError::NotChecked { .. }), "{error:?}");
    let message = bank_core::error_message(error);
    assert!(message.starts_with("Could not check what this transaction already has, nothing sent: HTTP 403"));
    // A transaction that is not there: the API says so, and nothing is sent either.
    let error = client
        .attach_file(uuid(9), receipt.clone(), false, scratch.cache(), None)
        .unwrap_err();
    assert!(bank_core::error_message(error).contains("nothing sent: HTTP 404 Not Found"));
    assert_eq!(server.state().paths("POST").len(), 0);

    // A transaction with no attachment needs no download to be checked.
    let outcome = client
        .attach_file(uuid(5), receipt.clone(), false, scratch.cache(), None)
        .unwrap();
    assert!(matches!(outcome, AttachOutcome::Uploaded { .. }));
    // force asks nothing.
    let outcome = client
        .attach_file(uuid(4), receipt, true, scratch.cache(), None)
        .unwrap();
    assert!(matches!(outcome, AttachOutcome::Uploaded { .. }));
    assert_eq!(server.state().attachments_of(4).len(), 2);
}

#[test]
fn what_cannot_be_an_attachment_is_not_sent() {
    let (server, client) = start();
    let scratch = Scratch::new();
    let attach = |id: String, path: String| client.attach_file(id, path, true, scratch.cache(), None).unwrap_err();

    let gif = scratch.write("receipt.pdf", b"GIF89a");
    assert_eq!(attach(uuid(5), gif), BankError::UnsupportedFile);
    let empty = scratch.write("empty.pdf", b"");
    assert_eq!(attach(uuid(5), empty), BankError::UnsupportedFile);
    let missing = attach(uuid(5), scratch.path("missing.pdf"));
    assert!(bank_core::error_message(missing).starts_with("Could not read the file: "));
    let pdf = scratch.write("invoice.pdf", PDF);
    for id in ["", "transaction-5", "../../organization", &format!("{}/../x", uuid(5))] {
        assert!(
            matches!(attach(id.into(), pdf.clone()), BankError::InvalidInput { .. }),
            "{id:?}"
        );
        assert!(matches!(
            client.attachments(id.into()),
            Err(BankError::InvalidInput { .. })
        ));
    }
    assert!(server.state().requests.is_empty());
}

#[test]
fn an_upload_is_only_done_when_the_api_says_so() {
    let (server, client) = start();
    let scratch = Scratch::new();
    let receipt = scratch.write("invoice.pdf", PDF);
    for (content_type, body, expected) in [
        (
            "application/json",
            r#"{"attachment": "?"}"#,
            r#"the upload was answered with {"attachment":"?"}"#,
        ),
        ("application/json", "[]", "the upload was answered with []"),
        ("text/html", "<html>", "expected value at line 1 column 1"),
    ] {
        server.state().upload_fault = Some(UploadFault::Answer(200, content_type, body.into()));
        let error = client
            .attach_file(uuid(5), receipt.clone(), true, scratch.cache(), None)
            .unwrap_err();
        assert_eq!(
            bank_core::error_message(error),
            format!("Unexpected answer: {expected}")
        );
    }
}
