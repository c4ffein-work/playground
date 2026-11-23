//! git bundles (version 2), as `git bundle create` writes them and `git bundle
//! verify` checks them: a short text header naming references, then a pack.
//!
//! Only the bundles of the sync protocol (`apps/pass/SYNC.md`): one reference, its
//! whole history, no prerequisite. Reading one puts its objects in the repository
//! and nothing else; writing one packs whole objects, each deflated on its own
//! (a store's history is small files, and gpg would not compress them further).

use std::collections::HashSet;
use std::io::{BufRead, Write};
use std::sync::atomic::AtomicBool;

use gix::objs::tree::EntryKind;
use gix::objs::Kind;
use gix::ObjectId;

use crate::git::{ancestors, git_error};
use crate::PassError;

const SIGNATURE: &[u8] = b"# v2 git bundle\n";

fn bad(why: impl Into<String>) -> PassError {
    PassError::BadBundle { message: why.into() }
}

/// The header of a bundle: the one reference it must hold, and where its pack starts.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Header {
    pub(crate) tip: ObjectId,
    pack_offset: usize,
}

/// Reads the header, and refuses anything but one reference named `reference` with no prerequisite.
pub(crate) fn read_header(bundle: &[u8], reference: &str) -> Result<Header, PassError> {
    let rest = bundle
        .strip_prefix(SIGNATURE)
        .ok_or_else(|| bad("it is not a git bundle (version 2)"))?;
    let mut offset = SIGNATURE.len();
    let mut tips = Vec::new();
    for line in rest.split_inclusive(|byte| *byte == b'\n') {
        offset += line.len();
        let Some(line) = line.strip_suffix(b"\n") else {
            return Err(bad("its header is cut short"));
        };
        if line.is_empty() {
            let [(tip, name)] = tips.as_slice() else {
                return Err(bad(format!("it holds {} references, not exactly one", tips.len())));
            };
            if name != reference {
                return Err(bad(format!("its reference is {name}, not {reference}")));
            }
            return Ok(Header {
                tip: *tip,
                pack_offset: offset,
            });
        }
        let line = std::str::from_utf8(line).map_err(|_| bad("its header is not text"))?;
        if line.starts_with('-') {
            return Err(bad("it is not a whole history: it needs commits that are not in it"));
        }
        if line.starts_with('@') {
            return Err(bad("it has capabilities this does not know"));
        }
        let (id, name) = line.split_once(' ').ok_or_else(|| bad("its header is malformed"))?;
        let id = ObjectId::from_hex(id.as_bytes()).map_err(|_| bad("its header is malformed"))?;
        tips.push((id, name.to_string()));
    }
    Err(bad("its header is cut short"))
}

/// Every object `tip` needs is in the repository: its commits, their trees, their files.
fn is_complete(repo: &gix::Repository, tip: ObjectId) -> bool {
    fn tree_is_there(repo: &gix::Repository, tree: ObjectId, seen: &mut HashSet<ObjectId>) -> bool {
        if !seen.insert(tree) {
            return true;
        }
        let Ok(object) = repo.find_tree(tree) else { return false };
        for entry in object.iter() {
            let Ok(entry) = entry else { return false };
            let id = entry.object_id();
            let there = match entry.mode().kind() {
                EntryKind::Tree => tree_is_there(repo, id, seen),
                EntryKind::Commit => true, // a submodule: its commit lives elsewhere
                _ => !seen.insert(id) || repo.has_object(id),
            };
            if !there {
                return false;
            }
        }
        true
    }
    let Ok(commits) = ancestors(repo, tip) else {
        return false;
    };
    let mut seen = HashSet::new();
    commits.into_iter().all(|commit| {
        repo.find_commit(commit)
            .ok()
            .and_then(|commit| commit.tree_id().ok().map(gix::Id::detach))
            .is_some_and(|tree| tree_is_there(repo, tree, &mut seen))
    })
}

/// Puts the objects of a bundle in the repository at `git_dir`, after checking that the
/// pack is sound and that the history of its tip is whole. On any failure nothing stays.
pub(crate) fn unpack(
    git_dir: &std::path::Path,
    bundle: &[u8],
    header: &Header,
    reopen: impl Fn() -> Result<gix::Repository, PassError>,
) -> Result<(), PassError> {
    let pack_dir = git_dir.join("objects").join("pack");
    std::fs::create_dir_all(&pack_dir).map_err(|e| git_error("cannot store the bundle", e))?;
    let mut pack: &[u8] = &bundle[header.pack_offset..];
    let mut reader: &mut dyn BufRead = &mut pack;
    let options = gix_pack::bundle::write::Options {
        // Every object is decompressed and its checksum compared as it is read.
        iteration_mode: gix_pack::data::input::Mode::Verify,
        ..Default::default()
    };
    let written = gix_pack::Bundle::write_to_directory(
        &mut reader,
        Some(&pack_dir),
        &mut gix::progress::Discard,
        &AtomicBool::new(false),
        None::<gix::odb::Handle>,
        gix::hash::Kind::Sha1,
        options,
    )
    .map_err(|_| bad("its pack is damaged or cut short"))?;
    let files = [&written.index_path, &written.data_path, &written.keep_path];
    let remove_files = || {
        for file in files.into_iter().flatten() {
            let _ = std::fs::remove_file(file);
        }
    };
    let complete = reopen().map(|repo| is_complete(&repo, header.tip));
    match complete {
        Ok(true) => {
            // The .keep file tells `git gc` to leave the pack alone until a reference holds it: one will.
            if let Some(keep) = &written.keep_path {
                let _ = std::fs::remove_file(keep);
            }
            Ok(())
        }
        Ok(false) => {
            remove_files();
            Err(bad("it is not complete: objects its history needs are missing"))
        }
        Err(e) => {
            remove_files();
            Err(e)
        }
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

/// A bundle holding `reference` at `tip`, with everything `tip` needs.
pub(crate) fn write(repo: &gix::Repository, tip: ObjectId, reference: &str) -> Result<Vec<u8>, PassError> {
    let failed = |e: &dyn std::fmt::Display| git_error("cannot build the bundle", e);
    let mut objects: Vec<(Kind, Vec<u8>)> = Vec::new();
    let mut seen = HashSet::new();
    fn add_tree(
        repo: &gix::Repository,
        tree: ObjectId,
        seen: &mut HashSet<ObjectId>,
        objects: &mut Vec<(Kind, Vec<u8>)>,
    ) -> Result<(), PassError> {
        if !seen.insert(tree) {
            return Ok(());
        }
        let object = repo.find_tree(tree).map_err(|e| git_error("cannot read a tree", e))?;
        objects.push((Kind::Tree, object.data.clone()));
        for entry in object.iter() {
            let entry = entry.map_err(|e| git_error("cannot read a tree", e))?;
            let id = entry.object_id();
            match entry.mode().kind() {
                EntryKind::Tree => add_tree(repo, id, seen, objects)?,
                EntryKind::Commit => {}
                _ => {
                    if seen.insert(id) {
                        let blob = repo
                            .find_object(id)
                            .map_err(|e| git_error("cannot read a file of the repository", e))?;
                        objects.push((Kind::Blob, blob.data.clone()));
                    }
                }
            }
        }
        Ok(())
    }
    let mut commits: Vec<ObjectId> = ancestors(repo, tip)?.into_iter().collect();
    commits.sort();
    for id in commits {
        let commit = repo.find_commit(id).map_err(|e| failed(&e))?;
        let tree = commit.tree_id().map_err(|e| failed(&e))?.detach();
        objects.push((Kind::Commit, commit.data.clone()));
        add_tree(repo, tree, &mut seen, &mut objects)?;
    }

    let mut pack = Vec::new();
    pack.extend_from_slice(b"PACK");
    pack.extend_from_slice(&2u32.to_be_bytes());
    pack.extend_from_slice(&(objects.len() as u32).to_be_bytes());
    for (kind, data) in &objects {
        pack.extend_from_slice(&pack_entry_header(*kind, data.len()));
        let mut deflate = gix::zlib::stream::deflate::Write::new(&mut pack, gix::zlib::Compression::DEFAULT);
        deflate.write_all(data).map_err(|e| failed(&e))?;
        deflate.flush().map_err(|e| failed(&e))?;
    }
    let checksum = ring::digest::digest(&ring::digest::SHA1_FOR_LEGACY_USE_ONLY, &pack);
    pack.extend_from_slice(checksum.as_ref());

    let mut bundle = SIGNATURE.to_vec();
    bundle.extend_from_slice(format!("{tip} {reference}\n\n").as_bytes());
    bundle.extend_from_slice(&pack);
    Ok(bundle)
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = "1111111111111111111111111111111111111111";
    const B: &str = "2222222222222222222222222222222222222222";

    #[test]
    fn headers() {
        let good = format!("# v2 git bundle\n{A} refs/pass-sync/pc\n\nPACK...");
        let header = read_header(good.as_bytes(), "refs/pass-sync/pc").unwrap();
        assert_eq!(header.tip, ObjectId::from_hex(A.as_bytes()).unwrap());
        assert_eq!(&good.as_bytes()[header.pack_offset..], b"PACK...");

        for (bundle, why) in [
            (
                good.clone(),
                "its reference is refs/pass-sync/pc, not refs/pass-sync/phone",
            ),
            (
                format!("# v2 git bundle\n{A} refs/pass-sync/phone\n{B} refs/heads/main\n\nPACK"),
                "it holds 2 references, not exactly one",
            ),
            (
                "# v2 git bundle\n\nPACK".to_string(),
                "it holds 0 references, not exactly one",
            ),
            (
                format!("# v2 git bundle\n-{B} an older commit\n{A} refs/pass-sync/phone\n\nPACK"),
                "it is not a whole history: it needs commits that are not in it",
            ),
            (
                format!("# v3 git bundle\n@object-format=sha256\n{A} refs/pass-sync/phone\n\nPACK"),
                "it is not a git bundle (version 2)",
            ),
            (
                format!("# v2 git bundle\n{A} refs/pass-sync/phone\n"),
                "its header is cut short",
            ),
            (format!("# v2 git bundle\n{A} refs/pass-sy"), "its header is cut short"),
            (
                "# v2 git bundle\nnot-an-id refs/pass-sync/phone\n\n".to_string(),
                "its header is malformed",
            ),
            ("PACK".to_string(), "it is not a git bundle (version 2)"),
            (String::new(), "it is not a git bundle (version 2)"),
        ] {
            assert_eq!(
                read_header(bundle.as_bytes(), "refs/pass-sync/phone"),
                Err(PassError::BadBundle { message: why.into() }),
                "{bundle:?}"
            );
        }
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
