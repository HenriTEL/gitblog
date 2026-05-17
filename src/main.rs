use std::{
    error::Error,
    fmt,
    fs,
    io::{self, BufRead, Write},
    path::{Path, PathBuf},
    process::ExitCode,
};

use chrono::{DateTime, FixedOffset, Utc};
use gitblog::{
    blog_post::BlogPost,
    feed,
    gemini::{markdown_file_to_gemtext, write_index_gemtext},
    git::{self, GitLocal, State},
    html::{markdown_file_to_html, write_index_from_blog_posts},
    repo_uri::{self, RepoUriError},
    static_content::write_static_content,
    user_profile::{
        GithubUserProfile, UserProfile, UserProfileDownloadError, UserProfileMeta, download_avatar,
    },
};

use clap::Parser;
use gix::bstr::{BStr, BString};
use gix::remote::Direction;
use gix::url::Scheme;

const MIN_UTC: chrono::DateTime<chrono::Utc> = chrono::DateTime::<chrono::Utc>::MIN_UTC;

/// URI did not designate a github.com Git remote, parsing failed, or profile download failed.
#[derive(Debug)]
pub enum UpdateProfileError {
    GitUrl(gix::url::parse::Error),
    RepoUri(RepoUriError),
    RemoteLookup(String),
    UnsupportedRepositoryScheme(String),
    UnsupportedHost(String),
    NonUtf8RepoPath,
    MissingRepoOwner,
    Download(UserProfileDownloadError),
}

impl fmt::Display for UpdateProfileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            UpdateProfileError::GitUrl(e) => write!(f, "invalid Git URL: {e}"),
            UpdateProfileError::RepoUri(e) => write!(f, "{e}"),
            UpdateProfileError::RemoteLookup(msg) => write!(f, "failed to read git remote: {msg}"),
            UpdateProfileError::UnsupportedRepositoryScheme(s) => {
                write!(f, "repository scheme `{s}` is not supported")
            }
            UpdateProfileError::UnsupportedHost(h) => {
                write!(
                    f,
                    "unsupported host `{h}` (only github.com profiles are implemented)"
                )
            }
            UpdateProfileError::NonUtf8RepoPath => write!(f, "repository path is not valid UTF-8"),
            UpdateProfileError::MissingRepoOwner => write!(
                f,
                "could not derive a GitHub username or organization from the repository path"
            ),
            UpdateProfileError::Download(e) => write!(f, "{e}"),
        }
    }
}

impl Error for UpdateProfileError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            UpdateProfileError::GitUrl(e) => Some(e),
            UpdateProfileError::RepoUri(e) => Some(e),
            UpdateProfileError::Download(e) => Some(e),
            _ => None,
        }
    }
}

/// Fetch profile picture and metadata for the Git hosting implied by `repo_uri` (currently only
/// [GitHub](https://github.com)) and writes the avatar under `dest/media/avatar`.
pub fn update_profile(
    repo_uri: &str,
    dest: impl AsRef<Path>,
    force_full: bool,
) -> Result<UserProfileMeta, UpdateProfileError> {
    let url = gitblog::repo_uri::parse_repo_url(repo_uri).map_err(UpdateProfileError::RepoUri)?;

    match url.scheme {
        gix::url::Scheme::Http | gix::url::Scheme::Https | gix::url::Scheme::Ssh => {}
        s => {
            return Err(UpdateProfileError::UnsupportedRepositoryScheme(
                s.to_string(),
            ));
        }
    }

    let host = url.host().unwrap_or("");
    let user_profile = if is_github_dot_com_host(host) {
        let owner = github_owner_from_git_path(url.path.as_ref())?;
        Ok(GithubUserProfile::new(owner))
    } else {
        Err(UpdateProfileError::UnsupportedHost(host.to_owned()))
    };
    let profile = user_profile?;
    let username = profile
        .get_username()
        .map_err(|e| UpdateProfileError::Download(e.into()))?;
    let bio = profile
        .get_about()
        .map_err(|e| UpdateProfileError::Download(e.into()))?;

    let avatar_path = dest.as_ref().join("media").join("avatar");
    if force_full || !avatar_path.is_file() {
        download_avatar(&profile, &avatar_path).map_err(UpdateProfileError::Download)?;
    } else {
        log::debug!(
            "skipping user profile update; {} already exists and --full was not passed",
            avatar_path.display()
        );
    }
    Ok(UserProfileMeta { username, bio })
}

/// Resolve the fetch remote URL from git config and update the profile when it points at GitHub.
pub fn update_profile_for_local_repo(
    repo: &gix::Repository,
    branch: &str,
    dest: impl AsRef<Path>,
    force_full: bool,
) -> Result<UserProfileMeta, UpdateProfileError> {
    match resolve_fetch_remote_url(repo, branch)? {
        Some(url) if matches!(url.scheme, Scheme::Http | Scheme::Https | Scheme::Ssh) => {
            let uri = url.to_bstring().to_string();
            update_profile(&uri, dest, force_full)
        }
        Some(url) => Err(UpdateProfileError::UnsupportedRepositoryScheme(
            url.scheme.to_string(),
        )),
        None => {
            log::info!(
                "no git remote configured; using fallback profile (set remote.origin.url for GitHub avatar)"
            );
            Ok(fallback_local_profile(repo))
        }
    }
}

fn resolve_fetch_remote_url(
    repo: &gix::Repository,
    branch: &str,
) -> Result<Option<gix::Url>, UpdateProfileError> {
    let mut candidates: Vec<BString> = Vec::new();
    if let Some(name) = repo.branch_remote_name(BStr::new(branch.as_bytes()), Direction::Fetch) {
        candidates.push(name.as_ref().to_owned());
    }
    if let Some(name) = repo.remote_default_name(Direction::Fetch) {
        candidates.push(name.into_owned());
    }
    candidates.push(BString::from("origin"));

    for name in candidates {
        match repo.try_find_remote(BStr::new(&name)) {
            Some(Ok(remote)) => {
                if let Some(url) = remote.url(Direction::Fetch) {
                    return Ok(Some(url.clone()));
                }
            }
            Some(Err(e)) => {
                return Err(UpdateProfileError::RemoteLookup(e.to_string()));
            }
            None => continue,
        }
    }
    Ok(None)
}

fn fallback_local_profile(repo: &gix::Repository) -> UserProfileMeta {
    let username = repo
        .worktree()
        .map(|wt| wt.base().to_path_buf())
        .or_else(|| repo.path().parent().map(|p| p.to_path_buf()))
        .and_then(|p| p.file_name().map(|n| n.to_os_string()))
        .and_then(|n| n.into_string().ok())
        .unwrap_or_else(|| "blog".to_string());
    UserProfileMeta {
        username,
        bio: String::new(),
    }
}

fn is_github_dot_com_host(host: &str) -> bool {
    host.eq_ignore_ascii_case("github.com") || host.eq_ignore_ascii_case("www.github.com")
}

/// First segment of [`gix::Url::path`] (e.g. `owner/repo` → `owner`, `/owner/repo.git` → `owner`).
fn github_owner_from_git_path(path: &[u8]) -> Result<String, UpdateProfileError> {
    let path_str = std::str::from_utf8(path).map_err(|_| UpdateProfileError::NonUtf8RepoPath)?;
    let trimmed = path_str.trim().trim_matches('/');

    trimmed
        .split('/')
        .next()
        .filter(|s| !s.is_empty())
        .map(|segment| segment.trim_end_matches(".git").to_string())
        .filter(|owner| !owner.is_empty())
        .ok_or(UpdateProfileError::MissingRepoOwner)
}

#[derive(Parser)]
#[command(version, about, long_about = None)]
struct CliArgs {
    /// Path or URL of the git repository containing the blog sources
    repo: String,
    /// Branch on the git repository to use
    #[arg(long, default_value = "main")]
    branch: String,
    /// URL where your blog is hosted
    #[arg(long)]
    blog_url: String,

    /// Generate all blog files, not just the ones that changed
    #[arg(long)]
    full: bool,
    /// Generate Gemini (`.gmi`) output alongside HTML
    #[arg(long)]
    gemini: bool,
    /// Use the local working tree only (no object database read). Only valid with a local repo path.
    #[arg(long)]
    no_fetch: bool,
    /// Directory to write generated blog files (created if missing)
    #[arg(long)]
    output: Option<PathBuf>,
    /// Delete existing top-level files in `--output` without prompting
    #[arg(long)]
    overwrite: bool,
    /// Frontmatter delimiter used in markdown files (for example: --- or +++)
    #[arg(long, default_value = "---")]
    frontmatter_delimiter: String,
}

fn main() -> ExitCode {
    let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"))
        .add_directive(
            "gix_packetline=off"
                .parse()
                .expect("valid tracing directive"),
        )
        .add_directive(
            "gix_packetline::read::blocking_io=off"
                .parse()
                .expect("valid tracing directive"),
        );

    tracing_subscriber::fmt()
        .with_env_filter(env_filter)
        .with_target(true)
        .init();

    let args = CliArgs::parse();

    let url = match repo_uri::parse_repo_url(&args.repo) {
        Ok(url) => url,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };

    if args.no_fetch && url.scheme != Scheme::File {
        eprintln!("--no-fetch requires a local repository path");
        return ExitCode::FAILURE;
    }

    if args.overwrite && args.output.is_none() {
        eprintln!("--overwrite requires --output");
        return ExitCode::FAILURE;
    }

    let output_dest = match args.output.as_ref() {
        Some(path) => match prepare_output_dir(path, args.overwrite) {
            Ok(path) => Some(path),
            Err(()) => return ExitCode::FAILURE,
        },
        None => None,
    };

    let atom_feed = fetch_atom_feed(&args.blog_url);
    let (up_to, previous_feed) = if args.full {
        (MIN_UTC.fixed_offset(), atom_feed.ok())
    } else {
        match atom_feed {
            Ok(feed) => {
                let updated = feed.updated;
                (updated, Some(feed))
            }
            Err(_) => {
                log::error!("Failed to fetch atom feed from {}", args.blog_url);
                log::info!("Falling back to full blog generation");
                (MIN_UTC.fixed_offset(), None)
            }
        }
    };

    match url.scheme {
        Scheme::Http | Scheme::Https => {
            let remote = gitblog::git::GitRemote {
                url,
                branch: args.branch.clone(),
            };
            if let Some(feed) = &previous_feed {
                hydrate_blog_posts_from_atom_feed(feed, &args.blog_url);
            }
            let updated_file_blobs = if up_to == MIN_UTC.fixed_offset() {
                vec![]
            } else {
                let tree_ends = remote.fetch_since(&up_to);
                let diff = remote.tree_diff(&tree_ends.up_to_tree, &tree_ends.head_tree);
                collect_updated_file_blobs(&diff)
            };
            let dest = remote
                .pull_files(&updated_file_blobs, output_dest.clone())
                .expect("fetch blobs");
            let user_profile = update_profile(
                &args.repo,
                <PathBuf as AsRef<Path>>::as_ref(&dest),
                args.full,
            )
            .expect("update profile");
            finish_build(
                &dest,
                &args,
                up_to,
                previous_feed.as_ref(),
                &user_profile,
                updated_file_blobs,
            );
        }
        Scheme::File => {
            let repo_root = match repo_uri::file_url_to_path(&url) {
                Ok(path) => path,
                Err(e) => {
                    eprintln!("{e}");
                    return ExitCode::FAILURE;
                }
            };
            let repo = match gix::discover(&repo_root) {
                Ok(repo) => repo,
                Err(e) => {
                    eprintln!("failed to open repository at {}: {e}", repo_root.display());
                    return ExitCode::FAILURE;
                }
            };

            if let Some(feed) = &previous_feed {
                hydrate_blog_posts_from_atom_feed(feed, &args.blog_url);
            }

            let (dest, updated_file_blobs) = if args.no_fetch {
                let dest = git::materialize_worktree_copy(&repo, output_dest.clone())
                    .expect("copy worktree");
                let blobs = if args.full || up_to == MIN_UTC.fixed_offset() {
                    vec![]
                } else {
                    collect_worktree_blobs_since(&dest, &up_to)
                };
                (dest, blobs)
            } else {
                let local = GitLocal {
                    repo_root: repo_root.clone(),
                    branch: args.branch.clone(),
                };
                let updated_file_blobs = if up_to == MIN_UTC.fixed_offset() {
                    vec![]
                } else {
                    let tree_ends = local.fetch_since(&up_to);
                    let diff = local.tree_diff(&tree_ends.up_to_tree, &tree_ends.head_tree);
                    collect_updated_file_blobs(&diff)
                };
                let dest = local
                    .pull_files(&updated_file_blobs, output_dest.clone())
                    .expect("materialize from object database");
                (dest, updated_file_blobs)
            };

            if up_to == MIN_UTC.fixed_offset() {
                let local = GitLocal {
                    repo_root: repo_root.clone(),
                    branch: args.branch.clone(),
                };
                local.index_publication_dates();
            }

            let user_profile = update_profile_for_local_repo(&repo, &args.branch, &dest, args.full)
                .expect("update profile");
            finish_build(
                &dest,
                &args,
                up_to,
                previous_feed.as_ref(),
                &user_profile,
                updated_file_blobs,
            );
        }
        ref scheme => {
            eprintln!(
                "The URL {} resolved to protocol {scheme} which is not supported.",
                url
            );
            return ExitCode::FAILURE;
        }
    }

    ExitCode::SUCCESS
}

/// Create `path` when missing; when it exists and is non-empty, clear it after confirmation or `--overwrite`.
fn prepare_output_dir(path: &Path, overwrite: bool) -> Result<PathBuf, ()> {
    let path = path.to_path_buf();
    if path.exists() && !path.is_dir() {
        eprintln!("--output path {} is not a directory", path.display());
        return Err(());
    }

    fs::create_dir_all(&path).map_err(|e| {
        eprintln!("failed to create output directory {}: {e}", path.display());
    })?;

    if !output_dir_is_nonempty(&path) {
        return Ok(path);
    }

    let entries = list_output_dir_toplevel(&path);
    if overwrite {
        clear_output_dir(&path).map_err(|e| {
            eprintln!("failed to clear output directory {}: {e}", path.display());
        })?;
        return Ok(path);
    }

    eprintln!("Output directory {} is not empty:", path.display());
    for name in &entries {
        eprintln!("  {name}");
    }
    eprintln!("Pass --overwrite to clear the output directory without this prompt.");
    eprint!("Delete these files and continue? [y/N] ");
    io::stderr().flush().ok();
    let mut line = String::new();
    let confirmed = io::stdin()
        .lock()
        .read_line(&mut line)
        .map(|n| n > 0)
        .unwrap_or(false)
        && matches!(line.trim().to_ascii_lowercase().as_str(), "y" | "yes");

    if !confirmed {
        eprintln!("Aborted.");
        return Err(());
    }

    clear_output_dir(&path).map_err(|e| {
        eprintln!("failed to clear output directory {}: {e}", path.display());
    })?;
    Ok(path)
}

fn output_dir_is_nonempty(path: &Path) -> bool {
    fs::read_dir(path)
        .map(|mut entries| entries.next().is_some())
        .unwrap_or(false)
}

fn list_output_dir_toplevel(path: &Path) -> Vec<String> {
    let Ok(entries) = fs::read_dir(path) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

fn clear_output_dir(path: &Path) -> io::Result<()> {
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let target = entry.path();
        if target.is_dir() {
            fs::remove_dir_all(target)?;
        } else {
            fs::remove_file(target)?;
        }
    }
    Ok(())
}

fn finish_build(
    dest: &Path,
    args: &CliArgs,
    up_to: DateTime<FixedOffset>,
    previous_feed: Option<&feed::Feed>,
    user_profile: &UserProfileMeta,
    updated_file_blobs: Vec<git::FileBlob>,
) {
    if args.no_fetch && up_to != MIN_UTC.fixed_offset() && !args.full {
        refresh_blog_posts_from_worktree_since(dest, &up_to, &args.frontmatter_delimiter);
    } else {
        refresh_blog_posts_from_markdown(
            dest,
            &updated_file_blobs,
            args.full,
            &args.frontmatter_delimiter,
        );
    }
    let posts = publishable_blog_posts(git::all_blog_posts());
    render_markdown_files(
        user_profile,
        dest,
        &posts,
        &args.frontmatter_delimiter,
        args.gemini,
    );
    let generated_feed = feed::build_feed_from_blog_posts(&args.blog_url, &posts, previous_feed);
    let xml = feed::generate(&generated_feed).expect("generate atom feed");
    std::fs::write(dest.join("atom.xml"), &xml).expect("write atom feed");
    println!("wrote atom.xml");
    write_index_from_blog_posts(dest, user_profile, &posts);
    println!("wrote index.html");
    if args.gemini {
        write_index_gemtext(dest, &posts).expect("write gemini index");
        println!("wrote index.gmi");
    }
    println!("Using destination: {:?}", dest.to_str());
    if up_to == MIN_UTC.fixed_offset() {
        write_static_content(dest);
    }
    println!("Blog built at {} ", dest.display());
}

/// Blobs for markdown files in `dest` modified after `up_to` (for `--no-fetch` incremental builds).
fn collect_worktree_blobs_since(dest: &Path, up_to: &DateTime<FixedOffset>) -> Vec<git::FileBlob> {
    let mut blobs = Vec::new();
    walk_markdown_files(dest, &mut |abs, rel| {
        let ts = file_last_updated(abs);
        if ts <= *up_to {
            return;
        }
        let content = std::fs::read(abs).expect("read markdown");
        let oid = gix::objs::compute_hash(gix::hash::Kind::Sha1, gix::objs::Kind::Blob, &content);
        blobs.push(git::FileBlob {
            file_path: rel.to_path_buf(),
            oid,
        });
    });
    blobs
}

fn refresh_blog_posts_from_worktree_since(
    dest: &Path,
    up_to: &DateTime<FixedOffset>,
    frontmatter_delimiter: &str,
) {
    walk_markdown_files(dest, &mut |abs, rel| {
        let ts = file_last_updated(abs);
        if ts <= *up_to {
            return;
        }
        let md = std::fs::read_to_string(abs).expect("read markdown");
        git::update_blog_post_from_markdown_path(rel.to_path_buf(), &md, ts, frontmatter_delimiter);
    });
}

fn render_markdown_files(
    user_profile: &UserProfileMeta,
    dest: &Path,
    _posts: &[BlogPost],
    frontmatter_delimiter: &str,
    with_gemini: bool,
) {
    walk_markdown_files(dest, &mut |abs, rel| {
        markdown_file_to_html(user_profile, abs, rel, frontmatter_delimiter);
        println!("wrote {} ", rel.with_extension("html").to_string_lossy());
        if with_gemini {
            markdown_file_to_gemtext(abs, frontmatter_delimiter).expect("write gemtext");
            println!("wrote {} ", rel.with_extension("gmi").to_string_lossy());
        }
        std::fs::remove_file(abs).expect("remove rendered markdown");
    });
}

fn collect_updated_file_blobs(
    diff: &std::collections::HashMap<PathBuf, (State, Option<gix::ObjectId>)>,
) -> Vec<gitblog::git::FileBlob> {
    diff.iter()
        .filter_map(|(path, (state, maybe_oid))| {
            if gitblog::path_is_ignored(path, false) {
                return None;
            }
            match (state, maybe_oid) {
                (State::Created, Some(oid)) => Some(gitblog::git::FileBlob {
                    file_path: path.clone(),
                    oid: oid.to_owned(),
                }),
                (State::Deleted, None) => None,
                (State::Modified, Some(oid)) => Some(gitblog::git::FileBlob {
                    file_path: path.clone(),
                    oid: oid.to_owned(),
                }),
                (State::Created, None) => None,
                (State::Deleted, Some(_)) => None,
                (State::Modified, None) => None,
            }
        })
        .collect::<Vec<_>>()
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::io::Write;
    use std::path::{Path, PathBuf};

    use gix::bstr::BStr;
    use gix::url::Scheme;

    use super::{
        CliArgs, clear_output_dir, collect_updated_file_blobs, fallback_local_profile,
        github_owner_from_git_path, is_github_dot_com_host, list_output_dir_toplevel,
        output_dir_is_nonempty, prepare_output_dir, render_markdown_files,
        resolve_fetch_remote_url,
    };
    use clap::Parser;
    use gitblog::blog_post::BlogPost;
    use gitblog::repo_uri::{self, parse_repo_url};
    use gitblog::user_profile::UserProfileMeta;
    use tempfile::tempdir_in;

    fn test_git_dir() -> PathBuf {
        let base = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/test-repos");
        std::fs::create_dir_all(&base).expect("create test-repos dir");
        base
    }

    #[test]
    fn parse_repo_url_https_unchanged() {
        let url = parse_repo_url("https://example.com/repo.git").unwrap();
        assert!(matches!(url.scheme, Scheme::Http | Scheme::Https));
    }

    #[test]
    fn parse_repo_url_bare_path_is_file() {
        let dir = std::env::temp_dir();
        let url = parse_repo_url(dir.to_str().unwrap()).unwrap();
        assert_eq!(url.scheme, Scheme::File);
    }

    #[test]
    fn parse_repo_url_file_scheme_is_file() {
        let dir = std::env::temp_dir();
        let input = format!("file://{}", dir.display());
        let url = parse_repo_url(&input).unwrap();
        assert_eq!(url.scheme, Scheme::File);
    }

    #[test]
    fn file_url_to_path_roundtrip() {
        let dir = std::env::temp_dir().canonicalize().unwrap();
        let url = parse_repo_url(dir.to_str().unwrap()).unwrap();
        let path = repo_uri::file_url_to_path(&url).unwrap();
        assert_eq!(path, dir);
    }

    fn init_git_repo(path: &Path) {
        let git_dir = path.join(".git");
        std::fs::create_dir_all(git_dir.join("objects/pack")).expect("objects");
        std::fs::create_dir_all(git_dir.join("refs/heads")).expect("refs");
        std::fs::write(
            git_dir.join("config"),
            "[core]\n\trepositoryformatversion = 0\n\tfilemode = true\n\tbare = false\n",
        )
        .expect("config");
        std::fs::write(git_dir.join("HEAD"), "ref: refs/heads/main\n").expect("HEAD");

        let repo = gix::discover(path).expect("discover");
        let empty_tree = gix::ObjectId::empty_tree(repo.object_hash());
        let sig = gix::actor::SignatureRef {
            name: "test".into(),
            email: "test@example.com".into(),
            time: gix::date::Time::now_local_or_utc(),
        };
        let commit = gix::objs::Commit {
            tree: empty_tree,
            parents: Default::default(),
            author: sig.to_owned(),
            committer: sig.to_owned(),
            message: "init".into(),
            encoding: None,
            extra_headers: vec![],
        };
        let id = repo.write_object(&commit).expect("write commit").detach();
        std::fs::write(git_dir.join("refs/heads/main"), format!("{id}\n")).expect("main ref");
    }

    #[test]
    fn resolve_fetch_remote_url_reads_origin() {
        let dir = tempdir_in(test_git_dir()).expect("tempdir");
        init_git_repo(dir.path());
        let config_path = dir.path().join(".git/config");
        let mut config = std::fs::read_to_string(&config_path).expect("read config");
        config.push_str(
            "\n[remote \"origin\"]\n\turl = https://github.com/alice/lab.git\n\tfetch = +refs/heads/*:refs/remotes/origin/*\n",
        );
        std::fs::write(&config_path, config).expect("write config");
        let repo = gix::discover(dir.path()).expect("discover");
        let url = resolve_fetch_remote_url(&repo, "main")
            .expect("resolve")
            .expect("origin url");
        assert_eq!(url.scheme, Scheme::Https);
        assert!(url.path.starts_with(b"/alice/"));
    }

    #[test]
    fn fallback_local_profile_uses_directory_name() {
        let dir = tempdir_in(test_git_dir()).expect("tempdir");
        init_git_repo(dir.path());
        let repo = gix::discover(dir.path()).expect("discover");
        let profile = fallback_local_profile(&repo);
        assert_eq!(
            profile.username,
            dir.path().file_name().unwrap().to_str().unwrap()
        );
        assert!(profile.bio.is_empty());
    }

    #[test]
    fn gemini_flag_defaults_to_false() {
        let args = CliArgs::parse_from([
            "gitblog",
            "https://example.com/repo.git",
            "--blog-url",
            "https://example.com",
        ]);
        assert!(!args.gemini);
    }

    #[test]
    fn gemini_flag_sets_true_when_present() {
        let args = CliArgs::parse_from([
            "gitblog",
            "https://example.com/repo.git",
            "--blog-url",
            "https://example.com",
            "--gemini",
        ]);
        assert!(args.gemini);
    }

    #[test]
    fn no_fetch_flag_parses() {
        let args = CliArgs::parse_from([
            "gitblog",
            "/tmp/my-blog",
            "--blog-url",
            "https://example.com",
            "--no-fetch",
        ]);
        assert!(args.no_fetch);
    }

    #[test]
    fn output_and_overwrite_flags_parse() {
        let args = CliArgs::parse_from([
            "gitblog",
            "https://example.com/repo.git",
            "--blog-url",
            "https://example.com",
            "--output",
            "/tmp/out",
            "--overwrite",
        ]);
        assert_eq!(args.output.as_deref(), Some(Path::new("/tmp/out")));
        assert!(args.overwrite);
    }

    #[test]
    fn prepare_output_dir_creates_missing_directory() {
        let base = tempdir_in(test_git_dir()).expect("tempdir");
        let out = base.path().join("new-output");
        prepare_output_dir(&out, false).expect("prepare");
        assert!(out.is_dir());
        assert!(!output_dir_is_nonempty(&out));
    }

    #[test]
    fn prepare_output_dir_overwrite_clears_toplevel_files() {
        let base = tempdir_in(test_git_dir()).expect("tempdir");
        let out = base.path().join("out");
        std::fs::create_dir_all(&out).expect("mkdir");
        std::fs::write(out.join("index.html"), "<html>").expect("write");
        std::fs::create_dir_all(out.join("css")).expect("mkdir css");
        std::fs::write(out.join("css/theme.css"), "body{}").expect("write css");

        prepare_output_dir(&out, true).expect("prepare with overwrite");

        assert!(!output_dir_is_nonempty(&out));
    }

    #[test]
    fn list_output_dir_toplevel_sorts_names() {
        let base = tempdir_in(test_git_dir()).expect("tempdir");
        std::fs::write(base.path().join("z.html"), "").expect("write");
        std::fs::write(base.path().join("a.html"), "").expect("write");
        assert_eq!(
            list_output_dir_toplevel(base.path()),
            vec!["a.html".to_string(), "z.html".to_string()]
        );
    }

    #[test]
    fn clear_output_dir_removes_only_toplevel_entries() {
        let base = tempdir_in(test_git_dir()).expect("tempdir");
        std::fs::write(base.path().join("index.html"), "").expect("write");
        let nested = base.path().join("media");
        std::fs::create_dir_all(&nested).expect("mkdir");
        std::fs::write(nested.join("avatar"), "").expect("write");

        clear_output_dir(base.path()).expect("clear");

        assert!(!output_dir_is_nonempty(base.path()));
    }

    #[test]
    fn collect_updated_file_blobs_skips_ignored_paths() {
        let mut diff: HashMap<PathBuf, (gitblog::git::State, Option<gix::ObjectId>)> =
            HashMap::new();
        let keep_oid = gix::objs::compute_hash(gix::hash::Kind::Sha1, gix::objs::Kind::Blob, b"ok");
        let ignored_oid =
            gix::objs::compute_hash(gix::hash::Kind::Sha1, gix::objs::Kind::Blob, b"ignored");
        diff.insert(
            PathBuf::from("posts/hello.md"),
            (gitblog::git::State::Modified, Some(keep_oid)),
        );
        diff.insert(
            PathBuf::from("draft/wip.md"),
            (gitblog::git::State::Created, Some(ignored_oid)),
        );

        let blobs = collect_updated_file_blobs(&diff);

        assert_eq!(blobs.len(), 1);
        assert_eq!(blobs[0].file_path, PathBuf::from("posts/hello.md"));
        assert_eq!(blobs[0].oid, keep_oid);
    }

    #[test]
    fn render_markdown_files_removes_markdown_after_html_write() {
        let dir = tempdir_in(test_git_dir()).expect("create temp dir");
        let markdown_path = dir.path().join("post.md");
        let mut source = std::fs::File::create(&markdown_path).expect("create markdown");
        writeln!(source, "# Post").expect("write markdown");

        render_markdown_files(
            &UserProfileMeta {
                username: "u".into(),
                bio: String::new(),
            },
            dir.path(),
            &[] as &[BlogPost],
            "---",
            false,
        );

        assert!(!markdown_path.exists(), "markdown should be removed");
        assert!(
            dir.path().join("post.html").exists(),
            "html should be written"
        );
    }

    #[test]
    fn render_markdown_files_removes_markdown_after_html_and_gemini() {
        let dir = tempdir_in(test_git_dir()).expect("create temp dir");
        let markdown_path = dir.path().join("post.md");
        let mut source = std::fs::File::create(&markdown_path).expect("create markdown");
        writeln!(source, "# Post").expect("write markdown");

        render_markdown_files(
            &UserProfileMeta {
                username: "u".into(),
                bio: String::new(),
            },
            dir.path(),
            &[] as &[BlogPost],
            "---",
            true,
        );

        assert!(!markdown_path.exists(), "markdown should be removed");
        assert!(
            dir.path().join("post.html").exists(),
            "html should be written"
        );
        assert!(
            dir.path().join("post.gmi").exists(),
            "gemini should be written"
        );
    }

    #[test]
    fn github_owner_from_https_style_path() {
        assert_eq!(
            github_owner_from_git_path(b"/HenriTEL/gitblog.git").unwrap(),
            "HenriTEL"
        );
    }

    #[test]
    fn github_owner_from_scp_style_path() {
        assert_eq!(
            github_owner_from_git_path(b"HenriTEL/gitblog").unwrap(),
            "HenriTEL"
        );
    }

    #[test]
    fn github_owner_strips_dot_git_only_on_segment() {
        assert_eq!(
            github_owner_from_git_path(b"org-name/repo.name.git").unwrap(),
            "org-name"
        );
    }

    #[test]
    fn parsed_https_and_ssh_github_urls_yield_same_owner() {
        let https = gix::Url::from_bytes(BStr::new(b"https://github.com/alice/lab.git")).unwrap();
        let ssh = gix::Url::from_bytes(BStr::new(b"git@github.com:alice/lab.git")).unwrap();
        assert_eq!(
            github_owner_from_git_path(https.path.as_ref()).unwrap(),
            "alice"
        );
        assert_eq!(
            github_owner_from_git_path(ssh.path.as_ref()).unwrap(),
            "alice"
        );
    }

    #[test]
    fn is_github_host_accepts_www() {
        assert!(is_github_dot_com_host("github.com"));
        assert!(is_github_dot_com_host("WWW.GITHUB.COM"));
        assert!(is_github_dot_com_host("www.github.com"));
        assert!(!is_github_dot_com_host("gitlab.com"));
    }
}

fn fetch_atom_feed(blog_url: &str) -> Result<feed::Feed, Box<dyn Error>> {
    let url = format!("{}/atom.xml", blog_url.trim_end_matches('/'));
    let body = reqwest::blocking::get(&url)?.error_for_status()?.text()?;
    let feed = feed::parse(&body)?;
    Ok(feed)
}

fn publishable_blog_posts(posts: Vec<BlogPost>) -> Vec<BlogPost> {
    posts
        .into_iter()
        .filter(|p| !gitblog::path_is_ignored(&p.path, false))
        .collect()
}

fn hydrate_blog_posts_from_atom_feed(feed: &feed::Feed, blog_url: &str) {
    for entry in &feed.entries {
        let path = source_path_from_entry_url(blog_url, &entry.link.href);
        let post = BlogPost::from_source(
            path.clone(),
            entry.title.clone(),
            entry.summary.clone(),
            entry.updated,
            entry.effective_published(),
        );
        git::update_blog_post_from_atom(path, post);
    }
}

fn refresh_blog_posts_from_markdown(
    dest: &Path,
    blobs: &[git::FileBlob],
    full: bool,
    frontmatter_delimiter: &str,
) {
    if full || blobs.is_empty() {
        walk_markdown_files(dest, &mut |abs, rel| {
            let md = std::fs::read_to_string(abs).expect("read markdown");
            let ts = file_last_updated(abs);
            git::update_blog_post_from_markdown_path(
                rel.to_path_buf(),
                &md,
                ts,
                frontmatter_delimiter,
            );
        });
        return;
    }

    for blob in blobs {
        let markdown_path = dest.join(&blob.file_path);
        if !markdown_path.extension().is_some_and(|e| e == "md") {
            continue;
        }
        if markdown_is_ignored(&blob.file_path) {
            continue;
        }
        let md = std::fs::read_to_string(&markdown_path).expect("read markdown");
        git::update_blog_post_from_markdown(&blob.oid, &md, frontmatter_delimiter);
    }
}

fn walk_markdown_files(dir: &Path, f: &mut impl FnMut(&Path, &Path)) {
    fn walk(root: &Path, current: &Path, f: &mut impl FnMut(&Path, &Path)) {
        let Ok(entries) = std::fs::read_dir(current) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(root, &path, f);
            } else if path.extension().is_some_and(|e| e == "md") {
                let rel = path.strip_prefix(root).expect("strip prefix");
                if markdown_is_ignored(rel) {
                    continue;
                }
                f(&path, rel);
            }
        }
    }

    walk(dir, dir, f);
}

fn markdown_is_ignored(relative_path: &Path) -> bool {
    gitblog::path_is_ignored(relative_path, false)
}

fn source_path_from_entry_url(blog_url: &str, entry_url: &str) -> std::path::PathBuf {
    let base = blog_url.trim_end_matches('/');
    let rel = if let Some(suffix) = entry_url.strip_prefix(base) {
        suffix.trim_start_matches('/').to_string()
    } else if let Ok(url) = reqwest::Url::parse(entry_url) {
        url.path().trim_start_matches('/').to_string()
    } else {
        entry_url.trim_start_matches('/').to_string()
    };
    let mut path = std::path::PathBuf::from(rel);
    match path.extension().and_then(|e| e.to_str()) {
        Some("html") => {
            path.set_extension("md");
            path
        }
        Some(_) => path,
        None => {
            path.set_extension("md");
            path
        }
    }
}

fn file_last_updated(path: &Path) -> DateTime<FixedOffset> {
    std::fs::metadata(path)
        .ok()
        .and_then(|meta| meta.modified().ok())
        .map(DateTime::<Utc>::from)
        .unwrap_or_else(Utc::now)
        .fixed_offset()
}
