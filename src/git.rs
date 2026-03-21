use std::collections::HashMap;
use std::io::{BufReader, Read};
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

use gix::bstr::{BStr, BString, ByteSlice};
use gix::object::tree::diff::{Action, Change};
use gix::Commit;
use gix::{Tree, progress};
use gix::protocol::handshake::Ref;
use gix::protocol::transport::packetline::read::ProgressAction;
use gix::protocol::{Command, fetch, ls_refs};
use gix::protocol::transport::client::http;
use gix_pack::data::input::{BytesToEntriesIter, EntryDataMode, Mode as PackMode};

use chrono::{DateTime, FixedOffset};
use gix::url::Url;

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

impl GitRemote {

    pub fn fetch(self, up_to: &DateTime<FixedOffset>) -> CommitEnds {
        let mut transport = http::connect(self.url, gix::protocol::transport::Protocol::default(), true);

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
        println!("Prefix: {}", prefix);
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

        let head_oid = refs
            .iter()
            .find_map(|r| match r {
                Ref::Direct { full_ref_name, object, .. }
                    if *full_ref_name == b"refs/heads/main"[..] => Some(*object),
                _ => None,
            })
            .expect("refs/heads/main not found");

        println!("refs/heads/main: {head_oid}");

        // Command::Fetch.default_features() reads the server capabilities to build
        // the feature list that Arguments::new needs to gate can_use_shallow() etc.
        let fetch_features = Command::Fetch
            .default_features(outcome.server_protocol_version, &outcome.capabilities);

        let mut args = fetch::Arguments::new(
            outcome.server_protocol_version,
            fetch_features,
            false, // trace
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
            log::trace!("Using deepen_since: {}", up_to);
        } else if args.can_use_shallow() {
            args.deepen(1);
            log::trace!("Using depth 1");
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

        // EntryDataMode::Keep stores compressed bytes in entry.compressed.
        let iter = BytesToEntriesIter::new_from_header(
            BufReader::new(reader),
            PackMode::AsIs,
            EntryDataMode::Keep,
            gix::hash::Kind::Sha1,
        ).expect("pack header");


        let head_commit = None;
        for entry in iter {
            let entry = entry.expect("pack entry");
            if matches!(entry.header, Header::Commit) {
                head_commit = Some(entry)
            }
            let compressed = entry.compressed.expect("compressed bytes present with Keep mode");
            let mut buf = Vec::with_capacity(entry.decompressed_size as usize);
            flate2::read::ZlibDecoder::new(compressed.as_slice())
                .read_to_end(&mut buf)
                .expect("decompress commit");
            println!("--- {:?} ---", entry.header);
            println!("{}", String::from_utf8_lossy(&buf));
        }
    }

    pub fn tree_diff(
        &self,
        from: &Commit,
        to: &gix::objs::Commit
    ) -> Result<HashMap<PathBuf, State>, Box<dyn std::error::Error>> {
        let mut result: HashMap<PathBuf, State> = HashMap::new();
        if from.id == to.id {
            return Ok(result);
        }

        //  Single tree-level diff between the baseline and HEAD.
        //        No blobs are loaded: gix only compares tree entries (names,
        //        modes, OIDs). Rename/copy detection is unavailable without the
        //        `blob-diff` feature, so Change::Rewrite will never be emitted.
        let head_tree = to.tree;
        let base_tree = from.tree;
        base_tree
            .changes()?
            .for_each_to_obtain_tree(
                &to.tree,
                |change: Change<'_, '_, '_>| -> Result<Action, std::convert::Infallible> {
                    log::trace!(
                        "change: {}",
                        match &change {
                            Change::Addition { location, .. } => format!("addition {location:?}"),
                            Change::Modification { location, .. } => format!("modification {location:?}"),
                            Change::Deletion { location, .. } => format!("deletion {location:?}"),
                            Change::Rewrite { source_location, location, .. } =>
                                format!("rewrite {source_location:?} -> {location:?}"),
                        }
                    );
                    apply_change(&mut result, change);
                    Ok(Action::Continue)
                },
            )?;
        Ok(result)
    }
}

/// Applies a single diff `change` into the accumulated `result` map.
///
/// `result` holds the net change from some commit C to HEAD (newest-first walk).
/// `change` is what happened from C's parent to C.
/// After this call, the entry (or entries, for rewrites) in `result` represent
/// the net change from C's parent to HEAD.
///
/// State-machine rules per (change, current_state):
///
/// | change       | current     | action          |
/// |--------------|-------------|-----------------|
/// | addition     | —           | Created         |
/// | addition     | Modified    | Created         |
/// | addition     | Deleted     | ignore          |
/// | addition     | Created     | **ERROR**       |
/// | modification | —           | Modified        |
/// | modification | any         | ignore          |
/// | deletion     | —           | Deleted         |
/// | deletion     | Created     | ignore          |
/// | deletion     | Deleted     | ignore          |
/// | deletion     | Modified    | **ERROR**       |
///
/// For `Rewrite { source → dest }`:
/// - source is treated with a stricter deletion rule: ignore for all known states.
/// - dest is treated exactly like an addition.
fn apply_change(result: &mut HashMap<PathBuf, State>, change: Change<'_, '_, '_>) {
    match change {
        Change::Addition { location, .. } => {
            let path = PathBuf::from(location.to_str_lossy().as_ref());
            match result.get(&path) {
                None => {
                    result.insert(path, State::Created);
                }
                Some(State::Modified) => {
                    result.insert(path, State::Created);
                }
                Some(State::Deleted) => {}
                Some(State::Created) => {
                    panic!("apply_change: invalid state (addition, created) for {path:?}")
                }
            }
        }
        Change::Modification { location, .. } => {
            let path = PathBuf::from(location.to_str_lossy().as_ref());
            match result.get(&path) {
                None => {
                    result.insert(path, State::Modified);
                }
                Some(State::Created | State::Deleted | State::Modified) => {}
            }
        }
        Change::Deletion { location, .. } => {
            let path = PathBuf::from(location.to_str_lossy().as_ref());
            match result.get(&path) {
                None => {
                    result.insert(path, State::Deleted);
                }
                Some(State::Created | State::Deleted) => {}
                Some(State::Modified) => {
                    panic!("apply_change: invalid state (deletion, modified) for {path:?}")
                }
            }
        }
        Change::Rewrite {
            source_location,
            location,
            ..
        } => {
            let a = PathBuf::from(source_location.to_str_lossy().as_ref());
            let b = PathBuf::from(location.to_str_lossy().as_ref());

            // Source path: unknown → Deleted; any known state → ignore.
            match result.get(&a) {
                None => {
                    result.insert(a, State::Deleted);
                }
                Some(State::Created | State::Modified | State::Deleted) => {}
            }

            // Destination path: same rules as Addition.
            match result.get(&b) {
                None => {
                    result.insert(b, State::Created);
                }
                Some(State::Modified) => {
                    result.insert(b, State::Created);
                }
                Some(State::Deleted) => {}
                Some(State::Created) => {
                    panic!("apply_change: invalid state (rewrite, created destination) for {b:?}")
                }
            }
        }
    }
}
