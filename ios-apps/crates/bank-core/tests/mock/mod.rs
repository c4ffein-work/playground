//! A mock of the Qonto API: the endpoints `bank.py` calls, answering with the shapes `bank.py` and its
//! `test.py` expect. Derived from that CLI, NOT from Qonto's documentation: where the CLI does not say
//! (the fields of a bank account besides `id`, of an attachment besides `id` and `url`), it is a guess.
//!
//! Used in-process by the Rust tests (over http, and over TLS in `tls.rs`), and as a program by the Swift
//! tests (`examples/mock_qonto.rs`, started by `apps/bank/swift-test.sh`).
//!
//! - `GET  /v2/organization`
//! - `GET  /v2/transactions?[with_attachments=][&emitted_at_from=][&emitted_at_to=]&bank_account_id=&per_page=&page=`
//! - `GET  /v2/transactions/<uuid>/attachments`
//! - `POST /v2/transactions/<uuid>/attachments` (multipart, `X-Qonto-Idempotency-Key`)
//! - `GET  /s3/<attachment id>?...X-Amz-Signature=...`: the pre-signed URLs, on the same server
#![allow(dead_code)]

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex, MutexGuard};

use serde_json::{json, Value};

pub const SLUG: &str = "test-org-slug";
pub const SECRET: &str = "test-secret-key";
pub const MAIN_ACCOUNT: &str = "12345678-1234-1234-1234-123456789abc";
pub const OTHER_ACCOUNT: &str = "12345678-1234-1234-1234-123456789abd";

pub const PDF: &[u8] = b"%PDF-1.7\n%mock invoice\n";
pub const JPG: &[u8] = b"\xff\xd8\xff\xe0\x00\x10JFIF\x00mock photo";
pub const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDRmock scan";

#[derive(Debug, Clone)]
pub struct Request {
    pub method: String,
    /// With its query, as it was sent.
    pub path: String,
    /// Names in lowercase.
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Request {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(n, _)| n == name).map(|(_, v)| v.as_str())
    }
}

#[derive(Debug, Clone)]
pub struct StoredAttachment {
    pub id: String,
    pub content: Vec<u8>,
    pub file_name: String,
    pub content_type: String,
    pub idempotency_key: Option<String>,
}

/// What goes wrong with the next upload.
pub enum UploadFault {
    /// The file is attached, but the answer is an error: what a lost answer looks like to the client.
    AttachedButFails,
    /// Nothing is attached: this status and body are answered.
    Answer(u16, &'static str, String),
}

pub struct State {
    pub slug: String,
    pub secret: String,
    pub accounts: Vec<Value>,
    /// Newest first, each with the `bank_account_id` it belongs to.
    pub transactions: Vec<Value>,
    /// By the id (UUID) of a transaction.
    pub attachments: HashMap<String, Vec<StoredAttachment>>,
    /// Every request that was received, oldest first.
    pub requests: Vec<Request>,
    /// At most this many transactions per page, whatever `per_page` asks.
    pub page_size: usize,
    pub upload_fault: Option<UploadFault>,
    /// `/s3/` answers 403, as an expired pre-signed URL does.
    pub storage_down: bool,
    /// Answered as `meta.next_page` instead of the real one.
    pub next_page_override: Option<Value>,
    /// Every API call is answered with a redirect to there.
    pub redirect_to: Option<String>,
    attachment_counter: u64,
}

/// A transaction as the API returns it: `test.py`'s `transaction(n)`, plus what is not always there.
pub fn transaction(n: u32, account: &str, emitted_at: &str, more: Value) -> Value {
    let mut transaction = json!({
        "transaction_id": format!("{SLUG}-1-transaction-{n}"),
        "id": transaction_uuid(n),
        "label": format!("label {n}"),
        "amount": f64::from(n),
        "amount_cents": 100 * n,
        "local_amount_cents": 100 * n,
        "currency": "EUR",
        "side": "debit",
        "emitted_at": emitted_at,
        "bank_account_id": account,
    });
    for (key, value) in more.as_object().unwrap() {
        transaction[key] = value.clone();
    }
    transaction
}

pub fn transaction_uuid(n: u32) -> String {
    format!("00000000-0000-0000-0000-{n:012}")
}

impl State {
    /// Two accounts. The main one has transactions 1 to 5 (5 is the newest, 1 and 2 are from 2023),
    /// the other one has transaction 6. Transaction 4 has a PDF attached, 2 a PDF and a PNG.
    pub fn sample() -> Self {
        let mut state = Self {
            slug: SLUG.into(),
            secret: SECRET.into(),
            accounts: vec![
                json!({
                    "id": MAIN_ACCOUNT, "slug": format!("{SLUG}-bank-account-1"), "name": "Main account",
                    "iban": "FR7616958000010000000000000", "bic": "QNTOFRP1XXX", "currency": "EUR",
                    "balance": 12345.67, "balance_cents": 1234567,
                    "authorized_balance": 12000.0, "authorized_balance_cents": 1200000,
                    "status": "active", "main": true,
                }),
                json!({
                    "id": OTHER_ACCOUNT, "name": "Taxes", "currency": "EUR",
                    "balance_cents": 5, "authorized_balance_cents": 5, "status": "active",
                }),
            ],
            transactions: vec![
                transaction(6, OTHER_ACCOUNT, "2024-07-01T09:00:00.000Z", json!({"side": "credit"})),
                transaction(
                    5,
                    MAIN_ACCOUNT,
                    "2024-06-15T10:30:00.000Z",
                    json!({
                        "label": "Hotel California", "amount_cents": 9150, "local_amount_cents": 9999,
                        "local_currency": "USD", "operation_type": "card", "card_last_digits": "1234",
                        "status": "completed", "settled_at": "2024-06-17T00:00:00.000Z",
                    }),
                ),
                transaction(
                    4,
                    MAIN_ACCOUNT,
                    "2024-03-01T08:00:00.000Z",
                    json!({"note": "paid by card"}),
                ),
                transaction(
                    3,
                    MAIN_ACCOUNT,
                    "2024-01-01T00:00:00.000Z",
                    json!({
                        "side": "credit", "amount_cents": 250000, "local_amount_cents": 250000,
                        "operation_type": "income", "clean_counterparty_name": "A Client",
                        "reference": "INV-2024-001", "local_currency": null, "note": null,
                    }),
                ),
                transaction(2, MAIN_ACCOUNT, "2023-12-31T23:59:59.000Z", json!({})),
                transaction(1, MAIN_ACCOUNT, "2023-05-05T12:00:00.000Z", json!({})),
            ],
            attachments: HashMap::new(),
            requests: Vec::new(),
            page_size: 100,
            upload_fault: None,
            storage_down: false,
            next_page_override: None,
            redirect_to: None,
            attachment_counter: 0,
        };
        state.attach(4, PDF, "application/pdf", None);
        state.attach(2, PDF, "application/pdf", None);
        state.attach(2, PNG, "image/png", None);
        state
    }

    /// Attaches a file to transaction `n`, as if it had been uploaded: returns the id of the attachment.
    pub fn attach(&mut self, n: u32, content: &[u8], content_type: &str, idempotency_key: Option<String>) -> String {
        self.attachment_counter += 1;
        let id = format!("a77ac4ed-0000-4000-8000-{:012}", self.attachment_counter);
        let extension = content_type.rsplit('/').next().unwrap_or("bin").replace("jpeg", "jpg");
        self.attachments
            .entry(transaction_uuid(n))
            .or_default()
            .push(StoredAttachment {
                id: id.clone(),
                content: content.to_vec(),
                file_name: format!("file.{extension}"),
                content_type: content_type.into(),
                idempotency_key,
            });
        id
    }

    /// The paths of the requests received with this method, oldest first.
    pub fn paths(&self, method: &str) -> Vec<String> {
        let sent = self.requests.iter().filter(|r| r.method == method);
        sent.map(|r| r.path.clone()).collect()
    }

    pub fn attachments_of(&self, n: u32) -> Vec<StoredAttachment> {
        self.attachments.get(&transaction_uuid(n)).cloned().unwrap_or_default()
    }
}

pub struct Mock {
    pub url: String,
    pub state: Arc<Mutex<State>>,
}

impl Mock {
    pub fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap()
    }
}

/// Serves `state` over plain http on a free port of 127.0.0.1, from a thread, until the process ends.
pub fn serve(state: State) -> Mock {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
    let state = Arc::new(Mutex::new(state));
    serve_on(listener, state.clone(), url.clone(), None);
    Mock { url, state }
}

/// `base_url` is how clients reach this server: what the attachment URLs start with.
pub fn serve_on(
    listener: TcpListener,
    state: Arc<Mutex<State>>,
    base_url: String,
    tls: Option<Arc<rustls::ServerConfig>>,
) {
    std::thread::spawn(move || {
        for sock in listener.incoming() {
            let Ok(mut sock) = sock else { continue };
            let (state, base_url, tls) = (state.clone(), base_url.clone(), tls.clone());
            std::thread::spawn(move || match tls {
                None => handle(&mut sock, &state, &base_url),
                Some(config) => {
                    let Ok(conn) = rustls::ServerConnection::new(config) else {
                        return;
                    };
                    let mut stream = rustls::StreamOwned::new(conn, sock);
                    handle(&mut stream, &state, &base_url);
                    stream.conn.send_close_notify();
                    let _ = stream.flush();
                }
            });
        }
    });
}

fn read_request(stream: &mut impl Read) -> Option<Request> {
    let mut data = Vec::new();
    let mut buf = [0u8; 16 * 1024];
    let head_end = loop {
        if let Some(position) = data.windows(4).position(|w| w == b"\r\n\r\n") {
            break position;
        }
        match stream.read(&mut buf) {
            Ok(0) | Err(_) => return None, // a refused handshake, or a fingerprint probe
            Ok(n) => data.extend_from_slice(&buf[..n]),
        }
    };
    let head = String::from_utf8_lossy(&data[..head_end]).into_owned();
    let mut lines = head.split("\r\n");
    let mut request_line = lines.next()?.split(' ');
    let (method, path) = (request_line.next()?.to_string(), request_line.next()?.to_string());
    let headers: Vec<(String, String)> = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.trim().to_ascii_lowercase(), value.trim().to_string()))
        .collect();
    let length = headers
        .iter()
        .find(|(name, _)| name == "content-length")
        .and_then(|(_, value)| value.parse::<usize>().ok())
        .unwrap_or(0);
    let mut body = data[head_end + 4..].to_vec();
    while body.len() < length {
        match stream.read(&mut buf) {
            Ok(0) | Err(_) => return None,
            Ok(n) => body.extend_from_slice(&buf[..n]),
        }
    }
    Some(Request {
        method,
        path,
        headers,
        body,
    })
}

struct Response {
    status: u16,
    content_type: String,
    body: Vec<u8>,
    location: Option<String>,
}

fn answer_json(status: u16, body: Value) -> Response {
    Response {
        status,
        content_type: "application/json; charset=utf-8".into(),
        body: body.to_string().into_bytes(),
        location: None,
    }
}

/// An error as Qonto answers it: `{"errors": [{"code": ..., "detail": ...}]}`
fn answer_error(status: u16, code: &str, detail: &str) -> Response {
    answer_json(status, json!({"errors": [{"code": code, "detail": detail}]}))
}

/// An error as S3 answers it.
fn answer_s3_error(code: &str) -> Response {
    Response {
        status: 403,
        content_type: "application/xml".into(),
        body: format!("<?xml version=\"1.0\"?>\n<Error>\n  <Code>{code}</Code>\n</Error>").into_bytes(),
        location: None,
    }
}

fn handle<S: Read + Write>(stream: &mut S, state: &Mutex<State>, base_url: &str) {
    let Some(request) = read_request(stream) else { return };
    let response = {
        let mut state = state.lock().unwrap();
        state.requests.push(request.clone());
        route(&mut state, &request, base_url)
    };
    let reason = match response.status {
        200 => "OK",
        302 => "Found",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        422 => "Unprocessable Entity",
        _ => "Internal Server Error",
    };
    let mut head = format!(
        "HTTP/1.1 {} {reason}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n",
        response.status,
        response.content_type,
        response.body.len()
    );
    if let Some(location) = response.location {
        head.push_str(&format!("Location: {location}\r\n"));
    }
    head.push_str("\r\n");
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(&response.body);
    let _ = stream.flush();
}

fn route(state: &mut State, request: &Request, base_url: &str) -> Response {
    let (path, query) = request.path.split_once('?').unwrap_or((&request.path, ""));
    if let Some(attachment_id) = path.strip_prefix("/s3/") {
        return storage(state, request, attachment_id, query);
    }
    if request.header("authorization") != Some(&format!("{}:{}", state.slug, state.secret)) {
        return answer_error(401, "unauthorized", "Invalid credentials");
    }
    if let Some(location) = &state.redirect_to {
        return Response {
            status: 302,
            content_type: "text/plain".into(),
            body: Vec::new(),
            location: Some(location.clone()),
        };
    }
    let transaction_id = path
        .strip_prefix("/v2/transactions/")
        .and_then(|rest| rest.strip_suffix("/attachments"));
    match (request.method.as_str(), path, transaction_id) {
        ("GET", "/v2/organization", _) => answer_json(
            200,
            json!({"organization": {"slug": state.slug, "legal_name": "Mock SAS", "bank_accounts": state.accounts}}),
        ),
        ("GET", "/v2/transactions", _) => transactions(state, query),
        (method, _, Some(id)) if state.transactions.iter().any(|t| t["id"] == id) => match method {
            "GET" => answer_json(200, json!({"attachments": attachments_json(state, id, base_url)})),
            "POST" => upload(state, request, id),
            _ => answer_error(404, "not_found", "No such route"),
        },
        (_, _, Some(_)) => answer_error(404, "not_found", "Transaction not found"),
        _ => answer_error(404, "not_found", "No such route"),
    }
}

fn attachments_json(state: &State, transaction_id: &str, base_url: &str) -> Vec<Value> {
    let attachments = state.attachments.get(transaction_id).map(Vec::as_slice).unwrap_or(&[]);
    attachments
        .iter()
        .map(|a| {
            json!({
                "id": a.id,
                "created_at": "2024-06-18T07:00:00.000Z",
                "file_name": a.file_name,
                "file_size": a.content.len().to_string(),
                "file_content_type": a.content_type,
                "url": format!("{base_url}/s3/{}?X-Amz-Expires=1800&X-Amz-Signature=mock", a.id),
            })
        })
        .collect()
}

fn is_timestamp(value: &str) -> bool {
    let pattern = b"dddd-dd-ddTdd:dd:ddZ";
    value.len() == pattern.len()
        && value
            .bytes()
            .zip(pattern)
            .all(|(b, p)| if *p == b'd' { b.is_ascii_digit() } else { b == *p })
}

fn transactions(state: &State, query: &str) -> Response {
    let bad = |detail: &str| answer_error(400, "bad_request", detail);
    let mut params = HashMap::new();
    for pair in query.split('&').filter(|pair| !pair.is_empty()) {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        let known = [
            "with_attachments",
            "emitted_at_from",
            "emitted_at_to",
            "bank_account_id",
            "per_page",
            "page",
        ];
        if !known.contains(&key) || params.insert(key, value).is_some() {
            return bad(&format!("unexpected parameter: {key}"));
        }
    }
    let Some(account) = params.get("bank_account_id") else {
        return bad("bank_account_id is required");
    };
    if !state.accounts.iter().any(|a| a["id"] == *account) {
        return answer_error(404, "not_found", "Bank account not found");
    }
    let number = |key: &str, default: usize| params.get(key).map_or(Some(default), |v| v.parse().ok());
    let (Some(per_page @ 1..=100), Some(page @ 1..)) = (number("per_page", 100), number("page", 1)) else {
        return bad("per_page is 1 to 100, page starts at 1");
    };
    let with_attachments = match params.get("with_attachments").copied() {
        None => None,
        Some("true") => Some(true),
        Some("false") => Some(false),
        Some(_) => return bad("with_attachments is true or false"),
    };
    for key in ["emitted_at_from", "emitted_at_to"] {
        if params.get(key).is_some_and(|value| !is_timestamp(value)) {
            return bad(&format!("{key} is not a date"));
        }
    }
    // To the second: what is emitted during the last second of a range is in it.
    let second = |timestamp: &str| timestamp[..19].to_string();
    let matching: Vec<Value> = state
        .transactions
        .iter()
        .filter(|t| t["bank_account_id"] == *account)
        .map(|t| {
            let mut t = t.clone();
            let attachments = state.attachments.get(t["id"].as_str().unwrap());
            let ids: Vec<&str> = attachments
                .iter()
                .flat_map(|a| a.iter())
                .map(|a| a.id.as_str())
                .collect();
            t["attachment_ids"] = json!(ids);
            t
        })
        .filter(|t| with_attachments.is_none_or(|with| with != t["attachment_ids"].as_array().unwrap().is_empty()))
        .filter(|t| {
            let emitted = second(t["emitted_at"].as_str().unwrap());
            params.get("emitted_at_from").is_none_or(|from| emitted >= second(from))
                && params.get("emitted_at_to").is_none_or(|to| emitted <= second(to))
        })
        .collect();
    let per_page = per_page.min(state.page_size);
    let total_pages = matching.len().div_ceil(per_page).max(1);
    let next_page = if page < total_pages {
        json!(page + 1)
    } else {
        Value::Null
    };
    answer_json(
        200,
        json!({
            "transactions": matching.iter().skip((page - 1) * per_page).take(per_page).collect::<Vec<_>>(),
            "meta": {
                "current_page": page,
                "next_page": state.next_page_override.clone().unwrap_or(next_page),
                "prev_page": if page > 1 { json!(page - 1) } else { Value::Null },
                "total_pages": total_pages,
                "total_count": matching.len(),
                "per_page": per_page,
            },
        }),
    )
}

/// The one part of a `multipart/form-data` body, as `bank.py`'s MultiPartForm writes it:
/// (the line of its Content-Disposition, its Content-Type, its content).
fn multipart_file(request: &Request) -> Option<(String, String, Vec<u8>)> {
    let boundary = request
        .header("content-type")?
        .strip_prefix("multipart/form-data; boundary=")?;
    let body = request
        .body
        .strip_prefix(format!("--{boundary}\r\n").as_bytes())?
        .strip_suffix(format!("\r\n--{boundary}--\r\n").as_bytes())?;
    let head_end = body.windows(4).position(|w| w == b"\r\n\r\n")?;
    let head = std::str::from_utf8(&body[..head_end]).ok()?;
    let (disposition, content_type) = head.split_once("\r\n")?;
    Some((
        disposition.to_string(),
        content_type.strip_prefix("Content-Type: ")?.to_string(),
        body[head_end + 4..].to_vec(),
    ))
}

fn upload(state: &mut State, request: &Request, transaction_id: &str) -> Response {
    let Some(key) = request.header("x-qonto-idempotency-key").map(String::from) else {
        return answer_error(400, "bad_request", "X-Qonto-Idempotency-Key is required");
    };
    let Some((disposition, content_type, content)) = multipart_file(request) else {
        return answer_error(400, "bad_request", "not the multipart body that is expected");
    };
    let Some(file_name) = disposition
        .strip_prefix("Content-Disposition: form-data; name=\"file\"; filename=\"")
        .and_then(|rest| rest.strip_suffix('"'))
    else {
        return answer_error(400, "bad_request", "the file is expected in the field named file");
    };
    if !["application/pdf", "image/jpeg", "image/png"].contains(&content_type.as_str()) {
        return answer_error(422, "invalid_file", "Only PDF, JPEG and PNG files are accepted");
    }
    let already = state.attachments.get(transaction_id).into_iter().flatten();
    if already.clone().any(|a| a.idempotency_key.as_deref() == Some(&key)) {
        return answer_json(200, json!({})); // A key that was seen: answered without doing anything again
    }
    let fault = state.upload_fault.take();
    if let Some(UploadFault::Answer(status, content_type, body)) = fault {
        return Response {
            status,
            content_type: content_type.into(),
            body: body.into_bytes(),
            location: None,
        };
    }
    state.attachment_counter += 1;
    let stored = StoredAttachment {
        id: format!("a77ac4ed-0000-4000-8000-{:012}", state.attachment_counter),
        content,
        file_name: file_name.to_string(),
        content_type,
        idempotency_key: Some(key),
    };
    state
        .attachments
        .entry(transaction_id.to_string())
        .or_default()
        .push(stored);
    match fault {
        Some(_) => answer_error(500, "internal_server_error", "Something went wrong"),
        None => answer_json(200, json!({})),
    }
}

fn storage(state: &State, request: &Request, attachment_id: &str, query: &str) -> Response {
    if request.method != "GET" || state.storage_down || !query.split('&').any(|p| p == "X-Amz-Signature=mock") {
        return answer_s3_error("AccessDenied");
    }
    match state.attachments.values().flatten().find(|a| a.id == attachment_id) {
        None => answer_s3_error("NoSuchKey"),
        Some(attachment) => Response {
            status: 200,
            content_type: attachment.content_type.clone(),
            body: attachment.content.clone(),
            location: None,
        },
    }
}
