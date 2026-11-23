//! The git side of a store, with gitoxide and no `git` program.
//!
//! gitoxide gives the object database, references and the index. What a store needs
//! on top is written here, in the small form it needs: committing given files,
//! merging whole files, and moving the work tree from one commit to another
//! (a store is a few hundred small files: no diff machinery).
//!
//! The commit is the truth and the work tree follows it: a change becomes a commit
//! first, and the directory is then moved to that commit.

use std::collections::{BTreeMap, HashSet};
use std::path::Path;

use gix::bstr::{BString, ByteSlice};
use gix::objs::tree::EntryKind;
use gix::refs::transaction::PreviousValue;
use gix::ObjectId;

use crate::crypto::SecretIdentity;
use crate::{layout, PassError};

/// The branch a store made here is on.
const BRANCH: &str = "main";

pub(crate) fn git_error(context: &str, e: impl std::fmt::Display) -> PassError {
    PassError::Git {
        message: format!("{context}: {e}"),
    }
}

/// Who the commits of this device are by.
#[derive(Debug, Clone)]
pub(crate) struct Author {
    pub(crate) name: String,
    pub(crate) email: String,
}

impl Author {
    fn signature(&self) -> gix::actor::Signature {
        let seconds = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        gix::actor::Signature {
            name: self.name.as_str().into(),
            email: self.email.as_str().into(),
            // The phone's time zone is not known here: UTC.
            time: gix::date::Time { seconds, offset: 0 },
        }
    }
}

/// Nothing of the machine's git configuration is read: there is none on a phone,
/// and on a computer (the tests) it must not change what this does.
fn options() -> gix::open::Options {
    gix::open::Options::isolated()
}

fn configure(mut repo: gix::Repository, author: &Author) -> Result<gix::Repository, PassError> {
    let mut config = repo.config_snapshot_mut();
    for (key, value) in [("user.name", &author.name), ("user.email", &author.email)] {
        config
            .set_raw_value(key, value.as_str())
            .map_err(|e| git_error("cannot set the author", e))?;
    }
    drop(config);
    Ok(repo)
}

/// The repository of a store, `None` when there is none (yet), or the store is a plain directory.
pub(crate) fn open(dir: &Path, author: &Author) -> Result<Option<gix::Repository>, PassError> {
    if !dir.join(".git").exists() {
        return Ok(None);
    }
    let repo = gix::open_opts(dir, options()).map_err(|e| git_error("cannot open the repository", e))?;
    configure(repo, author).map(Some)
}

/// The repository of a store, made empty (on the branch `main`, with no commit) when there is none.
pub(crate) fn open_or_init(dir: &Path, author: &Author) -> Result<gix::Repository, PassError> {
    if let Some(repo) = open(dir, author)? {
        return Ok(repo);
    }
    std::fs::create_dir_all(dir).map_err(|e| PassError::Io {
        message: format!("cannot create the store directory: {e}"),
    })?;
    let overrides = [format!("init.defaultBranch={BRANCH}")];
    let repo = gix::ThreadSafeRepository::init_opts(
        dir,
        gix::create::Kind::WithWorktree,
        gix::create::Options::default(),
        options().config_overrides(overrides),
    )
    .map_err(|e| git_error("cannot create the repository", e))?
    .to_thread_local();
    configure(repo, author)
}

// MARK: references

/// The commit `HEAD` is at, `None` in a repository with no commit yet.
pub(crate) fn head_commit(repo: &gix::Repository) -> Result<Option<ObjectId>, PassError> {
    let mut head = repo.head().map_err(|e| git_error("cannot read HEAD", e))?;
    head.try_peel_to_id()
        .map(|id| id.map(gix::Id::detach))
        .map_err(|e| git_error("cannot read HEAD", e))
}

/// The full name of the branch `HEAD` is on: `refs/heads/main`.
fn branch(repo: &gix::Repository) -> Result<String, PassError> {
    let name = repo
        .head_name()
        .map_err(|e| git_error("cannot read HEAD", e))?
        .ok_or_else(|| git_error("cannot change the store", "HEAD is detached: check a branch out"))?;
    Ok(name.as_bstr().to_string())
}

/// What a reference points to, `None` when there is no such reference.
pub(crate) fn reference(repo: &gix::Repository, name: &str) -> Option<ObjectId> {
    let mut reference = repo.try_find_reference(name).ok()??;
    reference.peel_to_id().ok().map(gix::Id::detach)
}

pub(crate) fn set_reference(repo: &gix::Repository, name: &str, commit: ObjectId, why: &str) -> Result<(), PassError> {
    repo.reference(name, commit, PreviousValue::Any, why)
        .map(|_| ())
        .map_err(|e| git_error(&format!("cannot update {name}"), e))
}

fn find_commit(repo: &gix::Repository, id: ObjectId) -> Result<gix::Commit<'_>, PassError> {
    repo.find_commit(id)
        .map_err(|e| git_error(&format!("cannot read the commit {id}"), e))
}

pub(crate) fn tree_of(repo: &gix::Repository, commit: ObjectId) -> Result<ObjectId, PassError> {
    find_commit(repo, commit)?
        .tree_id()
        .map(gix::Id::detach)
        .map_err(|e| git_error(&format!("cannot read the commit {commit}"), e))
}

/// What a commit says of itself.
pub(crate) struct Summary {
    /// The first line of its message.
    pub(crate) subject: String,
    /// `Name <email>`.
    pub(crate) author: String,
    /// Seconds since 1970.
    pub(crate) time: i64,
}

pub(crate) fn summary(repo: &gix::Repository, commit: ObjectId) -> Result<Summary, PassError> {
    let commit = find_commit(repo, commit)?;
    let subject = commit
        .message_raw_sloppy()
        .lines()
        .next()
        .map(|line| line.to_str_lossy().into_owned())
        .unwrap_or_default();
    let author = commit
        .author()
        .map(|author| format!("{} <{}>", author.name, author.email))
        .unwrap_or_default();
    let time = commit.time().map(|time| time.seconds).unwrap_or(0);
    Ok(Summary { subject, author, time })
}

// MARK: history

/// `tip` and everything before it.
pub(crate) fn ancestors(repo: &gix::Repository, tip: ObjectId) -> Result<HashSet<ObjectId>, PassError> {
    let walk = repo
        .rev_walk([tip])
        .all()
        .map_err(|e| git_error("cannot read the history", e))?;
    let mut seen = HashSet::new();
    for info in walk {
        seen.insert(info.map_err(|e| git_error("cannot read the history", e))?.id);
    }
    Ok(seen)
}

/// The commits `tip` has that `base` (nothing, when `None`) has not: every commit before
/// its parents, the newest first where that leaves a choice (as `git log` lists them).
pub(crate) fn commits_since(
    repo: &gix::Repository,
    base: Option<ObjectId>,
    tip: ObjectId,
) -> Result<Vec<ObjectId>, PassError> {
    let known = base.map(|base| ancestors(repo, base)).transpose()?.unwrap_or_default();
    // Each new commit: when it was made, its parents among the new ones, and how many children wait before it.
    let mut new: std::collections::HashMap<ObjectId, (i64, Vec<ObjectId>, usize)> = Default::default();
    for id in ancestors(repo, tip)? {
        if !known.contains(&id) {
            let commit = find_commit(repo, id)?;
            let time = commit.time().map(|time| time.seconds).unwrap_or(0);
            let parents = commit
                .parent_ids()
                .map(gix::Id::detach)
                .filter(|parent| !known.contains(parent))
                .collect();
            new.insert(id, (time, parents, 0));
        }
    }
    let parents: Vec<ObjectId> = new.values().flat_map(|(_, parents, _)| parents.clone()).collect();
    for parent in parents {
        if let Some(entry) = new.get_mut(&parent) {
            entry.2 += 1;
        }
    }
    let mut ordered = Vec::with_capacity(new.len());
    while !new.is_empty() {
        let next = new
            .iter()
            .filter(|(_, (_, _, waiting))| *waiting == 0)
            .max_by_key(|(id, (time, _, _))| (*time, **id))
            .map(|(id, _)| *id)
            .ok_or_else(|| git_error("cannot read the history", "it goes in a circle"))?;
        let (_, parents, _) = new.remove(&next).expect("just found");
        for parent in parents {
            if let Some(entry) = new.get_mut(&parent) {
                entry.2 -= 1;
            }
        }
        ordered.push(next);
    }
    Ok(ordered)
}

/// The commit two histories last had in common, `None` when they share nothing.
pub(crate) fn merge_base(
    repo: &gix::Repository,
    ours: ObjectId,
    theirs: ObjectId,
) -> Result<Option<ObjectId>, PassError> {
    let theirs = ancestors(repo, theirs)?;
    let common: Vec<ObjectId> = ancestors(repo, ours)?.intersection(&theirs).copied().collect();
    // Common ancestors come with all their own ancestors: the last ones are those that are nobody's parent.
    let mut parents = HashSet::new();
    for id in &common {
        parents.extend(find_commit(repo, *id)?.parent_ids().map(gix::Id::detach));
    }
    let mut last = Vec::new();
    for id in common {
        if !parents.contains(&id) {
            last.push((summary(repo, id)?.time, id));
        }
    }
    // Several only after a criss-cross merge, which this protocol does not produce: the newest then.
    Ok(last.into_iter().max().map(|(_, id)| id))
}

// MARK: trees and the work tree

/// A path of a tree can be written to the work tree: nothing in it leaves the
/// directory or lands in `.git`. The tree comes from another device: it is not trusted.
fn is_safe_path(path: &str) -> bool {
    !path.is_empty()
        && !path.contains('\\')
        && !path.chars().any(char::is_control)
        && path.split('/').all(|component| {
            !component.is_empty()
                && component != "."
                && component != ".."
                && !component.trim_end_matches([' ', '.']).eq_ignore_ascii_case(".git")
        })
}

/// Every regular file of a commit: path → blob. Symbolic links and submodules are
/// left out: neither is written to the work tree (a link could point out of it).
pub(crate) fn files(repo: &gix::Repository, commit: ObjectId) -> Result<BTreeMap<String, ObjectId>, PassError> {
    fn walk(
        repo: &gix::Repository,
        tree: ObjectId,
        prefix: &str,
        out: &mut BTreeMap<String, ObjectId>,
    ) -> Result<(), PassError> {
        let tree = repo.find_tree(tree).map_err(|e| git_error("cannot read a tree", e))?;
        for entry in tree.iter() {
            let entry = entry.map_err(|e| git_error("cannot read a tree", e))?;
            let path = format!("{prefix}{}", entry.filename().to_str_lossy());
            match entry.mode().kind() {
                EntryKind::Tree => walk(repo, entry.object_id(), &format!("{path}/"), out)?,
                EntryKind::Blob | EntryKind::BlobExecutable => {
                    out.insert(path, entry.object_id());
                }
                EntryKind::Link | EntryKind::Commit => {}
            }
        }
        Ok(())
    }
    let mut out = BTreeMap::new();
    walk(repo, tree_of(repo, commit)?, "", &mut out)?;
    Ok(out)
}

/// What a path of a tree is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PathKind {
    /// A regular file.
    File,
    /// A symbolic link, or a submodule: nothing a store holds.
    Other,
}

/// Every path of a commit that is not a directory, with what it is.
pub(crate) fn paths(repo: &gix::Repository, commit: ObjectId) -> Result<Vec<(String, PathKind, ObjectId)>, PassError> {
    fn walk(
        repo: &gix::Repository,
        tree: ObjectId,
        prefix: &str,
        out: &mut Vec<(String, PathKind, ObjectId)>,
    ) -> Result<(), PassError> {
        let tree = repo.find_tree(tree).map_err(|e| git_error("cannot read a tree", e))?;
        for entry in tree.iter() {
            let entry = entry.map_err(|e| git_error("cannot read a tree", e))?;
            let path = format!("{prefix}{}", entry.filename().to_str_lossy());
            match entry.mode().kind() {
                EntryKind::Tree => walk(repo, entry.object_id(), &format!("{path}/"), out)?,
                EntryKind::Blob | EntryKind::BlobExecutable => out.push((path, PathKind::File, entry.object_id())),
                EntryKind::Link | EntryKind::Commit => out.push((path, PathKind::Other, entry.object_id())),
            }
        }
        Ok(())
    }
    let mut out = Vec::new();
    walk(repo, tree_of(repo, commit)?, "", &mut out)?;
    Ok(out)
}

/// A tree holding exactly these files, each already in the repository.
pub(crate) fn tree_of_files(repo: &gix::Repository, files: &BTreeMap<String, ObjectId>) -> Result<ObjectId, PassError> {
    let failed = |e: &dyn std::fmt::Display| git_error("cannot build a tree", e);
    let mut editor = repo
        .edit_tree(ObjectId::empty_tree(repo.object_hash()))
        .map_err(|e| failed(&e))?;
    for (path, blob) in files {
        editor
            .upsert(path.as_str(), EntryKind::Blob, *blob)
            .map_err(|e| failed(&e))?;
    }
    Ok(editor.write().map_err(|e| failed(&e))?.detach())
}

/// The files of a commit, or none for no commit.
pub(crate) fn files_at(
    repo: &gix::Repository,
    commit: Option<ObjectId>,
) -> Result<BTreeMap<String, ObjectId>, PassError> {
    Ok(commit
        .map(|commit| files(repo, commit))
        .transpose()?
        .unwrap_or_default())
}

pub(crate) fn blob(repo: &gix::Repository, id: ObjectId) -> Result<Vec<u8>, PassError> {
    repo.find_blob(id)
        .map(|mut blob| blob.take_data())
        .map_err(|e| git_error("cannot read a file of the repository", e))
}

/// A file that differs between two commits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FileChange {
    pub(crate) path: String,
    pub(crate) before: Option<ObjectId>,
    pub(crate) after: Option<ObjectId>,
}

/// What changes from the commit `from` (nothing, when `None`) to the commit `to`, sorted by path.
pub(crate) fn changes(
    repo: &gix::Repository,
    from: Option<ObjectId>,
    to: ObjectId,
) -> Result<Vec<FileChange>, PassError> {
    let before = files_at(repo, from)?;
    let after = files(repo, to)?;
    let mut out = Vec::new();
    for (path, id) in &after {
        if before.get(path) != Some(id) {
            out.push(FileChange {
                path: path.clone(),
                before: before.get(path).copied(),
                after: Some(*id),
            });
        }
    }
    for (path, id) in &before {
        if !after.contains_key(path) {
            out.push(FileChange {
                path: path.clone(),
                before: Some(*id),
                after: None,
            });
        }
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(out)
}

/// Rewrites the index to say the work tree is at `commit`: `git status` on the store is then clean.
fn write_index(repo: &gix::Repository, commit: ObjectId) -> Result<(), PassError> {
    let mut index = repo
        .index_from_tree(&tree_of(repo, commit)?)
        .map_err(|e| git_error("cannot build the index", e))?;
    index
        .write(Default::default(), Default::default())
        .map_err(|e| git_error("cannot write the index", e))
}

/// Moves the branch `HEAD` is on, and the work tree with it, from where it is to `to`.
/// Returns the paths that changed. Local files in the way are overwritten: the work
/// tree holds nothing that is not in a commit.
pub(crate) fn move_to(repo: &gix::Repository, to: ObjectId, why: &str) -> Result<Vec<String>, PassError> {
    let root = repo
        .workdir()
        .ok_or_else(|| git_error("cannot check out", "the repository has no work tree"))?
        .to_path_buf();
    let changed = changes(repo, head_commit(repo)?, to)?;
    if let Some(unsafe_path) = changed.iter().find(|change| !is_safe_path(&change.path)) {
        return Err(git_error(
            "refusing to check out",
            format!("the path {:?} would leave the store", unsafe_path.path),
        ));
    }
    // Everything is read before anything is written: a missing object stops it here.
    let mut contents = Vec::new();
    for change in &changed {
        contents.push(change.after.map(|id| blob(repo, id)).transpose()?);
    }
    for (change, content) in changed.iter().zip(contents) {
        match content {
            Some(content) => layout::write(&root, &change.path, &content)?,
            None => layout::remove(&root, &change.path)?,
        }
    }
    set_reference(repo, &branch(repo)?, to, why)?;
    write_index(repo, to)?;
    Ok(changed.into_iter().map(|change| change.path).collect())
}

// MARK: committing

/// A file to write, or with `None` to remove, by path relative to the store.
pub(crate) type Edit = (String, Option<Vec<u8>>);

/// The tree `base` (the empty tree, when `None`) becomes once `edits` are applied to it.
pub(crate) fn edit_tree(repo: &gix::Repository, base: Option<ObjectId>, edits: &[Edit]) -> Result<ObjectId, PassError> {
    let failed = |e: &dyn std::fmt::Display| git_error("cannot commit", e);
    let base = base.unwrap_or_else(|| ObjectId::empty_tree(repo.object_hash()));
    let mut editor = repo.edit_tree(base).map_err(|e| failed(&e))?;
    for (path, content) in edits {
        match content {
            Some(content) => {
                let blob = repo.write_blob(content).map_err(|e| failed(&e))?;
                editor
                    .upsert(path.as_str(), EntryKind::Blob, blob)
                    .map_err(|e| failed(&e))?;
            }
            None => {
                editor.remove(path.as_str()).map_err(|e| failed(&e))?;
            }
        }
    }
    Ok(editor.write().map_err(|e| failed(&e))?.detach())
}

/// Writes a commit, signed by `signer` as `git commit -S` would (a commit of this
/// device is always signed: the owner takes no other). Moves no reference.
pub(crate) fn write_commit(
    repo: &gix::Repository,
    tree: ObjectId,
    parents: &[ObjectId],
    message: &str,
    author: &Author,
    signer: &SecretIdentity,
) -> Result<ObjectId, PassError> {
    let failed = |e: &dyn std::fmt::Display| git_error("cannot commit", e);
    let signature = author.signature();
    let mut commit = gix::objs::Commit {
        tree,
        parents: parents.iter().copied().collect(),
        author: signature.clone(),
        committer: signature,
        encoding: None,
        message: format!("{message}\n").into(),
        extra_headers: Vec::new(),
    };
    let mut payload = Vec::new();
    gix::objs::WriteTo::write_to(&commit, &mut payload).map_err(|e| failed(&e))?;
    let armored = signer.sign(&payload)?;
    commit
        .extra_headers
        .push((BString::from("gpgsig"), BString::from(armored.trim_end())));
    Ok(repo.write_object(&commit).map_err(|e| failed(&e))?.detach())
}

/// The signature a commit carries, and the bytes it signs.
pub(crate) struct CommitSignature {
    pub(crate) signature: Vec<u8>,
    pub(crate) signed: Vec<u8>,
}

/// `None` for an unsigned commit.
pub(crate) fn signature_of(repo: &gix::Repository, commit: ObjectId) -> Result<Option<CommitSignature>, PassError> {
    let failed = |e: &dyn std::fmt::Display| git_error("cannot read a commit", e);
    let object = repo.find_object(commit).map_err(|e| failed(&e))?;
    let found = gix::objs::CommitRefIter::signature(&object.data, repo.object_hash()).map_err(|e| failed(&e))?;
    Ok(found.map(|(signature, signed)| CommitSignature {
        signature: signature.to_vec(),
        signed: signed.to_bstring().into(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_from_another_device_cannot_leave_the_store() {
        for good in [
            "a.gpg",
            "Email/work.gpg",
            ".gpg-id",
            "work/.gpg-id",
            ".gitattributes",
            "a b/c.gpg",
        ] {
            assert!(is_safe_path(good), "{good:?}");
        }
        for bad in [
            "",
            "/etc/passwd",
            "../x",
            "a/../../x",
            "a//b",
            "./a",
            ".git/config",
            ".GIT/hooks/post-checkout",
            "a/.git/config",
            ".git./config",
            ".git /config",
            "a\\b",
            "a\nb",
        ] {
            assert!(!is_safe_path(bad), "{bad:?}");
        }
    }
}
