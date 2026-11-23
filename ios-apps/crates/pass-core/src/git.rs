//! The git side of a store, with gitoxide and no `git` program.
//!
//! gitoxide gives the object database, references, the index, and fetching.
//! What it does not have is written here, in the small form a password store needs:
//! committing given files, moving the work tree from one commit to another
//! (a store is a few hundred small files: no diff machinery), and pushing (push.rs).
//!
//! The commit is the truth and the work tree follows it: a change is committed
//! first and written to the directory after, so the two cannot disagree for long.

use std::collections::{BTreeMap, HashSet};
use std::path::Path;
use std::sync::atomic::AtomicBool;

use gix::bstr::{BString, ByteSlice};
use gix::objs::tree::EntryKind;
use gix::refs::transaction::PreviousValue;
use gix::remote::Direction;
use gix::ObjectId;
use gix_transport::client::blocking_io::Transport;
use gix_transport::Protocol;

use crate::crypto::SecretIdentity;
use crate::http::{GitHttp, HttpRemote};
use crate::ssh::SshRemote;
use crate::{layout, PassError};

pub(crate) const REMOTE_NAME: &str = "origin";

pub(crate) fn git_error(context: &str, e: impl std::fmt::Display) -> PassError {
    PassError::Git {
        message: format!("{context}: {e}"),
    }
}

/// gitoxide's error with its cause: "Failed to list references on the remote: connection refused".
fn with_cause(e: &gix::Error) -> String {
    let (what, cause) = (e.to_string(), e.probable_cause().to_string());
    if what == cause {
        what
    } else {
        format!("{what}: {cause}")
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

/// The repository of a store, `None` when the store is a plain directory.
pub(crate) fn open(dir: &Path, author: &Author) -> Result<Option<gix::Repository>, PassError> {
    if !dir.join(".git").exists() {
        return Ok(None);
    }
    let repo = gix::open_opts(dir, options()).map_err(|e| git_error("cannot open the repository", e))?;
    configure(repo, author).map(Some)
}

fn init(dir: &Path, author: &Author) -> Result<gix::Repository, PassError> {
    let repo = gix::ThreadSafeRepository::init_opts(
        dir,
        gix::create::Kind::WithWorktree,
        gix::create::Options::default(),
        options(),
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

/// The branch `HEAD` is on: `main`.
pub(crate) fn branch(repo: &gix::Repository) -> Result<String, PassError> {
    let name = repo
        .head_name()
        .map_err(|e| git_error("cannot read HEAD", e))?
        .ok_or_else(|| git_error("cannot sync", "HEAD is detached: check a branch out"))?;
    Ok(name.shorten().to_string())
}

/// The tip of `refs/remotes/origin/<branch>` as of the last fetch or push.
pub(crate) fn remote_tip(repo: &gix::Repository, branch: &str) -> Option<ObjectId> {
    let name = format!("refs/remotes/{REMOTE_NAME}/{branch}");
    let mut reference = repo.try_find_reference(name.as_str()).ok()??;
    reference.peel_to_id().ok().map(gix::Id::detach)
}

fn set_reference(repo: &gix::Repository, name: &str, commit: ObjectId, why: &str) -> Result<(), PassError> {
    repo.reference(name, commit, PreviousValue::Any, why)
        .map(|_| ())
        .map_err(|e| git_error(&format!("cannot update {name}"), e))
}

pub(crate) fn set_remote_tip(repo: &gix::Repository, branch: &str, commit: ObjectId) -> Result<(), PassError> {
    let name = format!("refs/remotes/{REMOTE_NAME}/{branch}");
    set_reference(repo, &name, commit, "update by push")
}

fn find_commit<'r>(repo: &'r gix::Repository, id: ObjectId) -> Result<gix::Commit<'r>, PassError> {
    repo.find_commit(id)
        .map_err(|e| git_error(&format!("cannot read the commit {id}"), e))
}

fn tree_of(repo: &gix::Repository, commit: ObjectId) -> Result<ObjectId, PassError> {
    find_commit(repo, commit)?
        .tree_id()
        .map(gix::Id::detach)
        .map_err(|e| git_error(&format!("cannot read the commit {commit}"), e))
}

/// The first line of a commit's message, and when it was made (seconds since 1970).
pub(crate) fn summary(repo: &gix::Repository, commit: ObjectId) -> Result<(String, i64), PassError> {
    let commit = find_commit(repo, commit)?;
    let subject = commit
        .message_raw_sloppy()
        .lines()
        .next()
        .map(|line| line.to_str_lossy().into_owned())
        .unwrap_or_default();
    let time = commit.time().map(|time| time.seconds).unwrap_or(0);
    Ok((subject, time))
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

/// How many commits `ours` has that `theirs` has not, and the reverse.
pub(crate) fn ahead_behind(
    repo: &gix::Repository,
    ours: Option<ObjectId>,
    theirs: Option<ObjectId>,
) -> Result<(u32, u32), PassError> {
    if ours == theirs {
        return Ok((0, 0));
    }
    let ours = ours.map(|tip| ancestors(repo, tip)).transpose()?.unwrap_or_default();
    let theirs = theirs.map(|tip| ancestors(repo, tip)).transpose()?.unwrap_or_default();
    Ok((
        ours.difference(&theirs).count() as u32,
        theirs.difference(&ours).count() as u32,
    ))
}

// MARK: trees and the work tree

/// A path of a tree can be written to the work tree: nothing in it leaves the
/// directory or lands in `.git`. The tree comes from the remote: it is not trusted.
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

/// What changes from the commit `from` (nothing, when `None`) to the commit `to`.
pub(crate) fn changes(
    repo: &gix::Repository,
    from: Option<ObjectId>,
    to: ObjectId,
) -> Result<Vec<FileChange>, PassError> {
    let before = from.map(|commit| files(repo, commit)).transpose()?.unwrap_or_default();
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
    let name = format!("refs/heads/{}", branch(repo)?);
    set_reference(repo, &name, to, why)?;
    write_index(repo, to)?;
    Ok(changed.into_iter().map(|change| change.path).collect())
}

// MARK: committing

/// A file to write, or with `None` to remove, by path relative to the store.
pub(crate) type Edit = (String, Option<Vec<u8>>);

/// Commits `edits` on top of `HEAD`, as `git add <files> && git commit -m <message>`
/// would, signed by `signer` when it can sign (`git commit -S`). The work tree is
/// not touched. Returns whether the commit is signed.
pub(crate) fn commit(
    repo: &gix::Repository,
    edits: &[Edit],
    message: &str,
    author: &Author,
    signer: Option<&SecretIdentity>,
) -> Result<bool, PassError> {
    let failed = |e: &dyn std::fmt::Display| git_error("cannot commit", e);
    let parent = head_commit(repo)?;
    let base = match parent {
        Some(parent) => tree_of(repo, parent)?,
        None => ObjectId::empty_tree(repo.object_hash()),
    };
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
    let tree = editor.write().map_err(|e| failed(&e))?.detach();

    let signature = author.signature();
    let mut commit = gix::objs::Commit {
        tree,
        parents: parent.into_iter().collect(),
        author: signature.clone(),
        committer: signature,
        encoding: None,
        message: format!("{message}\n").into(),
        extra_headers: Vec::new(),
    };
    let mut signed = false;
    if let Some(signer) = signer {
        let mut payload = Vec::new();
        gix::objs::WriteTo::write_to(&commit, &mut payload).map_err(|e| failed(&e))?;
        if let Some(armored) = signer.sign(&payload)? {
            commit
                .extra_headers
                .push((BString::from("gpgsig"), BString::from(armored.trim_end())));
            signed = true;
        }
    }
    let id = repo.write_object(&commit).map_err(|e| failed(&e))?.detach();
    let name = format!("refs/heads/{}", branch(repo)?);
    set_reference(repo, &name, id, &format!("commit: {message}"))?;
    write_index(repo, id)?;
    Ok(signed)
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

// MARK: the remote

/// A remote, over one of the two transports.
pub(crate) enum Remote {
    Http(std::sync::Arc<HttpRemote>),
    Ssh(SshRemote),
}

/// What a fetch found: the remote's branches are now under `refs/remotes/origin/`.
pub(crate) struct Fetched {
    /// The branch the remote's `HEAD` points to.
    pub(crate) default_branch: Option<String>,
    pub(crate) branches: Vec<String>,
}

fn fetch_with<T: Transport>(repo: &gix::Repository, url: &str, transport: T) -> Result<Fetched, gix::Error> {
    use gix::protocol::handshake::Ref;
    let refspec = format!("+refs/heads/*:refs/remotes/{REMOTE_NAME}/*");
    let remote = repo
        .remote_at(url)?
        .with_fetch_tags(gix::remote::fetch::Tags::None)
        .with_refspecs(Some(refspec.as_str()), Direction::Fetch)?;
    let options = gix::remote::ref_map::Options {
        // Ask for every reference: HEAD tells which branch is the main one.
        prefix_from_spec_as_filter_on_remote: false,
        ..Default::default()
    };
    let outcome = remote
        .to_connection_with_transport(transport)
        // Never the machine's credential helpers or a prompt: what we have is already in the transport.
        .with_credentials(|_| Ok(None))
        // Nor its git configuration: the transport is already set up.
        .with_transport_options(Box::new(()))
        .prepare_fetch(gix::progress::Discard, options)?
        .receive(gix::progress::Discard, &AtomicBool::new(false))?;
    let mut fetched = Fetched {
        default_branch: None,
        branches: Vec::new(),
    };
    for reference in &outcome.ref_map.remote_refs {
        let (name, target) = match reference {
            Ref::Symbolic {
                full_ref_name, target, ..
            }
            | Ref::Unborn { full_ref_name, target } => (full_ref_name, Some(target)),
            Ref::Direct { full_ref_name, .. } | Ref::Peeled { full_ref_name, .. } => (full_ref_name, None),
        };
        if let Some(branch) = name.to_str_lossy().strip_prefix("refs/heads/") {
            fetched.branches.push(branch.to_string());
        }
        if name == "HEAD" {
            fetched.default_branch = target
                .and_then(|target| target.to_str().ok()?.strip_prefix("refs/heads/"))
                .map(str::to_string);
        }
    }
    Ok(fetched)
}

/// Fetches every branch of the remote into `refs/remotes/origin/`. Changes nothing else.
pub(crate) fn fetch(repo: &gix::Repository, remote: &Remote) -> Result<Fetched, PassError> {
    match remote {
        Remote::Http(http) => {
            let url = gix::url::parse(http.url.as_bytes().as_bstr()).map_err(|e| PassError::InvalidConfig {
                message: format!("not a usable repository URL: {e}"),
            })?;
            let transport =
                gix_transport::client::blocking_io::http::connect_http(GitHttp(http.clone()), url, Protocol::V2, false);
            fetch_with(repo, &http.url, transport).map_err(|e| {
                http.take_failure().unwrap_or_else(|| PassError::Network {
                    message: format!("cannot fetch: {}", with_cause(&e)),
                })
            })
        }
        Remote::Ssh(ssh) => {
            let (output, input) = ssh.exec("git-upload-pack")?;
            let ending = output.outcome();
            // Version 1: version 2 is asked for through an environment variable that sshd drops by default.
            let transport = gix_transport::client::git::blocking_io::Connection::new(
                output,
                input,
                Protocol::V1,
                ssh.path(),
                None::<(String, Option<u16>)>,
                gix_transport::client::git::ConnectMode::Process,
                false,
            )
            .custom_url(Some(ssh.url.as_str().into()));
            fetch_with(repo, &ssh.url, transport).map_err(|e| {
                ending.failure().unwrap_or_else(|| PassError::Network {
                    message: format!("cannot fetch: {}", with_cause(&e)),
                })
            })
        }
    }
}

/// Clones into `dir`: fetches everything, and checks out the remote's main branch.
pub(crate) fn clone(dir: &Path, remote: &Remote, author: &Author) -> Result<gix::Repository, PassError> {
    let not_empty = std::fs::read_dir(dir)
        .map(|mut items| items.next().is_some())
        .unwrap_or(false);
    if not_empty {
        return Err(PassError::Io {
            message: "there is already a store on this device: remove it first".into(),
        });
    }
    std::fs::create_dir_all(dir).map_err(|e| PassError::Io {
        message: format!("cannot create the store directory: {e}"),
    })?;
    let result = (|| {
        let repo = init(dir, author)?;
        let fetched = fetch(&repo, remote)?;
        let branch = fetched
            .default_branch
            .filter(|branch| fetched.branches.contains(branch))
            .or_else(|| {
                ["main", "master"]
                    .iter()
                    .find(|name| fetched.branches.iter().any(|branch| branch == *name))
                    .map(|name| name.to_string())
            })
            .ok_or_else(|| {
                git_error(
                    "nothing to clone",
                    "the remote has no main branch yet: push the store to it from the computer first",
                )
            })?;
        let tip =
            remote_tip(&repo, &branch).ok_or_else(|| git_error("cannot clone", "the main branch did not arrive"))?;
        // HEAD on the main branch, which does not exist yet: `move_to` creates it at the tip.
        let head = gix::refs::transaction::RefEdit {
            change: gix::refs::transaction::Change::Update {
                log: Default::default(),
                expected: PreviousValue::Any,
                new: gix::refs::Target::Symbolic(
                    format!("refs/heads/{branch}")
                        .try_into()
                        .map_err(|e| git_error("the remote's main branch has an unusable name", e))?,
                ),
            },
            name: "HEAD".try_into().expect("a valid name"),
            deref: false,
        };
        repo.edit_reference(head).map_err(|e| git_error("cannot set HEAD", e))?;
        move_to(&repo, tip, "clone")?;
        Ok(repo)
    })();
    if result.is_err() {
        // Half a clone is not a store: the next attempt starts from nothing.
        let _ = std::fs::remove_dir_all(dir);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_from_the_remote_cannot_leave_the_store() {
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
