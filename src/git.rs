use std::cell::RefCell;
use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::collections::HashMap;
use std::collections::HashSet;
use std::collections::VecDeque;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::{SystemTime, UNIX_EPOCH};

use gix::revision::walk::Sorting;

use gix::bstr::{BStr, BString, ByteSlice};
use gix::objs::Tree;
use gix::objs::{CommitRef, TreeRef};
use gix::progress;
use gix::protocol::handshake::Ref;
use gix::protocol::transport::client::{self, Transport};
use gix::protocol::transport::packetline::read::ProgressAction;
use gix::protocol::{Command, fetch, ls_refs};
use gix_pack::cache;
use gix_pack::data::decode::entry::ResolvedBase;

use chrono::{DateTime, FixedOffset};
use gix::remote::Direction;
use gix::status::{Item as StatusItem, Submodule, UntrackedFiles, tree_index::TrackRenames};
use gix::url::Url;
use tempfile::NamedTempFile;

use crate::blog_post::{self, BlogPost, BlogPostUpdate};

/// Whether and how a file changed relative to the base state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum State {
    Created,
    Deleted,
    Modified,
}

/// A remote git repository identified by a URL and a branch name.
pub struct GitRemote {
    pub url: Url,
    pub branch: String,
    /// Folder of the repository holding the blog; `None` for the repository root.
    pub subdir: Option<PathBuf>,
}

/// A local git repository on disk.
pub struct GitLocal {
    pub repo_root: PathBuf,
    /// Branch to read; `None` for whatever `HEAD` designates.
    pub branch: Option<String>,
    /// Folder of the repository holding the blog; `None` for the repository root.
    pub subdir: Option<PathBuf>,
}

pub struct CommitEnds {
    pub tail_commit: gix::objs::Commit,
    pub head_commit: gix::objs::Commit,
}

pub struct TreeEnds {
    pub up_to_tree: gix::objs::Tree,
    pub head_tree: gix::objs::Tree,
    /// Author name of the branch tip.
    pub head_author: Option<String>,
}

pub struct FileBlob {
    pub file_path: PathBuf,
    pub oid: gix::ObjectId,
}

/// Files of the working tree that differ from the last commit and belong on the site.
#[derive(Default)]
pub struct UncommittedChanges {
    /// Folder of the working tree holding the blog.
    blog_root: PathBuf,
    /// Changed files, relative to the blog folder.
    pub files: BTreeMap<PathBuf, State>,
}

impl UncommittedChanges {
    /// Whether one of the files affects every generated page.
    #[must_use]
    pub fn affect_every_page(&self) -> bool {
        self.files.keys().any(|path| changes_every_page(path))
    }

    /// Replays the changes onto the sources in `dest`, recording `now` as the update date of the
    /// articles among them. Call once the git history has been applied to the blog post store.
    pub fn apply(
        &self,
        dest: &Path,
        now: DateTime<FixedOffset>,
        frontmatter_delimiter: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        // Deletions go first because a file may have been replaced by a folder of the same name.
        for (path, _) in self.files.iter().filter(|(_, s)| **s == State::Deleted) {
            match fs::remove_file(dest.join(path)) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e.into()),
                _ => {}
            }
        }
        for (path, _) in self.files.iter().filter(|(_, s)| **s != State::Deleted) {
            let source = self.blog_root.join(path);
            let target = dest.join(path);
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::copy(&source, &target)?;
            if is_post_source(path) {
                let markdown = fs::read_to_string(&source)?;
                update_blog_post_from_markdown_path(
                    path.clone(),
                    &markdown,
                    now,
                    frontmatter_delimiter,
                );
            }
        }
        Ok(())
    }
}

fn is_post_source(path: &Path) -> bool {
    path.extension().is_some_and(|e| e == "md") && !crate::path_is_not_a_post(path)
}

/// Whether an uncommitted change belongs on the site. Tracked files are published anyway, but a new
/// file only counts in a place the build reads from: elsewhere it could be anything.
fn uncommitted_change_is_relevant(path: &Path, state: &State) -> bool {
    if crate::path_is_ignored(path, false) {
        return false;
    }
    *state != State::Created
        || changes_every_page(path)
        || is_post_source(path)
        || path.starts_with("media")
}

/// Repository-relative paths that differ between `HEAD`, the index and the working tree, leaving
/// out ignored files.
fn uncommitted_paths(
    repo: &gix::Repository,
) -> Result<BTreeSet<PathBuf>, Box<dyn std::error::Error>> {
    let status = repo
        .status(progress::Discard)?
        .index_worktree_submodules(None::<Submodule>)
        .tree_index_track_renames(TrackRenames::Disabled)
        .untracked_files(UntrackedFiles::Files)
        .into_iter(None)?;
    let mut paths = BTreeSet::new();
    for item in status {
        let item = item?;
        // Items without a summary only report index bookkeeping, not a change.
        if matches!(&item, StatusItem::IndexWorktree(change) if change.summary().is_none()) {
            continue;
        }
        paths.insert(PathBuf::from(item.location().to_str_lossy().as_ref()));
    }
    Ok(paths)
}

thread_local! {
    static TREE_CACHE: RefCell<HashMap<gix::ObjectId, Tree>> = RefCell::new(HashMap::new());
}

fn cached_tree(oid: &gix::ObjectId) -> Option<Tree> {
    TREE_CACHE.with(|cache| cache.borrow().get(oid).cloned())
}

pub fn blob_blog_post(oid: &gix::ObjectId) -> Option<BlogPost> {
    blog_post::get_by_object_id(oid)
}

pub fn blob_blog_post_by_path(path: &Path) -> Option<BlogPost> {
    blog_post::get_by_path(path)
}

pub fn update_blog_post_from_atom(path: PathBuf, post: BlogPost) {
    let mut post = post;
    post.path = path;
    blog_post::upsert(post);
}

pub fn update_blog_post_from_markdown(
    oid: &gix::ObjectId,
    markdown: &str,
    frontmatter_delimiter: &str,
) {
    if let Some(mut post) = blog_post::get_by_object_id(oid) {
        post.update_from_source_content(markdown, frontmatter_delimiter);
        blog_post::upsert(post);
    }
}

pub fn update_blog_post_from_markdown_path(
    path: PathBuf,
    markdown: &str,
    last_updated: DateTime<FixedOffset>,
    frontmatter_delimiter: &str,
) {
    if blog_post::get_by_path(&path).is_none() {
        blog_post::upsert_with_defaults(path.clone(), None, last_updated);
    }
    let _ = blog_post::update_by_path(
        &path,
        BlogPostUpdate {
            last_updated: Some(last_updated),
            ..BlogPostUpdate::default()
        },
    );
    if let Some(mut post) = blog_post::get_by_path(&path) {
        post.update_from_source_content(markdown, frontmatter_delimiter);
        blog_post::upsert(post);
    }
}

pub fn all_blog_posts() -> Vec<BlogPost> {
    blog_post::all()
}

/// Apply one commit's tree diff to the blog post store. Commits must be applied oldest first.
pub fn apply_tree_diff_to_blog_posts(
    diff: &HashMap<PathBuf, (State, Option<gix::ObjectId>)>,
    commit_dt: DateTime<FixedOffset>,
) {
    let mut deleted = Vec::new();
    let mut created = Vec::new();
    let mut modified = Vec::new();

    for (path, (state, maybe_oid)) in diff {
        match (state, maybe_oid) {
            (State::Deleted, _) => deleted.push(path.clone()),
            (State::Created, Some(oid)) => created.push((path.clone(), *oid)),
            (State::Modified, Some(oid)) => modified.push((path.clone(), *oid)),
            _ => {}
        }
    }

    let mut handled_deleted: HashSet<PathBuf> = HashSet::new();
    let mut handled_created: HashSet<PathBuf> = HashSet::new();

    for draft_path in &deleted {
        if !blog_post::is_draft_md(draft_path) {
            continue;
        }
        let Some(published) = blog_post::published_path_from_draft(draft_path) else {
            continue;
        };
        let Some((created_path, oid)) = created
            .iter()
            .find(|(p, _)| p == &published)
            .map(|(p, o)| (p.clone(), *o))
        else {
            continue;
        };
        blog_post::transfer_post_to_path(draft_path, created_path.clone(), oid, commit_dt);
        blog_post::set_publication_date(&created_path, commit_dt);
        handled_deleted.insert(draft_path.clone());
        handled_created.insert(created_path);
    }

    for del_path in &deleted {
        if handled_deleted.contains(del_path) {
            continue;
        }
        if blog_post::get_by_path(del_path).is_none() {
            continue;
        }
        if let Some((created_path, oid)) = created.iter().find_map(|(path, oid)| {
            if handled_created.contains(path) {
                return None;
            }
            let post = blog_post::get_by_path(del_path)?;
            (post.object_id == Some(*oid)).then(|| (path.clone(), *oid))
        }) {
            blog_post::transfer_post_to_path(del_path, created_path.clone(), oid, commit_dt);
            handled_deleted.insert(del_path.clone());
            handled_created.insert(created_path);
        }
    }

    let remaining_deleted: Vec<_> = deleted
        .iter()
        .filter(|p| !handled_deleted.contains(*p) && blog_post::get_by_path(p).is_some())
        .collect();
    let remaining_created: Vec<_> = created
        .iter()
        .filter(|(p, _)| !handled_created.contains(p))
        .collect();
    if remaining_deleted.len() == 1 && remaining_created.len() == 1 {
        let del_path = remaining_deleted[0];
        let (created_path, oid) = remaining_created[0];
        blog_post::transfer_post_to_path(del_path, created_path.clone(), *oid, commit_dt);
        handled_deleted.insert(del_path.clone());
        handled_created.insert(created_path.clone());
    }

    for del_path in &deleted {
        if handled_deleted.contains(del_path) {
            continue;
        }
        if crate::path_is_not_a_post(del_path) {
            continue;
        }
        if !del_path.extension().is_some_and(|e| e == "md") {
            continue;
        }
        if blog_post::get_by_path(del_path).is_some() {
            blog_post::remove_by_path(del_path);
        }
    }

    for (path, oid) in &created {
        if handled_created.contains(path) || crate::path_is_not_a_post(path) {
            continue;
        }
        if !path.extension().is_some_and(|e| e == "md") {
            continue;
        }
        if let Some(existing) = blog_post::get_by_object_id(oid) {
            if existing.path != *path {
                blog_post::try_set_publication_date(&existing.path, commit_dt);
                if commit_dt > existing.last_updated {
                    let _ = blog_post::update_by_object_id(
                        oid,
                        BlogPostUpdate {
                            last_updated: Some(commit_dt),
                            ..BlogPostUpdate::default()
                        },
                    );
                }
                continue;
            }
        }
        blog_post::register_object_path(*oid, path.clone(), commit_dt);
        blog_post::try_set_publication_date(path, commit_dt);
        let _ = blog_post::update_by_object_id(
            oid,
            BlogPostUpdate {
                path: Some(path.clone()),
                last_updated: Some(commit_dt),
                ..BlogPostUpdate::default()
            },
        );
    }

    for (path, oid) in &modified {
        if crate::path_is_not_a_post(path) {
            continue;
        }
        if !path.extension().is_some_and(|e| e == "md") {
            continue;
        }
        blog_post::register_object_path(*oid, path.clone(), commit_dt);
        let _ = blog_post::update_by_object_id(
            oid,
            BlogPostUpdate {
                path: Some(path.clone()),
                last_updated: Some(commit_dt),
                ..BlogPostUpdate::default()
            },
        );
    }
}

fn walk_commits_newest_first(
    repo: &gix::Repository,
    head_id: gix::ObjectId,
    up_to: Option<&DateTime<FixedOffset>>,
) -> Vec<(gix::ObjectId, DateTime<FixedOffset>)> {
    let walk = repo
        .rev_walk([head_id])
        .first_parent_only()
        .sorting(Sorting::ByCommitTime(Default::default()))
        .all()
        .expect("rev_walk iterator");

    let mut commit_infos = Vec::new();
    for info in walk {
        let info = info.expect("rev walk item");
        let commit = info.object().expect("load commit");
        let dt = commit_time_from_commit(&commit);
        let tree_id = commit.tree_id().expect("commit tree").detach();
        commit_infos.push((tree_id, dt));
        if up_to.is_some_and(|cutoff| dt <= *cutoff) {
            break;
        }
    }
    commit_infos
}

fn apply_commit_history_to_blog_posts(
    repo: &gix::Repository,
    commit_infos: &[(gix::ObjectId, DateTime<FixedOffset>)],
    up_to: Option<&DateTime<FixedOffset>>,
    subdir: Option<&Path>,
) {
    let mut visited = HashSet::new();
    for (tree_oid, _) in commit_infos {
        cache_tree_recursively(repo, *tree_oid, &mut visited);
    }
    replay_commit_history(commit_infos, up_to, subdir);
}

/// Replays `commit_infos` (newest first, as `(root tree, commit time)`) onto the blog post store,
/// oldest commit first, so later commits win. The trees must already be in [`TREE_CACHE`].
///
/// When the oldest commit is at or before `up_to` it only serves as the base of the next diff.
fn replay_commit_history(
    commit_infos: &[(gix::ObjectId, DateTime<FixedOffset>)],
    up_to: Option<&DateTime<FixedOffset>>,
    subdir: Option<&Path>,
) {
    for (i, (tree_oid, commit_dt)) in commit_infos.iter().enumerate().rev() {
        let parent = commit_infos.get(i + 1);
        if parent.is_none() && up_to.is_some_and(|cutoff| commit_dt <= cutoff) {
            continue;
        }
        let from = parent.map_or_else(empty_tree, |(oid, _)| scoped_tree(oid, subdir));
        let diff = tree_diff(&from, &scoped_tree(tree_oid, subdir));
        apply_tree_diff_to_blog_posts(&diff, *commit_dt);
    }
}

fn empty_tree() -> Tree {
    Tree { entries: vec![] }
}

/// Follows `subdir` down from the `root` tree; `None` when a component does not exist.
fn resolve_subdir(
    root: gix::ObjectId,
    subdir: Option<&Path>,
    load: &impl Fn(&gix::ObjectId) -> Option<Tree>,
) -> Option<gix::ObjectId> {
    let Some(subdir) = subdir else {
        return Some(root);
    };
    let mut current = root;
    for component in subdir.iter() {
        let name = component.to_string_lossy();
        let tree = load(&current)?;
        current = tree
            .entries
            .iter()
            .find(|e| e.mode.is_tree() && e.filename.as_slice() == name.as_bytes())?
            .oid;
    }
    Some(current)
}

/// The blog's tree within the cached root tree `root`; empty when the folder does not exist there.
fn scoped_tree(root: &gix::ObjectId, subdir: Option<&Path>) -> Tree {
    resolve_subdir(*root, subdir, &cached_tree)
        .and_then(|oid| cached_tree(&oid))
        .unwrap_or_else(empty_tree)
}

fn subdir_not_found(subdir: Option<&Path>) -> String {
    let name = subdir.map(|p| p.display().to_string()).unwrap_or_default();
    format!("folder `{name}` not found in the repository")
}

fn cache_tree_recursively(
    repo: &gix::Repository,
    tree_oid: gix::ObjectId,
    visited: &mut HashSet<gix::ObjectId>,
) {
    if !visited.insert(tree_oid) {
        return;
    }
    let tree = load_tree(repo, tree_oid).expect("load tree");
    for entry in tree.entries.iter().filter(|e| e.mode.is_tree()) {
        cache_tree_recursively(repo, entry.oid, visited);
    }
}

fn author_display_name(name: &BStr) -> Option<String> {
    let name = name.to_str_lossy().trim().to_string();
    (!name.is_empty()).then_some(name)
}

/// Whether a change to the source file at `path` affects every generated page.
#[must_use]
pub fn changes_every_page(path: &Path) -> bool {
    let parts: Vec<_> = path.iter().filter_map(|part| part.to_str()).collect();
    matches!(
        parts.as_slice(),
        ["templates", _] | ["css", "layout.css" | "theme.css"]
    )
}

/// Files of `head_tree` that rendering needs even when they did not change since the last build.
#[must_use]
pub fn blobs_needed_for_rendering(head_tree: &Tree) -> Vec<FileBlob> {
    let mut blobs = Vec::new();
    for dir in head_tree.entries.iter().filter(|e| e.mode.is_tree()) {
        let Some(tree) = cached_tree(&dir.oid) else {
            continue;
        };
        let dir_name = PathBuf::from(dir.filename.to_str_lossy().as_ref());
        for entry in tree.entries.iter().filter(|e| !e.mode.is_tree()) {
            let file_path = dir_name.join(entry.filename.to_str_lossy().as_ref());
            if changes_every_page(&file_path) || file_path == Path::new("media/avatar") {
                blobs.push(FileBlob {
                    file_path,
                    oid: entry.oid,
                });
            }
        }
    }
    blobs
}

/// Creates or refreshes a bare clone of `url` at `clone_dir`, with the remote's branches mirrored
/// under `refs/heads/` so it can be read like any local repository.
pub fn sync_clone(url: &Url, clone_dir: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let interrupt = AtomicBool::new(false);
    if clone_dir.join("HEAD").is_file() {
        let repo = gix::open(clone_dir)?;
        repo.find_remote("origin")?
            .connect(Direction::Fetch)?
            .prepare_fetch(progress::Discard, Default::default())?
            .receive(progress::Discard, &interrupt)?;
    } else {
        fs::create_dir_all(clone_dir)?;
        gix::prepare_clone_bare(url.clone(), clone_dir)?
            .configure_remote(|remote| {
                Ok(remote.with_refspecs(Some("+refs/heads/*:refs/heads/*"), Direction::Fetch)?)
            })
            .fetch_only(progress::Discard, &interrupt)?;
    }
    Ok(())
}

fn connect_to_remote(url: &Url) -> Result<Box<dyn Transport + Send>, client::connect::Error> {
    gix::protocol::transport::connect(
        url.clone(),
        client::connect::Options {
            version: gix::protocol::transport::Protocol::default(),
            trace: true,
            ssh: Default::default(),
        },
    )
}

fn branch_commit_id(
    repo: &gix::Repository,
    branch: &str,
) -> Result<gix::ObjectId, Box<dyn std::error::Error>> {
    let ref_name = format!("refs/heads/{}", branch);
    let mut reference = repo
        .find_reference(&ref_name)
        .map_err(|e| format!("reference {ref_name}: {e}"))?;
    let id = reference
        .peel_to_id_in_place()
        .map_err(|e| format!("peel {ref_name}: {e}"))?;
    Ok(id.detach())
}

fn commit_time_from_commit(commit: &gix::Commit<'_>) -> DateTime<FixedOffset> {
    let sig = commit.committer().expect("committer");
    let time = sig.time;
    let offset =
        FixedOffset::east_opt(time.offset).unwrap_or_else(|| FixedOffset::east_opt(0).unwrap());
    DateTime::from_timestamp(time.seconds, 0)
        .expect("valid commit timestamp")
        .with_timezone(&offset)
}

fn cache_tree_oid(oid: gix::ObjectId, tree: Tree) {
    TREE_CACHE.with(|cache| {
        cache.borrow_mut().insert(oid, tree);
    });
}

fn load_tree(
    repo: &gix::Repository,
    tree_oid: gix::ObjectId,
) -> Result<Tree, Box<dyn std::error::Error>> {
    if let Some(tree) = cached_tree(&tree_oid) {
        return Ok(tree);
    }
    let obj = repo
        .find_object(tree_oid)
        .map_err(|e| format!("find tree {tree_oid}: {e}"))?;
    let tree_ref = TreeRef::from_bytes(obj.data.as_ref())?;
    let tree = Tree::from(tree_ref);
    cache_tree_oid(tree_oid, tree.clone());
    Ok(tree)
}

/// Write every file under `tree_oid` (recursive) into `dest_root` from a local object database.
fn write_tree_from_repo(
    repo: &gix::Repository,
    dest_root: &Path,
    tree_oid: gix::ObjectId,
    relative: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let tree = load_tree(repo, tree_oid)?;
    for entry in &tree.entries {
        let path = relative.join(entry.filename.to_str_lossy().as_ref());
        if crate::path_is_ignored(&path, entry.mode.is_tree()) {
            continue;
        }
        if entry.mode.is_tree() {
            let dir_at = dest_root.join(&path);
            fs::create_dir_all(&dir_at)?;
            write_tree_from_repo(repo, dest_root, entry.oid, &path)?;
        } else if entry.mode.is_blob() || entry.mode.is_executable() || entry.mode.is_link() {
            let blob = repo
                .find_object(entry.oid)
                .map_err(|e| format!("find blob {}: {e}", entry.oid))?;
            let data = blob.data.to_vec();
            let out_path = dest_root.join(&path);
            if let Some(parent) = out_path.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(out_path, &data)?;
        }
    }
    Ok(())
}

fn temp_dest_dir() -> Result<PathBuf, Box<dyn std::error::Error>> {
    Ok(std::env::temp_dir().join(format!(
        "gitblog-files-{}",
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos()
    )))
}

/// Write every file under `tree_oid` (recursive) into `dest_root`, using objects from a decoded pack.
fn write_tree_from_pack_store(
    store: &HashMap<gix::ObjectId, (gix::objs::Kind, Vec<u8>)>,
    dest_root: &Path,
    tree_oid: &gix::ObjectId,
    relative: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let (kind, data) = store
        .get(tree_oid)
        .ok_or_else(|| format!("tree {} not in pack", tree_oid))?;
    if !matches!(kind, gix::objs::Kind::Tree) {
        return Err(format!("expected tree object, got {:?}", kind).into());
    }
    let tree_ref = TreeRef::from_bytes(data.as_slice())?;
    let tree = Tree::from(tree_ref);
    for entry in &tree.entries {
        let path = relative.join(entry.filename.to_str_lossy().as_ref());
        if crate::path_is_ignored(&path, entry.mode.is_tree()) {
            continue;
        }
        if entry.mode.is_tree() {
            let dir_at = dest_root.join(&path);
            fs::create_dir_all(&dir_at)?;
            write_tree_from_pack_store(store, dest_root, &entry.oid, &path)?;
        } else if entry.mode.is_blob() || entry.mode.is_executable() || entry.mode.is_link() {
            let (bk, blob) = store
                .get(&entry.oid)
                .ok_or_else(|| format!("blob {} not in pack", entry.oid))?;
            if !matches!(bk, gix::objs::Kind::Blob) {
                return Err(format!("expected blob object, got {:?}", bk).into());
            }
            let out_path = dest_root.join(&path);
            if let Some(parent) = out_path.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(out_path, blob.as_slice())?;
        }
    }
    Ok(())
}

fn checkout_commit_into_dest(
    store: &HashMap<gix::ObjectId, (gix::objs::Kind, Vec<u8>)>,
    dest: &Path,
    commit_oid: gix::ObjectId,
    subdir: Option<&Path>,
) -> Result<(), Box<dyn std::error::Error>> {
    let (kind, data) = store
        .get(&commit_oid)
        .ok_or_else(|| format!("commit {} not in pack", commit_oid))?;
    if !matches!(kind, gix::objs::Kind::Commit) {
        return Err(format!("expected commit object, got {:?}", kind).into());
    }
    let commit = CommitRef::from_bytes(data.as_slice())?;
    let load = |oid: &gix::ObjectId| {
        let (_, data) = store.get(oid)?;
        Some(Tree::from(TreeRef::from_bytes(data.as_slice()).ok()?))
    };
    let tree_oid =
        resolve_subdir(commit.tree(), subdir, &load).ok_or_else(|| subdir_not_found(subdir))?;
    write_tree_from_pack_store(store, dest, &tree_oid, Path::new(""))
}

struct FetchedCommit {
    tree: gix::ObjectId,
    time: DateTime<FixedOffset>,
    first_parent: Option<gix::ObjectId>,
    author: Option<String>,
}

impl GitRemote {
    /// Fetch the files from the remote repository and write them to the destination directory.
    /// If the destination is not provided, a temporary directory will be created.
    /// Return the destination directory.
    pub fn pull_files(
        &self,
        blobs: &[FileBlob],
        destination: Option<PathBuf>,
    ) -> Result<PathBuf, Box<dyn std::error::Error>> {
        let dest = match destination {
            Some(path) => path,
            None => std::env::temp_dir().join(format!(
                "gitblog-files-{}",
                SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos()
            )),
        };
        fs::create_dir_all(&dest)?;

        let mut tip_commit_oid: Option<gix::ObjectId> = None;

        let mut transport = connect_to_remote(&self.url)?;

        let outcome = gix::protocol::handshake(
            &mut transport,
            gix::protocol::transport::Service::UploadPack,
            &mut |_| Ok(None),
            vec![],
            &mut progress::Discard,
        )?;

        let fetch_features =
            Command::Fetch.default_features(outcome.server_protocol_version, &outcome.capabilities);
        let mut args =
            fetch::Arguments::new(outcome.server_protocol_version, fetch_features, false);
        if blobs.len() > 0 {
            for blob in blobs {
                args.want(blob.oid);
            }
        } else {
            let prefix = BString::new(format!("ref-prefix refs/heads/{}", self.branch).into());
            let refs = ls_refs(
                &mut transport,
                &outcome.capabilities,
                |_caps, args, _features| {
                    args.push(prefix);
                    Ok(ls_refs::Action::Continue)
                },
                &mut progress::Discard,
                false,
            )
            .expect("ls_refs command");

            let target_ref = format!("refs/heads/{}", self.branch);
            let head_oid = refs
                .iter()
                .find_map(|r| match r {
                    Ref::Direct {
                        full_ref_name,
                        object,
                        ..
                    } if *full_ref_name == target_ref.as_bytes() => Some(*object),
                    _ => None,
                })
                .expect(&format!("{} not found", target_ref));
            tip_commit_oid = Some(head_oid);
            args.want(head_oid);
            if args.can_use_shallow() {
                args.deepen(1);
            }
        }

        let mut reader = args.send(&mut transport, true)?;
        let response = fetch::Response::from_line_reader(
            outcome.server_protocol_version,
            &mut reader,
            true,
            false,
        )?;
        if !response.has_pack() {
            return Err("expected a packfile in fetch response".into());
        }

        reader.set_progress_handler(Some(Box::new(|_is_err, _text| ProgressAction::Continue)));

        let mut pack_tmp = NamedTempFile::new()?;
        std::io::copy(&mut reader, pack_tmp.as_file_mut())?;
        pack_tmp.flush()?;

        let pack = gix_pack::data::File::at(pack_tmp.path(), gix::hash::Kind::Sha1)?;
        let mut inflate_step = gix::features::zlib::Inflate::default();
        let mut inflate_decode = gix::features::zlib::Inflate::default();
        let mut decode_cache = cache::Never;
        let mut offset: gix_pack::data::Offset = 12;

        let decoded_objects: RefCell<HashMap<gix::ObjectId, (gix::objs::Kind, Vec<u8>)>> =
            RefCell::new(HashMap::new());
        let requested: HashMap<gix::ObjectId, &PathBuf> =
            blobs.iter().map(|b| (b.oid, &b.file_path)).collect();
        let mut written: HashMap<gix::ObjectId, bool> = HashMap::new();

        for _ in 0..pack.num_objects() {
            let entry = pack.entry(offset)?;
            let mut out = Vec::new();
            let outcome = pack.decode_entry(
                entry.clone(),
                &mut out,
                &mut inflate_decode,
                &|base_id, out_buf| {
                    let store = decoded_objects.borrow();
                    let key = gix::ObjectId::from(base_id.to_owned());
                    let Some((kind, data)) = store.get(&key) else {
                        return None;
                    };
                    out_buf.extend_from_slice(data.as_slice());
                    Some(ResolvedBase::OutOfPack {
                        kind: *kind,
                        end: out_buf.len(),
                    })
                },
                &mut decode_cache,
            )?;

            let kind = outcome.kind;
            let oid = gix::objs::compute_hash(gix::hash::Kind::Sha1, kind, out.as_slice());
            decoded_objects
                .borrow_mut()
                .insert(oid, (kind, out.clone()));

            if !requested.is_empty() && matches!(kind, gix::objs::Kind::Blob) {
                if let Some(path) = requested.get(&oid) {
                    let output_path = dest.join(path);
                    if let Some(parent) = output_path.parent() {
                        fs::create_dir_all(parent)?;
                    }
                    fs::write(output_path, out.as_slice())?;
                    written.insert(oid, true);
                }
            }

            let mut entry_payload = vec![0u8; entry.decompressed_size as usize];
            let consumed =
                pack.decompress_entry(&entry, &mut inflate_step, entry_payload.as_mut_slice())?;
            offset = entry.pack_offset() + entry.header_size() as u64 + consumed as u64;
        }

        if blobs.is_empty() {
            let tip = tip_commit_oid.ok_or("branch tip OID missing for full checkout")?;
            let store = decoded_objects.borrow();
            checkout_commit_into_dest(&store, &dest, tip, self.subdir.as_deref())?;
        } else {
            for oid in requested.keys() {
                if !written.contains_key(oid) {
                    return Err(format!("blob {} not found in fetched pack", oid).into());
                }
            }
        }

        Ok(dest)
    }

    /// Fetch the branch's commits and trees (no file contents) since `up_to`, or its whole history
    /// when `up_to` is `None`, and replay them onto the blog post store.
    /// Return the Tree objects corresponding to the up_to and head commits.
    pub fn fetch_since(&self, up_to: Option<&DateTime<FixedOffset>>) -> TreeEnds {
        let mut transport = connect_to_remote(&self.url).expect("connect to remote");

        // Capture handshake outcome: we need server_protocol_version and capabilities
        // to pass real server features to fetch::Arguments::new later.
        let outcome = gix::protocol::handshake(
            &mut transport,
            gix::protocol::transport::Service::UploadPack,
            &mut |_| Ok(None),
            vec![],
            &mut progress::Discard,
        )
        .expect("initial handshake");

        // ls_refs filtered to refs/heads/main via a ref-prefix argument.
        let prefix = BString::new(format!("ref-prefix refs/heads/{}", self.branch).into());
        let refs = ls_refs(
            &mut transport,
            &outcome.capabilities,
            |_caps, args, _features| {
                args.push(prefix);
                Ok(ls_refs::Action::Continue)
            },
            &mut progress::Discard,
            false,
        )
        .expect("ls_refs command");

        let target_ref = format!("refs/heads/{}", self.branch);
        let head_oid = refs
            .iter()
            .find_map(|r| match r {
                Ref::Direct {
                    full_ref_name,
                    object,
                    ..
                } if *full_ref_name == target_ref.as_bytes() => Some(*object),
                _ => None,
            })
            .expect(&format!("{} not found", target_ref));

        // Command::Fetch.default_features() reads the server capabilities to build
        // the feature list that Arguments::new needs to gate can_use_shallow() etc.
        let fetch_features =
            Command::Fetch.default_features(outcome.server_protocol_version, &outcome.capabilities);

        let mut args =
            fetch::Arguments::new(outcome.server_protocol_version, fetch_features, false);

        args.want(head_oid);
        // Only commits newer than the last known update are needed for incremental builds.
        if let Some(cutoff) = up_to {
            if args.can_use_deepen_since() {
                args.deepen_since(cutoff.timestamp());
            } else if args.can_use_shallow() {
                args.deepen(1);
            }
        }
        // Dates only need commits and trees: skip all blob objects to reduce pack size.
        if args.can_use_filter() {
            args.filter("blob:none");
        }

        // Set add_done_argument so no negotiation rounds needed.
        let mut reader = args.send(&mut transport, true).expect("fetch send");

        // from_line_reader consumes acknowledgements/shallow-info sections and
        // leaves `reader` positioned at the start of the raw pack stream.
        let response = fetch::Response::from_line_reader(
            outcome.server_protocol_version,
            &mut reader,
            true,  // client_expects_pack
            false, // wants_to_negotiate
        )
        .expect("fetch response");

        assert!(response.has_pack(), "expected a packfile in fetch response");

        // The reader's WithSidebands was constructed without a progress handler,
        // so fill_buf() returns raw packet bytes including the sideband channel
        // byte (0x01). Setting a handler switches it to sideband-decoding mode,
        // stripping the channel byte and forwarding progress/error messages.
        reader.set_progress_handler(Some(Box::new(|_is_err, _text| ProgressAction::Continue)));

        // Persist pack bytes and decode objects via gix_pack::data::File::decode_entry(),
        // which resolves OfsDelta/RefDelta chains and yields restored object bytes.
        let mut pack_tmp = NamedTempFile::new().expect("create temp pack");
        std::io::copy(&mut reader, pack_tmp.as_file_mut()).expect("write fetched pack");
        pack_tmp.flush().expect("flush temp pack");

        let pack = gix_pack::data::File::at(pack_tmp.path(), gix::hash::Kind::Sha1)
            .expect("open temp pack for decoding");
        let mut inflate_step = gix::features::zlib::Inflate::default();
        let mut inflate_decode = gix::features::zlib::Inflate::default();
        let mut decode_cache = cache::Never;
        let mut offset: gix_pack::data::Offset = 12; // PACK header size

        // Keep already decoded objects so ref-delta bases can be provided if needed.
        let decoded_objects: RefCell<HashMap<gix::ObjectId, (gix::objs::Kind, Vec<u8>)>> =
            RefCell::new(HashMap::new());

        let mut commits: HashMap<gix::ObjectId, FetchedCommit> = HashMap::new();

        for _ in 0..pack.num_objects() {
            let entry = pack.entry(offset).expect("read pack entry at offset");
            let mut out = Vec::new();
            let outcome = pack
                .decode_entry(
                    entry.clone(),
                    &mut out,
                    &mut inflate_decode,
                    &|base_id, out_buf| {
                        let store = decoded_objects.borrow();
                        let key = gix::ObjectId::from(base_id.to_owned());
                        let Some((kind, data)) = store.get(&key) else {
                            return None;
                        };
                        out_buf.extend_from_slice(data.as_slice());
                        Some(ResolvedBase::OutOfPack {
                            kind: *kind,
                            end: out_buf.len(),
                        })
                    },
                    &mut decode_cache,
                )
                .expect("decode restored object");

            let kind = outcome.kind;
            let oid = gix::objs::compute_hash(gix::hash::Kind::Sha1, kind, out.as_slice());
            decoded_objects
                .borrow_mut()
                .insert(oid, (kind, out.clone()));

            if matches!(kind, gix::objs::Kind::Commit) {
                let commit = CommitRef::from_bytes(out.as_slice()).expect("parse commit");
                let time = commit.committer.time;
                let offset = FixedOffset::east_opt(time.offset)
                    .unwrap_or_else(|| FixedOffset::east_opt(0).unwrap());
                let dt = DateTime::from_timestamp(time.seconds, 0)
                    .expect("valid commit timestamp")
                    .with_timezone(&offset);
                commits.insert(
                    oid,
                    FetchedCommit {
                        tree: commit.tree(),
                        time: dt,
                        first_parent: commit.parents().next(),
                        author: author_display_name(commit.author.name),
                    },
                );
            } else if matches!(kind, gix::objs::Kind::Tree) {
                let tree_ref = TreeRef::from_bytes(out.as_slice()).expect("parse tree");
                let tree = Tree::from(tree_ref);
                cache_tree_oid(oid, tree);
            }

            // Move to the next entry by consuming exactly this entry from the pack.
            let mut entry_payload = vec![0u8; entry.decompressed_size as usize];
            let consumed = pack
                .decompress_entry(&entry, &mut inflate_step, entry_payload.as_mut_slice())
                .expect("decompress single entry payload");
            offset = entry.pack_offset() + entry.header_size() as u64 + consumed as u64;
        }

        // Newest first along the first-parent chain; it stops at the shallow boundary.
        let mut commit_infos: Vec<(gix::ObjectId, DateTime<FixedOffset>)> = Vec::new();
        let mut next = Some(head_oid);
        while let Some(commit) = next.and_then(|oid| commits.get(&oid)) {
            commit_infos.push((commit.tree, commit.time));
            next = commit.first_parent;
        }
        println!("Fetched {} commits", commit_infos.len());

        let subdir = self.subdir.as_deref();
        replay_commit_history(&commit_infos, up_to, subdir);

        let tree_at = |info: Option<&(gix::ObjectId, DateTime<FixedOffset>)>| {
            info.map_or_else(empty_tree, |(oid, _)| scoped_tree(oid, subdir))
        };
        TreeEnds {
            up_to_tree: tree_at(commit_infos.last()),
            head_tree: tree_at(commit_infos.first()),
            head_author: commits.get(&head_oid).and_then(|c| c.author.clone()),
        }
    }

    /// Compute the difference between two trees.
    pub fn tree_diff(
        &self,
        from: &Tree,
        to: &Tree,
    ) -> HashMap<PathBuf, (State, Option<gix::ObjectId>)> {
        tree_diff(from, to)
    }
}

impl GitLocal {
    fn open(&self) -> Result<gix::Repository, Box<dyn std::error::Error>> {
        Ok(gix::discover(&self.repo_root)?)
    }

    /// The commit to build: the tip of the requested branch, else `HEAD`.
    fn tip_commit_id(
        &self,
        repo: &gix::Repository,
    ) -> Result<gix::ObjectId, Box<dyn std::error::Error>> {
        match &self.branch {
            Some(branch) => branch_commit_id(repo, branch),
            None => Ok(repo.head_id().map_err(|e| format!("HEAD: {e}"))?.detach()),
        }
    }

    /// Whether the checked-out files are a working copy of the commit being built.
    fn working_tree_matches_build(&self, repo: &gix::Repository) -> bool {
        self.branch.as_ref().is_none_or(|branch| {
            let expected = format!("refs/heads/{branch}");
            repo.head_name()
                .ok()
                .flatten()
                .is_some_and(|head| head.as_bstr() == expected.as_bytes())
        })
    }

    /// What the working tree adds to the commit being built. Nothing for a bare repository, or
    /// when another branch than the one being built is checked out.
    pub fn uncommitted_changes(&self) -> Result<UncommittedChanges, Box<dyn std::error::Error>> {
        let repo = self.open()?;
        let Some(worktree) = repo.worktree() else {
            return Ok(UncommittedChanges::default());
        };
        if !self.working_tree_matches_build(&repo) {
            println!("Ignoring uncommitted changes: the branch to build is not checked out");
            return Ok(UncommittedChanges::default());
        }
        let root = worktree.base().to_path_buf();
        let head_tree = repo.head_tree().map_err(|e| format!("HEAD tree: {e}"))?;
        let subdir = self.subdir.as_deref().unwrap_or(Path::new(""));

        let mut files = BTreeMap::new();
        for repo_path in uncommitted_paths(&repo)? {
            let Ok(path) = repo_path.strip_prefix(subdir) else {
                continue;
            };
            let on_disk = root.join(&repo_path);
            let state = if on_disk.is_file() {
                if head_tree.lookup_entry_by_path(&repo_path)?.is_some() {
                    State::Modified
                } else {
                    State::Created
                }
            } else if on_disk.symlink_metadata().is_err() {
                State::Deleted
            } else {
                continue;
            };
            if uncommitted_change_is_relevant(path, &state) {
                files.insert(path.to_path_buf(), state);
            }
        }
        Ok(UncommittedChanges {
            blog_root: root.join(subdir),
            files,
        })
    }

    pub fn tree_diff(
        &self,
        from: &Tree,
        to: &Tree,
    ) -> HashMap<PathBuf, (State, Option<gix::ObjectId>)> {
        tree_diff(from, to)
    }

    pub fn pull_files(
        &self,
        blobs: &[FileBlob],
        destination: Option<PathBuf>,
    ) -> Result<PathBuf, Box<dyn std::error::Error>> {
        let repo = self.open()?;
        let dest = match destination {
            Some(path) => path,
            None => temp_dest_dir()?,
        };
        fs::create_dir_all(&dest)?;

        if blobs.is_empty() {
            let head_id = self.tip_commit_id(&repo)?;
            let commit = repo
                .find_object(head_id)
                .map_err(|e| format!("find commit {head_id}: {e}"))?;
            let commit = commit.into_commit();
            let root_id = commit.tree_id()?.detach();
            let load = |oid: &gix::ObjectId| load_tree(&repo, *oid).ok();
            let subdir = self.subdir.as_deref();
            let tree_id =
                resolve_subdir(root_id, subdir, &load).ok_or_else(|| subdir_not_found(subdir))?;
            write_tree_from_repo(&repo, &dest, tree_id, Path::new(""))?;
        } else {
            for blob in blobs {
                let object = repo
                    .find_object(blob.oid)
                    .map_err(|e| format!("find blob {}: {e}", blob.oid))?;
                let out_path = dest.join(&blob.file_path);
                if let Some(parent) = out_path.parent() {
                    fs::create_dir_all(parent)?;
                }
                fs::write(out_path, &*object.data)?;
            }
        }

        Ok(dest)
    }

    /// Walk the full branch history and set post dates from git events.
    pub fn index_publication_dates(&self) {
        let repo = self.open().expect("discover local repo");
        let head_id = self.tip_commit_id(&repo).expect("revision to build");
        let commit_infos = walk_commits_newest_first(&repo, head_id, None);
        println!(
            "Indexed publication dates from {} commits",
            commit_infos.len()
        );
        apply_commit_history_to_blog_posts(&repo, &commit_infos, None, self.subdir.as_deref());
    }

    /// Author name of the commit being built.
    #[must_use]
    pub fn head_author(&self) -> Option<String> {
        let repo = self.open().ok()?;
        let head_id = self.tip_commit_id(&repo).ok()?;
        let commit = repo.find_object(head_id).ok()?.try_into_commit().ok()?;
        let author = commit.author().ok()?;
        author_display_name(author.name)
    }

    pub fn fetch_since(&self, up_to: &DateTime<FixedOffset>) -> TreeEnds {
        let repo = self.open().expect("discover local repo");
        let head_id = self.tip_commit_id(&repo).expect("revision to build");
        let commit_infos = walk_commits_newest_first(&repo, head_id, Some(up_to));

        println!("Fetched {} commits", commit_infos.len());

        let subdir = self.subdir.as_deref();
        apply_commit_history_to_blog_posts(&repo, &commit_infos, Some(up_to), subdir);

        let head_tree = commit_infos
            .first()
            .map_or_else(empty_tree, |(oid, _)| scoped_tree(oid, subdir));

        let up_to_tree = commit_infos
            .last()
            .filter(|(_, dt)| *dt <= *up_to)
            .map_or_else(empty_tree, |(oid, _)| scoped_tree(oid, subdir));

        TreeEnds {
            up_to_tree,
            head_tree,
            head_author: self.head_author(),
        }
    }
}

/// Compute the difference between two trees.
pub fn tree_diff(from: &Tree, to: &Tree) -> HashMap<PathBuf, (State, Option<gix::ObjectId>)> {
    let mut result: HashMap<PathBuf, (State, Option<gix::ObjectId>)> = HashMap::new();
    let mut queue: VecDeque<(PathBuf, Option<Tree>, Option<Tree>)> = VecDeque::new();
    queue.push_back((PathBuf::new(), Some(from.clone()), Some(to.clone())));

    while let Some((base_path, left_opt, right_opt)) = queue.pop_front() {
        match (left_opt, right_opt) {
            (Some(left), Some(right)) => {
                let mut left_entries: HashMap<_, _> = HashMap::new();
                for entry in &left.entries {
                    left_entries.insert(entry.filename.clone(), entry.clone());
                }

                let mut right_entries: HashMap<_, _> = HashMap::new();
                for entry in &right.entries {
                    right_entries.insert(entry.filename.clone(), entry.clone());
                }

                for (name, left_entry) in &left_entries {
                    let file_name = name.to_str_lossy();
                    let mut full_path = base_path.clone();
                    full_path.push(file_name.as_ref());

                    match right_entries.get(name) {
                        None => {
                            if left_entry.mode.is_tree() {
                                queue.push_back((full_path, cached_tree(&left_entry.oid), None));
                            } else {
                                result.insert(full_path, (State::Deleted, None));
                            }
                        }
                        Some(right_entry) => {
                            if left_entry.oid == right_entry.oid {
                                // If tree OIDs match, descendants are identical: skip subtree.
                                continue;
                            }

                            match (left_entry.mode.is_tree(), right_entry.mode.is_tree()) {
                                (true, true) => {
                                    queue.push_back((
                                        full_path.clone(),
                                        cached_tree(&left_entry.oid),
                                        cached_tree(&right_entry.oid),
                                    ));
                                }
                                (false, false) => {
                                    result.insert(
                                        full_path,
                                        (State::Modified, Some(right_entry.oid)),
                                    );
                                }
                                (true, false) => {
                                    queue.push_back((
                                        full_path.clone(),
                                        cached_tree(&left_entry.oid),
                                        None,
                                    ));
                                    result
                                        .insert(full_path, (State::Created, Some(right_entry.oid)));
                                }
                                (false, true) => {
                                    result.insert(full_path.clone(), (State::Deleted, None));
                                    queue.push_back((
                                        full_path,
                                        None,
                                        cached_tree(&right_entry.oid),
                                    ));
                                }
                            }
                        }
                    }
                }

                for (name, right_entry) in &right_entries {
                    if left_entries.contains_key(name) {
                        continue;
                    }

                    let file_name = name.to_str_lossy();
                    let mut full_path = base_path.clone();
                    full_path.push(file_name.as_ref());

                    if right_entry.mode.is_tree() {
                        queue.push_back((full_path, None, cached_tree(&right_entry.oid)));
                    } else {
                        result.insert(full_path, (State::Created, Some(right_entry.oid)));
                    }
                }
            }
            (Some(left), None) => {
                for entry in &left.entries {
                    let mut full_path = base_path.clone();
                    full_path.push(entry.filename.to_str_lossy().as_ref());
                    if entry.mode.is_tree() {
                        queue.push_back((full_path, cached_tree(&entry.oid), None));
                    } else {
                        result.insert(full_path, (State::Deleted, None));
                    }
                }
            }
            (None, Some(right)) => {
                for entry in &right.entries {
                    let mut full_path = base_path.clone();
                    full_path.push(entry.filename.to_str_lossy().as_ref());
                    if entry.mode.is_tree() {
                        queue.push_back((full_path, None, cached_tree(&entry.oid)));
                    } else {
                        result.insert(full_path, (State::Created, Some(entry.oid)));
                    }
                }
            }
            (None, None) => {}
        }
    }

    result
}

#[cfg(test)]
#[path = "git_tests.rs"]
mod tests;
