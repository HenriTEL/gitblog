use std::collections::HashMap;
use std::collections::VecDeque;
use std::cell::RefCell;
use std::fs;
use std::hash::Hash;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::time::{SystemTime, UNIX_EPOCH};

use gix::bstr::{BStr, BString, ByteSlice};
use gix::objs::{CommitRef, TreeRef};
use gix::objs::Tree;
use gix::object::tree::diff::{Action, Change};
use gix::Commit;
use gix::progress;
use gix::protocol::handshake::Ref;
use gix::protocol::transport::packetline::read::ProgressAction;
use gix::protocol::{Command, fetch, ls_refs};
use gix::protocol::transport::client::http;
use gix_pack::cache;
use gix_pack::data::decode::entry::ResolvedBase;

use chrono::{DateTime, FixedOffset};
use gix::url::Url;
use tempfile::NamedTempFile;

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

impl GitRemote {

    pub fn get_files(
        &self,
        blobs: &[FileBlob],
        destination: Option<PathBuf>,
    ) -> Result<PathBuf, Box<dyn std::error::Error>> {
        let dest = match destination {
            Some(path) => path,
            None => std::env::temp_dir().join(format!("gitblog-files-{}", SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos())),
        };
        fs::create_dir_all(&dest)?;

        if blobs.is_empty() {
            return Ok(dest);
        }

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
        let mut args = fetch::Arguments::new(outcome.server_protocol_version, fetch_features, false);
        for blob in blobs {
            args.want(blob.oid);
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
            decoded_objects.borrow_mut().insert(oid, (kind, out.clone()));

            if matches!(kind, gix::objs::Kind::Blob) {
                if let Some(path) = requested.get(&oid) {
                    let output_path = dest.join(path);
                    if let Some(parent) = output_path.parent() {
                        fs::create_dir_all(parent)?;
                    }
                    fs::write(output_path, out.as_slice())?;
                    written.insert(oid, true);
                } else {
                    println!("blob object not requested: {:?}", oid);
                }
            } else {
                println!("non-blob object: {:?}", kind);
            }

            let mut entry_payload = vec![0u8; entry.decompressed_size as usize];
            let consumed =
                pack.decompress_entry(&entry, &mut inflate_step, entry_payload.as_mut_slice())?;
            offset = entry.pack_offset() + entry.header_size() as u64 + consumed as u64;
        }

        for blob in blobs {
            if !written.contains_key(&blob.oid) {
                return Err(format!("blob {} not found in fetched pack", blob.oid).into());
            }
        }

        Ok(dest)
    }

    pub fn fetch(&self, up_to: &DateTime<FixedOffset>) -> TreeEnds {
        let mut transport = http::connect(self.url.clone(), gix::protocol::transport::Protocol::default(), true);

        // Capture handshake outcome: we need server_protocol_version and capabilities
        // to pass real server features to fetch::Arguments::new later.
        let outcome = gix::protocol::handshake(
            &mut transport,
            gix::protocol::transport::Service::UploadPack,
            &mut |_| Ok(None),
            vec![],
            &mut progress::Discard,
        ).expect("initial handshake");

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
        ).expect("ls_refs command");


    // let refs = ls_refs(
    //     &mut transport,
    //     &outcome.capabilities,
    //     |_caps, args, _features| {
    //         args.push(b"ref-prefix refs/heads/main".into());
    //         Ok(ls_refs::Action::Continue)
    //     },
    //     &mut progress::Discard,
    //     false,
    // ).expect("ls_refs command");

        let target_ref = format!("refs/heads/{}", self.branch);
        let head_oid = refs
            .iter()
            .find_map(|r| match r {
                Ref::Direct { full_ref_name, object, .. }
                    if *full_ref_name == target_ref.as_bytes() => Some(*object),
                _ => None,
            })
            .expect(&format!("{} not found", target_ref));

        println!("{}: {head_oid}", target_ref);

        // Command::Fetch.default_features() reads the server capabilities to build
        // the feature list that Arguments::new needs to gate can_use_shallow() etc.
        let fetch_features = Command::Fetch
            .default_features(outcome.server_protocol_version, &outcome.capabilities);

        let mut args = fetch::Arguments::new(
            outcome.server_protocol_version,
            fetch_features,
            false,
        );

        args.want(head_oid);
        // Limit history depth so we don't download the full repository.
        // Use deepen-since when up_to is a real (positive) Unix timestamp so that
        // only commits newer than the last known update are included.
        // Fall back to deepen(1) for the MIN_UTC sentinel (no blog_url given) –
        // its timestamp is ≈ -8.3e12, which GitHub rejects as invalid.
        let ts = up_to.timestamp();
        if ts > 0 && args.can_use_deepen_since() {
            args.deepen_since(ts);
            println!("Using deepen_since: {}", up_to);
        } else if args.can_use_shallow() {
            args.deepen(1);
            println!("Using depth 1");
        }
        // blob:none: skip all blob objects to reduce pack size.
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
        ).expect("fetch response");

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
                if head_tree_id.is_none() {
                    head_tree_id = Some(commit_tree_id);
                }
                up_to_tree_id = Some(commit_tree_id);
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

        TreeEnds {
            up_to_tree: up_to_tree.expect("up_to tree object was not found in pack"),
            head_tree: head_tree.expect("head tree object was not found in pack"),
        }
    }

    pub fn tree_diff(
        &self,
        from: &Tree,
        to: &Tree,
    ) -> Result<HashMap<PathBuf, (State, Option<gix::ObjectId>)>, Box<dyn std::error::Error>> {
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
                                        result.insert(full_path, (State::Modified, Some(right_entry.oid)));
                                    }
                                    (true, false) => {
                                        queue.push_back((full_path.clone(), cached_tree(&left_entry.oid), None));
                                        result.insert(full_path, (State::Created, Some(right_entry.oid)));
                                    }
                                    (false, true) => {
                                        result.insert(full_path.clone(), (State::Deleted, None));
                                        queue.push_back((full_path, None, cached_tree(&right_entry.oid)));
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

        Ok(result)
    }
}
