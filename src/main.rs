use std::{
    error::Error,
    fmt,
    path::{Path, PathBuf},
};

use chrono::{DateTime, FixedOffset, Utc};
use gitblog::{
    blog_post::{BlogPost, fallback_title},
    feed,
    gemini::{markdown_file_to_gemtext, write_index_gemtext},
    git::{self, State},
    html::{markdown_file_to_html, write_index_from_blog_posts},
    static_content::write_static_content,
    user_profile::{
        GithubUserProfile, UserProfileDownloadError, UserProfileMeta,
        download as download_user_profile,
    },
};

use clap::Parser;
use gix::bstr::BStr;
use log::error;

const MIN_UTC: chrono::DateTime<chrono::Utc> = chrono::DateTime::<chrono::Utc>::MIN_UTC;

/// URI did not designate a github.com Git remote, parsing failed, or profile download failed.
#[derive(Debug)]
pub enum UpdateProfileError {
    GitUrl(gix::url::parse::Error),
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
) -> Result<UserProfileMeta, UpdateProfileError> {
    let url =
        gix::Url::from_bytes(BStr::new(repo_uri.as_bytes())).map_err(UpdateProfileError::GitUrl)?;

    match url.scheme {
        gix::url::Scheme::Http | gix::url::Scheme::Https | gix::url::Scheme::Ssh => {}
        s => {
            return Err(UpdateProfileError::UnsupportedRepositoryScheme(
                s.to_string(),
            ));
        }
    }

    let host = url.host().unwrap_or("");
    if !is_github_dot_com_host(host) {
        return Err(UpdateProfileError::UnsupportedHost(host.to_owned()));
    }

    let owner = github_owner_from_git_path(url.path.as_ref())?;
    download_user_profile(&GithubUserProfile::new(owner), dest)
        .map_err(UpdateProfileError::Download)
}

/// Downloads the repo owner’s avatar and bio unless `force_full` is false and `dest/media/avatar` already exists.
fn maybe_update_user_profile(force_full: bool, repo_uri: &str, dest: &Path) {
    let avatar_path = dest.join("media/avatar");
    if force_full || !avatar_path.is_file() {
        match update_profile(repo_uri, dest) {
            Ok(_) => {}
            Err(e) => log::warn!("could not update user profile: {e}"),
        }
    } else {
        log::debug!(
            "skipping user profile update; {} already exists and --full was not passed",
            avatar_path.display()
        );
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
    /// Frontmatter delimiter used in markdown files (for example: --- or +++)
    #[arg(long, default_value = "---")]
    frontmatter_delimiter: String,
}

fn main() {
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

    let url = gix::Url::from_bytes(BStr::new(args.repo.as_bytes())).expect("build git url");
    match url.scheme {
        gix::url::Scheme::Http | gix::url::Scheme::Https => {
            let remote = gitblog::git::GitRemote {
                url,
                branch: args.branch,
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
                .pull_files(&updated_file_blobs, None)
                .expect("fetch blobs");
            maybe_update_user_profile(args.full, &args.repo, dest.as_ref());
            refresh_blog_posts_from_markdown(
                &dest,
                &updated_file_blobs,
                args.full,
                &args.frontmatter_delimiter,
            );
            let posts = git::all_blog_posts();
            render_markdown_files(&dest, &posts, &args.frontmatter_delimiter, args.gemini);
            let generated_feed =
                feed::build_feed_from_blog_posts(&args.blog_url, &posts, previous_feed.as_ref());
            let xml = feed::generate(&generated_feed).expect("generate atom feed");
            std::fs::write(dest.join("atom.xml"), &xml).expect("write atom feed");
            println!("wrote atom.xml");
            write_index_from_blog_posts(&dest, &posts);
            println!("wrote index.html");
            if args.gemini {
                write_index_gemtext(&dest, &posts).expect("write gemini index");
                println!("wrote index.gmi");
            }
            println!("Using destination: {:?}", dest.to_str());
            if up_to == MIN_UTC.fixed_offset() {
                write_static_content(&dest);
            }
            println!("Blog built at {} ", dest.display());
        }
        _ => error!(
            "The URL {} resolved to protocol {} which is not supported.",
            url, url.scheme
        ), // TODO exit failure
    }
}

fn render_markdown_files(
    dest: &Path,
    posts: &[BlogPost],
    frontmatter_delimiter: &str,
    with_gemini: bool,
) {
    let post_titles = posts
        .iter()
        .map(|post| (post.path.clone(), post.title.clone()))
        .collect::<std::collections::HashMap<_, _>>();
    walk_markdown_files(dest, &mut |abs, rel| {
        markdown_file_to_html(abs, frontmatter_delimiter);
        println!("wrote {} ", rel.with_extension("html").to_string_lossy());
        if with_gemini {
            let title = post_titles
                .get(rel)
                .cloned()
                .unwrap_or_else(|| fallback_title(rel));
            markdown_file_to_gemtext(abs, &title).expect("write gemtext");
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
    use std::path::PathBuf;

    use gix::bstr::BStr;

    use super::{
        CliArgs, collect_updated_file_blobs, github_owner_from_git_path, is_github_dot_com_host,
        render_markdown_files,
    };
    use clap::Parser;
    use tempfile::tempdir;

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
        let dir = tempdir().expect("create temp dir");
        let markdown_path = dir.path().join("post.md");
        let mut source = std::fs::File::create(&markdown_path).expect("create markdown");
        writeln!(source, "# Post").expect("write markdown");

        render_markdown_files(dir.path(), &[], "---", false);

        assert!(!markdown_path.exists(), "markdown should be removed");
        assert!(
            dir.path().join("post.html").exists(),
            "html should be written"
        );
    }

    #[test]
    fn render_markdown_files_removes_markdown_after_html_and_gemini() {
        let dir = tempdir().expect("create temp dir");
        let markdown_path = dir.path().join("post.md");
        let mut source = std::fs::File::create(&markdown_path).expect("create markdown");
        writeln!(source, "# Post").expect("write markdown");

        render_markdown_files(dir.path(), &[], "---", true);

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

fn hydrate_blog_posts_from_atom_feed(feed: &feed::Feed, blog_url: &str) {
    for entry in &feed.entries {
        let path = source_path_from_entry_url(blog_url, &entry.link.href);
        let post = BlogPost::from_source(
            path.clone(),
            entry.title.clone(),
            entry.summary.clone(),
            entry.updated,
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
