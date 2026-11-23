//! `git push` of one branch, which gitoxide does not have yet: the `receive-pack`
//! side of the git protocol, written out. It is the simple side: read what the
//! remote has, say which reference moves from what to what, send a pack with the
//! objects it lacks, read its verdict.
//!
//! The pack holds whole objects, each deflated on its own, no deltas: a store's
//! commits are a few small files.

use std::collections::HashSet;
use std::io::{Read, Write};

use gix::objs::tree::EntryKind;
use gix::objs::Kind;
use gix::ObjectId;

use crate::git::{ancestors, git_error, Remote};
use crate::PassError;

fn rejected(message: impl Into<String>) -> PassError {
    PassError::PushRejected {
        message: message.into(),
    }
}

fn protocol_error(what: &str) -> PassError {
    PassError::Network {
        message: format!("unexpected answer from the remote's git: {what}"),
    }
}

/// One line of git's wire format: 4 hex digits of length, then the payload. `None` is a flush (`0000`).
fn pkt_line(payload: &[u8]) -> Vec<u8> {
    let mut line = format!("{:04x}", payload.len() + 4).into_bytes();
    line.extend_from_slice(payload);
    line
}

/// Reads one line. `Ok(None)` at a flush, or at the end of the stream when `at_end_too`.
fn read_pkt_line(reader: &mut impl Read) -> Result<Option<Vec<u8>>, PassError> {
    let mut length = [0u8; 4];
    reader
        .read_exact(&mut length)
        .map_err(|e| protocol_error(&format!("it stopped ({e})")))?;
    let length = std::str::from_utf8(&length)
        .ok()
        .and_then(|hex| usize::from_str_radix(hex, 16).ok())
        .ok_or_else(|| protocol_error("not a git answer"))?;
    if length < 4 {
        // 0000 is a flush; 0001 and 0002 are delimiters of protocol v2, which is not spoken here.
        return Ok(None);
    }
    let mut payload = vec![0u8; length - 4];
    reader
        .read_exact(&mut payload)
        .map_err(|e| protocol_error(&format!("it stopped ({e})")))?;
    Ok(Some(payload))
}

/// What a remote says it has, before a push.
#[derive(Debug, Default, PartialEq, Eq)]
struct Advertisement {
    /// Full reference name → what it points to.
    references: Vec<(String, ObjectId)>,
    capabilities: Vec<String>,
}

/// Reads the reference advertisement, up to its flush. Over http it starts with a
/// `# service=` line and a flush of its own, which `skip_service_header` passes.
fn read_advertisement(reader: &mut impl Read, skip_service_header: bool) -> Result<Advertisement, PassError> {
    let mut advertisement = Advertisement::default();
    if skip_service_header {
        match read_pkt_line(reader)? {
            Some(line) if line.starts_with(b"# service=git-receive-pack") => {}
            _ => return Err(protocol_error("no receive-pack service")),
        }
        if read_pkt_line(reader)?.is_some() {
            return Err(protocol_error("no flush after the service line"));
        }
    }
    let mut first = true;
    while let Some(line) = read_pkt_line(reader)? {
        let line = String::from_utf8_lossy(&line).into_owned();
        let line = line.trim_end_matches('\n');
        let (reference, capabilities) = match line.split_once('\0') {
            Some((reference, capabilities)) => (reference, Some(capabilities)),
            None => (line, None),
        };
        if first {
            advertisement.capabilities = capabilities
                .unwrap_or("")
                .split(' ')
                .filter(|c| !c.is_empty())
                .map(str::to_string)
                .collect();
            first = false;
        }
        if let Some(error) = reference.strip_prefix("ERR ") {
            return Err(PassError::Network {
                message: format!("the server says: {error}"),
            });
        }
        let (id, name) = reference
            .split_once(' ')
            .ok_or_else(|| protocol_error("a reference with no name"))?;
        // An empty repository advertises its capabilities on a reference that does not exist.
        if name == "capabilities^{}" {
            continue;
        }
        let id = ObjectId::from_hex(id.as_bytes()).map_err(|_| protocol_error("a reference with a bad id"))?;
        advertisement.references.push((name.to_string(), id));
    }
    Ok(advertisement)
}

/// Reads the report that follows a push: `unpack ok`, then `ok <ref>` or `ng <ref> <reason>`.
fn read_report(reader: &mut impl Read, reference: &str) -> Result<(), PassError> {
    let mut unpacked = false;
    let mut accepted = false;
    while let Some(line) = read_pkt_line(reader)? {
        let line = String::from_utf8_lossy(&line).into_owned();
        let line = line.trim_end_matches('\n');
        if let Some(status) = line.strip_prefix("unpack ") {
            if status != "ok" {
                return Err(rejected(format!("the remote could not store the commits: {status}")));
            }
            unpacked = true;
        } else if let Some(refused) = line.strip_prefix("ng ") {
            let reason = refused.strip_prefix(reference).unwrap_or(refused).trim();
            return Err(rejected(reason));
        } else if line.strip_prefix("ok ") == Some(reference) {
            accepted = true;
        }
    }
    if unpacked && accepted {
        Ok(())
    } else {
        Err(protocol_error("no confirmation of the push"))
    }
}

/// The header of one object in a pack: its type and its size, 4 bits then 7 at a time.
fn pack_entry_header(kind: Kind, size: usize) -> Vec<u8> {
    let kind = match kind {
        Kind::Commit => 1u8,
        Kind::Tree => 2,
        Kind::Blob => 3,
        Kind::Tag => 4,
    };
    let mut size = size as u64;
    let mut byte = (kind << 4) | (size & 0x0f) as u8;
    size >>= 4;
    let mut header = Vec::new();
    while size != 0 {
        header.push(byte | 0x80);
        byte = (size & 0x7f) as u8;
        size >>= 7;
    }
    header.push(byte);
    header
}

/// A version 2 pack of whole objects.
fn write_pack(objects: &[(Kind, Vec<u8>)]) -> Result<Vec<u8>, PassError> {
    let failed = |e: std::io::Error| git_error("cannot build the pack", e);
    let mut pack = Vec::new();
    pack.extend_from_slice(b"PACK");
    pack.extend_from_slice(&2u32.to_be_bytes());
    pack.extend_from_slice(&(objects.len() as u32).to_be_bytes());
    for (kind, data) in objects {
        pack.extend_from_slice(&pack_entry_header(*kind, data.len()));
        let mut deflate = gix::zlib::stream::deflate::Write::new(&mut pack, gix::zlib::Compression::DEFAULT);
        deflate.write_all(data).map_err(failed)?;
        deflate.flush().map_err(failed)?;
    }
    let checksum = ring::digest::digest(&ring::digest::SHA1_FOR_LEGACY_USE_ONLY, &pack);
    pack.extend_from_slice(checksum.as_ref());
    Ok(pack)
}

/// Every tree and blob under `tree` that is not in `known`, added to `out` and to `known`.
fn collect_tree(
    repo: &gix::Repository,
    tree: ObjectId,
    known: &mut HashSet<ObjectId>,
    out: Option<&mut Vec<(Kind, Vec<u8>)>>,
) -> Result<(), PassError> {
    fn walk(
        repo: &gix::Repository,
        tree: ObjectId,
        known: &mut HashSet<ObjectId>,
        out: &mut Option<&mut Vec<(Kind, Vec<u8>)>>,
    ) -> Result<(), PassError> {
        if !known.insert(tree) {
            return Ok(());
        }
        let object = repo.find_tree(tree).map_err(|e| git_error("cannot read a tree", e))?;
        if let Some(out) = out.as_mut() {
            out.push((Kind::Tree, object.data.clone()));
        }
        for entry in object.iter() {
            let entry = entry.map_err(|e| git_error("cannot read a tree", e))?;
            let id = entry.object_id();
            match entry.mode().kind() {
                EntryKind::Tree => walk(repo, id, known, out)?,
                // A submodule's commit lives in another repository.
                EntryKind::Commit => {}
                EntryKind::Blob | EntryKind::BlobExecutable | EntryKind::Link => {
                    if known.insert(id) {
                        if let Some(out) = out.as_mut() {
                            let blob = repo
                                .find_object(id)
                                .map_err(|e| git_error("cannot read a file of the repository", e))?;
                            out.push((Kind::Blob, blob.data.clone()));
                        }
                    }
                }
            }
        }
        Ok(())
    }
    let mut out = out;
    walk(repo, tree, known, &mut out)
}

/// The objects the remote needs to have `tip`, given the commits it has (`have`).
fn objects_to_send(
    repo: &gix::Repository,
    tip: ObjectId,
    have: &[ObjectId],
) -> Result<Vec<(Kind, Vec<u8>)>, PassError> {
    let failed = |e: &dyn std::fmt::Display| git_error("cannot read the history", e);
    let mut theirs = HashSet::new();
    for commit in have {
        theirs.extend(ancestors(repo, *commit)?);
    }
    let mut new_commits: Vec<ObjectId> = ancestors(repo, tip)?.difference(&theirs).copied().collect();
    new_commits.sort();

    // What the remote surely has: the files of the commits the new ones are built on.
    let mut known = HashSet::new();
    let mut commits = Vec::new();
    for id in &new_commits {
        let commit = repo.find_commit(*id).map_err(|e| failed(&e))?;
        for parent in commit.parent_ids().map(gix::Id::detach) {
            if theirs.contains(&parent) {
                let tree = repo
                    .find_commit(parent)
                    .and_then(|parent| parent.tree_id().map(gix::Id::detach))
                    .map_err(|e| failed(&e))?;
                collect_tree(repo, tree, &mut known, None)?;
            }
        }
        let tree = commit.tree_id().map_err(|e| failed(&e))?.detach();
        commits.push((tree, commit.data.clone()));
    }
    let mut objects = Vec::new();
    for (tree, data) in commits {
        objects.push((Kind::Commit, data));
        collect_tree(repo, tree, &mut known, Some(&mut objects))?;
    }
    Ok(objects)
}

/// The update to ask for, or `None` when the remote branch is already at `commit`.
fn plan(
    repo: &gix::Repository,
    advertisement: &Advertisement,
    commit: ObjectId,
    reference: &str,
    force: bool,
) -> Result<Option<Vec<u8>>, PassError> {
    if !advertisement.capabilities.iter().any(|c| c == "report-status") {
        return Err(protocol_error("it does not report the result of a push"));
    }
    let old = advertisement
        .references
        .iter()
        .find(|(name, _)| name == reference)
        .map(|(_, id)| *id);
    if old == Some(commit) {
        return Ok(None);
    }
    if let (Some(old), false) = (old, force) {
        // Only ever forward: what is on the remote must be part of what is sent.
        if !repo.has_object(old) || !ancestors(repo, commit)?.contains(&old) {
            return Err(rejected(
                "the branch on the remote holds commits that are not on this device (not a fast-forward)",
            ));
        }
    }
    let have: Vec<ObjectId> = advertisement
        .references
        .iter()
        .map(|(_, id)| *id)
        .filter(|id| repo.has_object(id) && repo.find_commit(*id).is_ok())
        .collect();
    let objects = objects_to_send(repo, commit, &have)?;

    let old = old.unwrap_or_else(|| ObjectId::null(repo.object_hash()));
    let command = format!("{old} {commit} {reference}\0 report-status agent=pass-core\n");
    let mut request = pkt_line(command.as_bytes());
    request.extend_from_slice(b"0000");
    request.extend_from_slice(&write_pack(&objects)?);
    Ok(Some(request))
}

/// Makes `refs/heads/<branch>` on the remote point to `commit`, sending what it lacks.
/// Fast-forward only, unless `force`. Returns false when the remote already had it.
/// The remote can still refuse (a hook, a branch this device may not write): `PushRejected`.
pub(crate) fn push(
    repo: &gix::Repository,
    remote: &Remote,
    commit: ObjectId,
    branch: &str,
    force: bool,
) -> Result<bool, PassError> {
    let reference = format!("refs/heads/{branch}");
    match remote {
        Remote::Http(http) => {
            let advertisement = read_advertisement(&mut http.receive_pack_advertisement()?.as_slice(), true)?;
            let Some(request) = plan(repo, &advertisement, commit, &reference, force)? else {
                return Ok(false);
            };
            read_report(&mut http.receive_pack(&request)?.as_slice(), &reference)?;
        }
        Remote::Ssh(ssh) => {
            let (mut output, mut input) = ssh.exec("git-receive-pack")?;
            let outcome = output.outcome();
            // When the server's git stops with an error, what it said is the better message.
            let or_said = |e: PassError| outcome.failure().unwrap_or(e);
            let advertisement = read_advertisement(&mut output, false).map_err(or_said)?;
            let Some(request) = plan(repo, &advertisement, commit, &reference, force)? else {
                let _ = input.write_all(b"0000");
                return Ok(false);
            };
            input
                .write_all(&request)
                .map_err(|e| or_said(PassError::Network { message: e.to_string() }))?;
            drop(input);
            read_report(&mut output, &reference).map_err(or_said)?;
        }
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = "1111111111111111111111111111111111111111";
    const B: &str = "2222222222222222222222222222222222222222";

    fn id(hex: &str) -> ObjectId {
        ObjectId::from_hex(hex.as_bytes()).unwrap()
    }

    fn lines(lines: &[&str]) -> Vec<u8> {
        let mut out = Vec::new();
        for line in lines {
            if line.is_empty() {
                out.extend_from_slice(b"0000");
            } else {
                out.extend_from_slice(&pkt_line(line.as_bytes()));
            }
        }
        out
    }

    #[test]
    fn advertisements() {
        let ssh = lines(&[
            &format!("{A} refs/heads/main\0report-status delete-refs ofs-delta\n"),
            &format!("{B} refs/heads/devices/phone\n"),
            "",
        ]);
        let advertisement = read_advertisement(&mut ssh.as_slice(), false).unwrap();
        assert_eq!(
            advertisement.references,
            [
                ("refs/heads/main".to_string(), id(A)),
                ("refs/heads/devices/phone".to_string(), id(B))
            ]
        );
        assert_eq!(
            advertisement.capabilities,
            ["report-status", "delete-refs", "ofs-delta"]
        );

        let mut http = lines(&["# service=git-receive-pack\n", ""]);
        http.extend_from_slice(&ssh);
        assert_eq!(read_advertisement(&mut http.as_slice(), true).unwrap(), advertisement);

        let zero = "0".repeat(40);
        let empty = lines(&[&format!("{zero} capabilities^{{}}\0report-status\n"), ""]);
        let advertisement = read_advertisement(&mut empty.as_slice(), false).unwrap();
        assert!(advertisement.references.is_empty());
        assert_eq!(advertisement.capabilities, ["report-status"]);

        for bad in [
            b"".to_vec(),
            b"<html>not git</html>".to_vec(),
            lines(&["no-id\n", ""]),
            lines(&[&format!("{A} refs/heads/main\n")]), // no flush: cut short
            ssh.clone(),                                 // as http: no service line
        ] {
            let as_http = bad == ssh;
            assert!(read_advertisement(&mut bad.as_slice(), as_http).is_err());
        }
        let error = lines(&["ERR access denied\n", ""]);
        assert!(
            matches!(read_advertisement(&mut error.as_slice(), false), Err(PassError::Network { message }) if message.contains("access denied"))
        );
    }

    #[test]
    fn reports() {
        let reference = "refs/heads/devices/phone";
        let ok = lines(&["unpack ok\n", &format!("ok {reference}\n"), ""]);
        assert!(read_report(&mut ok.as_slice(), reference).is_ok());

        let hook = lines(&["unpack ok\n", "ng refs/heads/main hook declined\n", ""]);
        assert!(
            matches!(read_report(&mut hook.as_slice(), "refs/heads/main"), Err(PassError::PushRejected { message }) if message == "hook declined")
        );
        let unpack = lines(&["unpack index-pack failed\n", ""]);
        assert!(matches!(
            read_report(&mut unpack.as_slice(), reference),
            Err(PassError::PushRejected { .. })
        ));
        // Silence, or the confirmation of another reference, is not a success.
        let other = lines(&["unpack ok\n", "ok refs/heads/main\n", ""]);
        assert!(read_report(&mut other.as_slice(), reference).is_err());
        assert!(read_report(&mut lines(&[""]).as_slice(), reference).is_err());
    }

    #[test]
    fn pack_entry_headers() {
        // From git's pack-format documentation: type in bits 4-6, size continued 7 bits at a time.
        assert_eq!(pack_entry_header(Kind::Commit, 0), [0x10]);
        assert_eq!(pack_entry_header(Kind::Blob, 15), [0x3f]);
        assert_eq!(pack_entry_header(Kind::Blob, 16), [0xb0, 0x01]);
        assert_eq!(pack_entry_header(Kind::Tree, 0x12345), [0xa5, 0xb4, 0x24]);
    }
}
