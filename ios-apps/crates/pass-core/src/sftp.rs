//! The little of SFTP (version 3, draft-ietf-secsh-filexfer-02, what OpenSSH speaks)
//! that a drop zone needs: read one file whole, with a size limit, and publish one
//! by writing it beside its name and renaming it over. Written out here rather than
//! taken from a crate: it is a few requests, and the limits are ours to set.
//!
//! One request at a time, each answered before the next is sent.

use std::io::{Read, Write};

use crate::ssh::{Input, Outcome, Output, SshRemote};
use crate::PassError;

const INIT: u8 = 1;
const VERSION: u8 = 2;
const OPEN: u8 = 3;
const CLOSE: u8 = 4;
const READ: u8 = 5;
const WRITE: u8 = 6;
const FSTAT: u8 = 8;
const REMOVE: u8 = 13;
const RENAME: u8 = 18;
const STATUS: u8 = 101;
const HANDLE: u8 = 102;
const DATA: u8 = 103;
const ATTRS: u8 = 105;
const EXTENDED: u8 = 200;

const STATUS_OK: u32 = 0;
const STATUS_EOF: u32 = 1;
const STATUS_NO_SUCH_FILE: u32 = 2;
const STATUS_PERMISSION_DENIED: u32 = 3;

const OPEN_READ: u32 = 0x01;
const OPEN_WRITE: u32 = 0x02;
const OPEN_CREATE: u32 = 0x08;
const OPEN_TRUNCATE: u32 = 0x10;
const ATTR_SIZE: u32 = 0x01;

/// What every server accepts in one read or one write.
const CHUNK: usize = 32 * 1024;
/// No answer of the protocol is bigger than a data chunk and its envelope.
const MAX_PACKET: usize = CHUNK + 1024;
const POSIX_RENAME: &str = "posix-rename@openssh.com";

fn broken(what: &str) -> PassError {
    PassError::Network {
        message: format!("the drop zone does not answer as an SFTP server: {what}"),
    }
}

fn put_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_be_bytes());
}

fn put_string(out: &mut Vec<u8>, value: &[u8]) {
    put_u32(out, value.len() as u32);
    out.extend_from_slice(value);
}

/// Reads the fields of an answer, in order.
struct Fields<'a>(&'a [u8]);

impl<'a> Fields<'a> {
    fn take(&mut self, count: usize) -> Result<&'a [u8], PassError> {
        if self.0.len() < count {
            return Err(broken("an answer is cut short"));
        }
        let (head, rest) = self.0.split_at(count);
        self.0 = rest;
        Ok(head)
    }

    fn u32(&mut self) -> Result<u32, PassError> {
        Ok(u32::from_be_bytes(self.take(4)?.try_into().expect("4 bytes")))
    }

    fn u64(&mut self) -> Result<u64, PassError> {
        Ok(u64::from_be_bytes(self.take(8)?.try_into().expect("8 bytes")))
    }

    fn string(&mut self) -> Result<&'a [u8], PassError> {
        let length = self.u32()? as usize;
        self.take(length)
    }
}

/// An answer: its type and what follows the request id.
struct Answer {
    kind: u8,
    body: Vec<u8>,
}

/// A status answer that is not a success.
fn refusal(body: &[u8], doing: &str) -> PassError {
    let mut fields = Fields(body);
    let code = fields.u32().unwrap_or(u32::MAX);
    let said = fields
        .string()
        .map(|text| String::from_utf8_lossy(text).into_owned())
        .unwrap_or_default();
    match code {
        STATUS_PERMISSION_DENIED => PassError::Network {
            message: format!("the drop zone does not allow to {doing}"),
        },
        _ => PassError::Network {
            message: format!("the drop zone cannot {doing}: {said} (status {code})"),
        },
    }
}

/// An SFTP session with a drop zone.
pub(crate) struct Sftp {
    output: Output,
    input: Input,
    outcome: Outcome,
    next_id: u32,
    posix_rename: bool,
}

impl Sftp {
    /// Connects (the host key must be the pinned one), authenticates with the device's key, starts SFTP.
    pub(crate) fn connect(remote: &SshRemote) -> Result<Self, PassError> {
        let (output, input) = remote.subsystem("sftp")?;
        let mut sftp = Self {
            outcome: output.outcome(),
            output,
            input,
            next_id: 0,
            posix_rename: false,
        };
        let mut init = vec![INIT];
        put_u32(&mut init, 3);
        sftp.send(&init)?;
        let packet = sftp.receive()?;
        let mut fields = Fields(&packet);
        if fields.take(1)? != [VERSION] {
            return Err(broken("no version"));
        }
        if fields.u32()? != 3 {
            return Err(broken("it is not version 3"));
        }
        // Then the extensions it has, as name and value pairs.
        while !fields.0.is_empty() {
            let name = fields.string()?;
            fields.string()?;
            sftp.posix_rename |= name == POSIX_RENAME.as_bytes();
        }
        Ok(sftp)
    }

    fn failed(&self, e: impl std::fmt::Display) -> PassError {
        self.outcome.failure().unwrap_or_else(|| PassError::Network {
            message: format!("the connection to the drop zone broke: {e}"),
        })
    }

    fn send(&mut self, packet: &[u8]) -> Result<(), PassError> {
        let mut framed = Vec::with_capacity(packet.len() + 4);
        put_u32(&mut framed, packet.len() as u32);
        framed.extend_from_slice(packet);
        self.input.write_all(&framed).map_err(|e| self.failed(e))
    }

    fn receive(&mut self) -> Result<Vec<u8>, PassError> {
        let mut length = [0u8; 4];
        self.output.read_exact(&mut length).map_err(|e| self.failed(e))?;
        let length = u32::from_be_bytes(length) as usize;
        if length == 0 || length > MAX_PACKET {
            return Err(broken("an answer of an impossible size"));
        }
        let mut packet = vec![0u8; length];
        self.output.read_exact(&mut packet).map_err(|e| self.failed(e))?;
        Ok(packet)
    }

    /// Sends one request (its type, then `body` after the request id) and reads its answer.
    fn request(&mut self, kind: u8, body: &[u8]) -> Result<Answer, PassError> {
        self.next_id = self.next_id.wrapping_add(1);
        let mut packet = vec![kind];
        put_u32(&mut packet, self.next_id);
        packet.extend_from_slice(body);
        self.send(&packet)?;
        let answer = self.receive()?;
        let mut fields = Fields(&answer);
        let kind = fields.take(1)?[0];
        if fields.u32()? != self.next_id {
            return Err(broken("an answer to another request"));
        }
        Ok(Answer {
            kind,
            body: fields.0.to_vec(),
        })
    }

    /// A request that is answered by a status: `Ok(code)` for the codes in `accepted`, an error otherwise.
    fn status(&mut self, kind: u8, body: &[u8], accepted: &[u32], doing: &str) -> Result<u32, PassError> {
        let answer = self.request(kind, body)?;
        if answer.kind != STATUS {
            return Err(broken("no status"));
        }
        let code = Fields(&answer.body).u32()?;
        if accepted.contains(&code) {
            Ok(code)
        } else {
            Err(refusal(&answer.body, doing))
        }
    }

    /// Opens a file. `Ok(None)` when there is no such file.
    fn open(&mut self, name: &str, flags: u32, doing: &str) -> Result<Option<Vec<u8>>, PassError> {
        let mut body = Vec::new();
        put_string(&mut body, name.as_bytes());
        put_u32(&mut body, flags);
        put_u32(&mut body, 0); // no attributes: the server's own (the drop zone forces 0600)
        let answer = self.request(OPEN, &body)?;
        match answer.kind {
            HANDLE => Ok(Some(Fields(&answer.body).string()?.to_vec())),
            STATUS if Fields(&answer.body).u32()? == STATUS_NO_SUCH_FILE => Ok(None),
            STATUS => Err(refusal(&answer.body, doing)),
            _ => Err(broken("neither a handle nor a status")),
        }
    }

    fn close(&mut self, handle: &[u8], doing: &str) -> Result<(), PassError> {
        let mut body = Vec::new();
        put_string(&mut body, handle);
        self.status(CLOSE, &body, &[STATUS_OK], doing).map(|_| ())
    }

    /// Reads a file whole. `Ok(None)` when there is no such file. A file over `limit` bytes
    /// is refused, by the size the server gives for it and again by what it actually sends.
    pub(crate) fn download(&mut self, name: &str, limit: u64) -> Result<Option<Vec<u8>>, PassError> {
        let doing = format!("read {name}");
        let Some(handle) = self.open(name, OPEN_READ, &doing)? else {
            return Ok(None);
        };
        let mut body = Vec::new();
        put_string(&mut body, &handle);
        let attributes = self.request(FSTAT, &body)?;
        if attributes.kind == ATTRS {
            let mut fields = Fields(&attributes.body);
            if fields.u32()? & ATTR_SIZE != 0 && fields.u64()? > limit {
                let _ = self.close(&handle, &doing);
                return Err(PassError::TooLarge);
            }
        }
        let mut content = Vec::new();
        loop {
            let mut body = Vec::new();
            put_string(&mut body, &handle);
            body.extend_from_slice(&(content.len() as u64).to_be_bytes());
            put_u32(&mut body, CHUNK as u32);
            let answer = self.request(READ, &body)?;
            match answer.kind {
                DATA => {
                    let data = Fields(&answer.body).string()?;
                    if data.is_empty() {
                        return Err(broken("an empty read"));
                    }
                    content.extend_from_slice(data);
                    if content.len() as u64 > limit {
                        let _ = self.close(&handle, &doing);
                        return Err(PassError::TooLarge);
                    }
                }
                STATUS if Fields(&answer.body).u32()? == STATUS_EOF => break,
                STATUS => return Err(refusal(&answer.body, &doing)),
                _ => return Err(broken("neither data nor a status")),
            }
        }
        self.close(&handle, &doing)?;
        Ok(Some(content))
    }

    /// Publishes a file: uploaded as `<name>.part`, then renamed over `<name>`, so that
    /// a reader finds the previous file or the new one, never part of one.
    pub(crate) fn publish(&mut self, name: &str, content: &[u8]) -> Result<(), PassError> {
        let doing = format!("write {name}");
        let part = format!("{name}.part");
        let flags = OPEN_WRITE | OPEN_CREATE | OPEN_TRUNCATE;
        let handle = self.open(&part, flags, &doing)?.ok_or_else(|| PassError::Network {
            message: format!("the drop zone cannot {doing}: no such directory"),
        })?;
        let mut offset = 0u64;
        for chunk in content.chunks(CHUNK) {
            let mut body = Vec::new();
            put_string(&mut body, &handle);
            body.extend_from_slice(&offset.to_be_bytes());
            put_string(&mut body, chunk);
            self.status(WRITE, &body, &[STATUS_OK], &doing)?;
            offset += chunk.len() as u64;
        }
        self.close(&handle, &doing)?;

        let mut names = Vec::new();
        put_string(&mut names, part.as_bytes());
        put_string(&mut names, name.as_bytes());
        if self.posix_rename {
            let mut body = Vec::new();
            put_string(&mut body, POSIX_RENAME.as_bytes());
            body.extend_from_slice(&names);
            self.status(EXTENDED, &body, &[STATUS_OK], &doing)?;
        } else {
            // The plain rename of SFTP 3 refuses to replace a file: remove it first.
            let mut target = Vec::new();
            put_string(&mut target, name.as_bytes());
            self.status(REMOVE, &target, &[STATUS_OK, STATUS_NO_SUCH_FILE], &doing)?;
            self.status(RENAME, &names, &[STATUS_OK], &doing)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fields_are_read_in_order_and_never_past_the_end() {
        let mut packet = Vec::new();
        put_u32(&mut packet, 7);
        put_string(&mut packet, b"posix-rename@openssh.com");
        packet.extend_from_slice(&9u64.to_be_bytes());
        let mut fields = Fields(&packet);
        assert_eq!(fields.u32().unwrap(), 7);
        assert_eq!(fields.string().unwrap(), b"posix-rename@openssh.com");
        assert_eq!(fields.u64().unwrap(), 9);
        assert!(fields.u32().is_err());

        // A length that promises more than there is.
        let mut lying = Vec::new();
        put_u32(&mut lying, 1000);
        lying.extend_from_slice(b"short");
        assert!(Fields(&lying).string().is_err());
    }

    #[test]
    fn refusals_say_what_was_being_done() {
        let mut body = Vec::new();
        put_u32(&mut body, STATUS_PERMISSION_DENIED);
        put_string(&mut body, b"Permission denied");
        assert_eq!(
            refusal(&body, "write phone.bundle.gpg").to_string(),
            "the drop zone does not allow to write phone.bundle.gpg"
        );
        let mut body = Vec::new();
        put_u32(&mut body, 4);
        put_string(&mut body, b"Failure");
        assert_eq!(
            refusal(&body, "write x").to_string(),
            "the drop zone cannot write x: Failure (status 4)"
        );
    }
}
