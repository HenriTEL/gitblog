use std::cell::RefCell;
use std::collections::HashMap;
use std::collections::HashSet;
use std::collections::VecDeque;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use gix::revision::walk::Sorting;

use gix::bstr::{BString, ByteSlice};
use gix::objs::Tree;
use gix::objs::{CommitRef, TreeRef};
use gix::progress;
use gix::protocol::handshake::Ref;
use gix::protocol::transport::client::http;
use gix::protocol::transport::packetline::read::ProgressAction;
use gix::protocol::{Command, fetch, ls_refs};
use gix_pack::cache;
use gix_pack::data::decode::entry::ResolvedBase;

use chrono::{DateTime, FixedOffset};
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
}

/// A local git repository on disk.
pub struct GitLocal {
    pub repo_root: PathBuf,
    pub branch: String,
}

pub struct CommitEnds {
    pub tail_commit: gix::objs::Commit,
    pub head_commit: gix::objs::Commit,
}

pub struct TreeEnds {
    pub up_to_tree: gix::objs::Tree,
    pub head_tree: gix::objs::Tree,
}

pub struct FileBlob {
    pub file_path: PathBuf,
    pub oid: gix::ObjectId,
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

/// Apply one commit's tree diff to the blog post store (newest-first walk).
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
        if crate::path_is_ignored(del_path, false) {
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
        if handled_created.contains(path) || crate::path_is_ignored(path, false) {
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
        if crate::path_is_ignored(path, false) {
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
    tree_diff_fn: impl Fn(&Tree, &Tree) -> HashMap<PathBuf, (State, Option<gix::ObjectId>)>,
) {
    for i in 0..commit_infos.len() {
        let (tree_oid, commit_dt) = &commit_infos[i];
        let current_tree = load_tree(repo, *tree_oid).expect("load tree");
        let parent_tree = if i + 1 < commit_infos.len() {
            Some(load_tree(repo, commit_infos[i + 1].0).expect("load parent tree"))
        } else {
            None
        };
        let from = parent_tree.unwrap_or_else(|| Tree { entries: vec![] });
        let diff = tree_diff_fn(&from, &current_tree);
        apply_tree_diff_to_blog_posts(&diff, *commit_dt);
    }
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

/// Copy blog-relevant files from a worktree into `destination` or a fresh temp directory.
pub fn materialize_worktree_copy(
    repo: &gix::Repository,
    destination: Option<PathBuf>,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let dest = match destination {
        Some(path) => path,
        None => temp_dest_dir()?,
    };
    fs::create_dir_all(&dest)?;

    let source_base = repo
        .worktree()
        .map(|wt| wt.base().to_path_buf())
        .unwrap_or_else(|| repo.path().to_path_buf());

    copy_worktree_files(&source_base, &source_base, &dest)?;
    Ok(dest)
}

fn worktree_copy_is_skipped(rel: &Path, is_dir: bool) -> bool {
    if rel
        .components()
        .next()
        .is_some_and(|c| c.as_os_str() == ".git")
    {
        return true;
    }
    crate::path_is_ignored(rel, is_dir)
}

fn copy_worktree_files(
    root: &Path,
    current: &Path,
    dest: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    for entry in fs::read_dir(current)? {
        let entry = entry?;
        let path = entry.path();
        let rel = path.strip_prefix(root)?;
        if worktree_copy_is_skipped(rel, path.is_dir()) {
            continue;
        }
        let out = dest.join(rel);
        if path.is_dir() {
            fs::create_dir_all(&out)?;
            copy_worktree_files(root, &path, dest)?;
        } else {
            if let Some(parent) = out.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::copy(&path, &out)?;
        }
    }
    Ok(())
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
) -> Result<(), Box<dyn std::error::Error>> {
    let (kind, data) = store
        .get(&commit_oid)
        .ok_or_else(|| format!("commit {} not in pack", commit_oid))?;
    if !matches!(kind, gix::objs::Kind::Commit) {
        return Err(format!("expected commit object, got {:?}", kind).into());
    }
    let commit = CommitRef::from_bytes(data.as_slice())?;
    let tree_oid = commit.tree();
    write_tree_from_pack_store(store, dest, &tree_oid, Path::new(""))
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

        let mut transport = http::connect(
            self.url.clone(),
            gix::protocol::transport::Protocol::default(),
            true,
        );

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
            checkout_commit_into_dest(&store, &dest, tip)?;
        } else {
            for oid in requested.keys() {
                if !written.contains_key(oid) {
                    return Err(format!("blob {} not found in fetched pack", oid).into());
                }
            }
        }

        Ok(dest)
    }

    /// Fetch changes since the given date and time.
    /// Return the Tree objects corresponding to the up_to and head commits.
    pub fn fetch_since(&self, up_to: &DateTime<FixedOffset>) -> TreeEnds {
        let mut transport = http::connect(
            self.url.clone(),
            gix::protocol::transport::Protocol::default(),
            true,
        );

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
        // Limit history depth so we don't download the full repository.
        // Use deepen-since when up_to is a real (positive) Unix timestamp so that
        // only commits newer than the last known update are included.
        // Fall back to deepen(1) for the MIN_UTC sentinel (no blog_url given) –
        // its timestamp is ≈ -8.3e12, which GitHub rejects as invalid.
        let ts = up_to.timestamp();
        if args.can_use_deepen_since() {
            args.deepen_since(ts);
            // Skip all blob objects to reduce pack size.
            if args.can_use_filter() {
                args.filter("blob:none");
            }
        } else if args.can_use_shallow() {
            args.deepen(1);
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

        let mut head_tree_id: Option<gix::ObjectId> = None;
        let mut up_to_tree_id: Option<gix::ObjectId> = None;
        let mut head_tree: Option<gix::objs::Tree> = None;
        let mut up_to_tree: Option<gix::objs::Tree> = None;

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

        let mut commits_count: i32 = 0;
        let mut commit_infos: Vec<(gix::ObjectId, DateTime<FixedOffset>)> = Vec::new();

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
                let commit_tree_id = commit.tree();
                let time = commit.committer.time;
                let offset = FixedOffset::east_opt(time.offset)
                    .unwrap_or_else(|| FixedOffset::east_opt(0).unwrap());
                let dt = DateTime::from_timestamp(time.seconds, 0)
                    .expect("valid commit timestamp")
                    .with_timezone(&offset);
                commit_infos.push((commit_tree_id, dt));
                if head_tree_id.is_none() {
                    head_tree_id = Some(commit_tree_id);
                }
                up_to_tree_id = Some(commit_tree_id);
                commits_count += 1;
            } else if matches!(kind, gix::objs::Kind::Tree) {
                let tree_ref = TreeRef::from_bytes(out.as_slice()).expect("parse tree");
                let tree = Tree::from(tree_ref);
                TREE_CACHE.with(|cache| {
                    cache.borrow_mut().insert(oid, tree.clone());
                });
                if Some(oid) == head_tree_id {
                    head_tree = Some(tree.clone());
                }
                if Some(oid) == up_to_tree_id {
                    up_to_tree = Some(tree);
                }
            }

            // Move to the next entry by consuming exactly this entry from the pack.
            let mut entry_payload = vec![0u8; entry.decompressed_size as usize];
            let consumed = pack
                .decompress_entry(&entry, &mut inflate_step, entry_payload.as_mut_slice())
                .expect("decompress single entry payload");
            offset = entry.pack_offset() + entry.header_size() as u64 + consumed as u64;
        }

        println!("Fetched {} commits", commits_count);

        for i in 0..commit_infos.len() {
            let (tree_oid, commit_dt) = &commit_infos[i];
            let Some(current_tree) = cached_tree(tree_oid) else {
                continue;
            };
            let parent_tree = if i + 1 < commit_infos.len() {
                cached_tree(&commit_infos[i + 1].0)
            } else {
                None
            };
            let from = parent_tree.unwrap_or_else(|| Tree { entries: vec![] });
            let diff = self.tree_diff(&from, &current_tree);
            apply_tree_diff_to_blog_posts(&diff, *commit_dt);
        }

        TreeEnds {
            up_to_tree: up_to_tree.expect("up_to tree object was not found in pack"),
            head_tree: head_tree.expect("head tree object was not found in pack"),
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
            let head_id = branch_commit_id(&repo, &self.branch)?;
            let commit = repo
                .find_object(head_id)
                .map_err(|e| format!("find commit {head_id}: {e}"))?;
            let commit = commit.into_commit();
            let tree_id = commit.tree_id()?.detach();
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

    /// Walk the full branch history and set publication dates from git events.
    pub fn index_publication_dates(&self) {
        let repo = self.open().expect("discover local repo");
        let head_id = branch_commit_id(&repo, &self.branch).expect("branch ref");
        let commit_infos = walk_commits_newest_first(&repo, head_id, None);
        println!(
            "Indexed publication dates from {} commits",
            commit_infos.len()
        );
        apply_commit_history_to_blog_posts(&repo, &commit_infos, tree_diff);
    }

    pub fn fetch_since(&self, up_to: &DateTime<FixedOffset>) -> TreeEnds {
        let repo = self.open().expect("discover local repo");
        let head_id = branch_commit_id(&repo, &self.branch).expect("branch ref");
        let commit_infos = walk_commits_newest_first(&repo, head_id, Some(up_to));

        println!("Fetched {} commits", commit_infos.len());

        apply_commit_history_to_blog_posts(&repo, &commit_infos, tree_diff);

        let head_tree = commit_infos
            .first()
            .map(|(oid, _)| load_tree(&repo, *oid).expect("head tree"))
            .unwrap_or_else(|| Tree { entries: vec![] });

        let up_to_tree = commit_infos
            .last()
            .and_then(|(oid, dt)| {
                if *dt <= *up_to {
                    load_tree(&repo, *oid).ok()
                } else {
                    None
                }
            })
            .unwrap_or_else(|| Tree { entries: vec![] });

        TreeEnds {
            up_to_tree,
            head_tree,
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
mod tests {
    use std::collections::HashMap;
    use std::path::{Path, PathBuf};
    use std::process::Command;

    use chrono::{TimeZone, Utc};
    use gix::objs::Kind;

    use super::{State, apply_tree_diff_to_blog_posts, worktree_copy_is_skipped};
    use crate::blog_post;

    #[test]
    fn worktree_copy_skips_dot_git() {
        assert!(worktree_copy_is_skipped(Path::new(".git"), true));
        assert!(worktree_copy_is_skipped(Path::new(".git/config"), false));
        assert!(!worktree_copy_is_skipped(Path::new("posts/post.md"), false));
    }

    #[test]
    fn apply_diff_sets_publication_on_first_create() {
        let path = PathBuf::from("posts/gitblog-test-create.md");
        let content = b"# Hello\n";
        let oid = gix::objs::compute_hash(gix::hash::Kind::Sha1, Kind::Blob, content);
        let commit_dt = Utc
            .with_ymd_and_hms(2024, 6, 1, 10, 0, 0)
            .unwrap()
            .fixed_offset();
        let mut diff = HashMap::new();
        diff.insert(path.clone(), (State::Created, Some(oid)));

        apply_tree_diff_to_blog_posts(&diff, commit_dt);

        let post = blog_post::get_by_path(&path).expect("post registered");
        assert_eq!(post.publication_date, Some(commit_dt));
    }

    #[test]
    fn apply_diff_draft_publish_sets_publication_to_commit() {
        let draft = PathBuf::from("posts/gitblog-test.draft.md");
        let published = PathBuf::from("posts/gitblog-test.md");
        let content = b"# Draft\n";
        let oid = gix::objs::compute_hash(gix::hash::Kind::Sha1, Kind::Blob, content);
        let draft_dt = Utc
            .with_ymd_and_hms(2024, 1, 1, 10, 0, 0)
            .unwrap()
            .fixed_offset();
        let publish_dt = Utc
            .with_ymd_and_hms(2024, 2, 1, 10, 0, 0)
            .unwrap()
            .fixed_offset();

        let mut create_draft = HashMap::new();
        create_draft.insert(draft.clone(), (State::Created, Some(oid)));
        apply_tree_diff_to_blog_posts(&create_draft, draft_dt);

        let mut publish = HashMap::new();
        publish.insert(draft.clone(), (State::Deleted, None));
        publish.insert(published.clone(), (State::Created, Some(oid)));
        apply_tree_diff_to_blog_posts(&publish, publish_dt);

        let post = blog_post::get_by_path(&published).expect("published post");
        assert_eq!(post.publication_date, Some(publish_dt));
    }

    #[test]
    fn git_repo_preserves_publication_across_rename() {
        let dir = tempfile::tempdir().expect("tempdir");
        let repo = dir.path();
        Command::new("git")
            .args(["init", "-b", "main"])
            .current_dir(repo)
            .output()
            .expect("git init");
        Command::new("git")
            .args(["config", "user.email", "test@example.com"])
            .current_dir(repo)
            .status()
            .expect("git config email");
        Command::new("git")
            .args(["config", "user.name", "Test"])
            .current_dir(repo)
            .status()
            .expect("git config name");

        let old_path = repo.join("article.md");
        std::fs::write(&old_path, "# Post\n").expect("write");
        Command::new("git")
            .args(["add", "article.md"])
            .current_dir(repo)
            .status()
            .expect("git add");
        Command::new("git")
            .args([
                "commit",
                "-m",
                "create",
                "--date",
                "2024-01-01T10:00:00+00:00",
            ])
            .current_dir(repo)
            .status()
            .expect("git commit create");

        let local = super::GitLocal {
            repo_root: repo.to_path_buf(),
            branch: "main".to_string(),
        };
        local.index_publication_dates();
        let created = blog_post::get_by_path(Path::new("article.md")).expect("created");
        let original_publication = created.effective_publication_date();

        std::fs::rename(repo.join("article.md"), repo.join("renamed.md")).expect("rename");
        Command::new("git")
            .args(["add", "-A"])
            .current_dir(repo)
            .status()
            .expect("git add rename");
        Command::new("git")
            .args([
                "commit",
                "-m",
                "rename",
                "--date",
                "2024-06-01T10:00:00+00:00",
            ])
            .current_dir(repo)
            .status()
            .expect("git commit rename");

        local.index_publication_dates();
        let renamed = blog_post::get_by_path(Path::new("renamed.md")).expect("renamed");
        assert_eq!(
            renamed.effective_publication_date(),
            original_publication,
            "publication date should survive a path rename"
        );
    }
}
