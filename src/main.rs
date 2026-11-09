use std::{
    collections::{HashMap, HashSet},
    error::Error,
    fmt, fs,
    io::{self, BufRead, Write},
    path::{Component, Path, PathBuf},
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
    static_content::{site_has_icon, write_static_content},
    templates::install_blog_templates,
    user_profile::{
        GithubUserProfile, UserProfile, UserProfileDownloadError, UserProfileMeta, download_avatar,
    },
};

use clap::Parser;
use gix::bstr::{BStr, BString};
use gix::remote::Direction;
use gix::url::Scheme;
use tracing_core::{Event, Subscriber};
use tracing_subscriber::{
    Layer,
    filter::LevelFilter,
    layer::{Context, SubscriberExt},
    util::SubscriberInitExt,
};

const MIN_UTC: chrono::DateTime<chrono::Utc> = chrono::DateTime::<chrono::Utc>::MIN_UTC;
/// Branch used when reading straight from a remote, where there is no `HEAD` to follow.
const DEFAULT_REMOTE_BRANCH: &str = "main";

/// URI did not designate a github.com Git remote, parsing failed, or profile download failed.
#[derive(Debug)]
pub enum UpdateProfileError {
    GitUrl(gix::url::parse::Error),
    RepoUri(RepoUriError),
    RemoteLookup(String),
    UnsupportedRepositoryScheme(String),
    UnsupportedHost(String),
    NoRemote,
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
            UpdateProfileError::NoRemote => write!(f, "no git remote configured"),
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
            UpdateProfileError::RemoteLookup(_)
            | UpdateProfileError::UnsupportedRepositoryScheme(_)
            | UpdateProfileError::UnsupportedHost(_)
            | UpdateProfileError::NoRemote
            | UpdateProfileError::NonUtf8RepoPath
            | UpdateProfileError::MissingRepoOwner => None,
        }
    }
}

/// Fetch profile picture and metadata for the Git hosting implied by `repo_uri` (currently only
/// [GitHub](https://github.com)) and writes the avatar under `dest/media/avatar`.
fn update_profile(
    repo_uri: &str,
    dest: impl AsRef<Path>,
    force_full: bool,
) -> Result<UserProfileMeta, UpdateProfileError> {
    let url = gitblog::repo_uri::parse_repo_url(repo_uri).map_err(UpdateProfileError::RepoUri)?;

    match url.scheme {
        gix::url::Scheme::Http | gix::url::Scheme::Https | gix::url::Scheme::Ssh => {}
        s @ (gix::url::Scheme::File | gix::url::Scheme::Git | gix::url::Scheme::Ext(_)) => {
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
    let mut social_accounts = base_social_accounts();
    social_accounts.insert("github".to_string(), profile.profile_url());
    social_accounts.extend(
        profile
            .get_social_accounts()
            .map_err(|e| UpdateProfileError::Download(e.into()))?,
    );

    let avatar_path = dest.as_ref().join("media").join("avatar");
    if force_full || !avatar_path.is_file() {
        download_avatar(&profile, &avatar_path).map_err(UpdateProfileError::Download)?;
    } else {
        log::debug!(
            "skipping user profile update; {} already exists and --full was not passed",
            avatar_path.display()
        );
    }
    Ok(UserProfileMeta {
        username,
        bio,
        social_accounts,
        avatar_url: None,
    })
}

fn base_social_accounts() -> HashMap<String, String> {
    HashMap::from([("syndication".to_string(), "/atom.xml".to_string())])
}

/// Resolve the fetch remote URL from git config and update the profile when it points at GitHub.
fn update_profile_for_local_repo(
    repo: &gix::Repository,
    branch: Option<&str>,
    dest: impl AsRef<Path>,
    force_full: bool,
) -> Result<UserProfileMeta, UpdateProfileError> {
    match resolve_fetch_remote_url(repo, branch)? {
        Some(url) if matches!(url.scheme, Scheme::Http | Scheme::Https | Scheme::Ssh) => {
            update_profile(&url.to_bstring().to_string(), dest, force_full)
        }
        Some(url) => Err(UpdateProfileError::UnsupportedRepositoryScheme(
            url.scheme.to_string(),
        )),
        None => Err(UpdateProfileError::NoRemote),
    }
}

/// The hosting profile when there is one, else a profile named `fallback_name`.
fn profile_or_fallback(
    profile: Result<UserProfileMeta, UpdateProfileError>,
    fallback_name: impl FnOnce() -> String,
) -> UserProfileMeta {
    match profile {
        Ok(profile) => profile,
        Err(
            e @ (UpdateProfileError::UnsupportedHost(_)
            | UpdateProfileError::UnsupportedRepositoryScheme(_)
            | UpdateProfileError::NoRemote),
        ) => {
            log::info!("{e}; using fallback profile (no avatar or hosting links)");
            UserProfileMeta {
                username: fallback_name(),
                social_accounts: base_social_accounts(),
                ..UserProfileMeta::default()
            }
        }
        Err(e) => panic!("update profile: {e}"),
    }
}

fn resolve_fetch_remote_url(
    repo: &gix::Repository,
    branch: Option<&str>,
) -> Result<Option<gix::Url>, UpdateProfileError> {
    let branch = branch.map(str::to_owned).or_else(|| {
        let head = repo.head_name().ok().flatten()?;
        Some(head.shorten().to_string())
    });
    let mut candidates: Vec<BString> = Vec::new();
    if let Some(name) = branch
        .as_deref()
        .and_then(|b| repo.branch_remote_name(BStr::new(b.as_bytes()), Direction::Fetch))
    {
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
        .or_else(|| repo.path().parent().map(std::path::Path::to_path_buf))
        .and_then(|p| p.file_name().map(std::ffi::OsStr::to_os_string))
        .and_then(|n| n.into_string().ok())
        .unwrap_or_else(|| "blog".to_string());
    UserProfileMeta {
        username,
        ..UserProfileMeta::default()
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

/// gix hands some events to the subscriber without asking whether they are enabled, which
/// bypasses the log filter, so its most verbose level is enforced on every event.
struct MaxEventLevel(LevelFilter);

impl<S: Subscriber> Layer<S> for MaxEventLevel {
    fn event_enabled(&self, event: &Event<'_>, _: Context<'_, S>) -> bool {
        *event.metadata().level() <= self.0
    }
}

#[derive(Parser)]
#[command(version, about, long_about = None)]
struct CliArgs {
    /// Path or URL of the git repository containing the blog sources. A local path also publishes
    /// its uncommitted changes, and never talks to the git remote
    #[arg(env = "SOURCE_REPO", default_value = "./")]
    repo: String,
    /// Branch on the git repository to use [default: the repository's HEAD when it is on disk,
    /// `main` when reading from a remote without --clone-dir]
    #[arg(long)]
    branch: Option<String>,
    /// URL where your blog is hosted
    #[arg(long, env = "BASE_URL")]
    blog_url: String,
    /// Folder of the repository containing the blog, when it is not the repository root
    #[arg(long)]
    repo_subdir: Option<PathBuf>,
    /// Keep a clone of the remote repository here and reuse it between builds
    #[arg(long)]
    clone_dir: Option<PathBuf>,
    /// Do not look up a hosting profile: no avatar and no social links
    #[arg(long)]
    no_social: bool,

    /// Generate all blog files, not just the ones that changed
    #[arg(long)]
    full: bool,
    /// Generate Gemini (`.gmi`) output alongside HTML
    #[arg(long)]
    gemini: bool,
    /// Build from the last commit already on disk: do not contact the git remote and ignore
    /// uncommitted changes. A remote repository needs an existing --clone-dir
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

    let max_level = env_filter.max_level_hint().unwrap_or(LevelFilter::TRACE);
    tracing_subscriber::fmt()
        .with_env_filter(env_filter)
        .with_target(true)
        .finish()
        .with(MaxEventLevel(max_level))
        .init();

    let args = CliArgs::parse();

    let url = match repo_uri::parse_repo_url(&args.repo) {
        Ok(url) => url,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };

    if let Err(message) = validate_args(&args, &url) {
        eprintln!("{message}");
        return ExitCode::FAILURE;
    }

    let subdir = match args.repo_subdir.as_deref().map(normalize_repo_subdir) {
        None => None,
        Some(Ok(subdir)) => subdir,
        Some(Err(e)) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };

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
    } else if let Ok(feed) = atom_feed {
        let updated = feed.updated;
        (updated, Some(feed))
    } else {
        log::error!("Failed to fetch atom feed from {}", args.blog_url);
        log::info!("Falling back to full blog generation");
        (MIN_UTC.fixed_offset(), None)
    };

    let context = BuildContext {
        args: &args,
        up_to,
        previous_feed: previous_feed.as_ref(),
        output_dest,
        subdir,
    };

    match url.scheme {
        Scheme::Http | Scheme::Https | Scheme::Ssh => {
            if let Some(clone_dir) = args.clone_dir.as_ref() {
                if args.no_fetch {
                    if let Err(e) = gix::open(clone_dir) {
                        eprintln!("no usable clone at {}: {e}", clone_dir.display());
                        return ExitCode::FAILURE;
                    }
                } else if let Err(e) = git::sync_clone(&url, clone_dir) {
                    eprintln!("failed to update clone at {}: {e}", clone_dir.display());
                    return ExitCode::FAILURE;
                }
                build_from_local_repo(&context, clone_dir)
            } else {
                build_from_remote(&context, url);
                ExitCode::SUCCESS
            }
        }
        Scheme::File => match repo_uri::file_url_to_path(&url) {
            Ok(repo_root) => build_from_local_repo(&context, &repo_root),
            Err(e) => {
                eprintln!("{e}");
                ExitCode::FAILURE
            }
        },
        ref scheme @ (Scheme::Git | Scheme::Ext(_)) => {
            eprintln!("The URL {url} resolved to protocol {scheme} which is not supported.");
            ExitCode::FAILURE
        }
    }
}

/// Rejects option combinations that cannot work with the repository at `url`.
fn validate_args(args: &CliArgs, url: &gix::Url) -> Result<(), &'static str> {
    let is_local = url.scheme == Scheme::File;
    if args.no_fetch && !is_local && args.clone_dir.is_none() {
        return Err("--no-fetch with a remote repository requires --clone-dir");
    }
    if args.overwrite && args.output.is_none() {
        return Err("--overwrite requires --output");
    }
    if args.clone_dir.is_some() && is_local {
        return Err("--clone-dir requires a remote repository");
    }
    Ok(())
}

struct BuildContext<'a> {
    args: &'a CliArgs,
    up_to: DateTime<FixedOffset>,
    previous_feed: Option<&'a feed::Feed>,
    output_dest: Option<PathBuf>,
    subdir: Option<PathBuf>,
}

impl BuildContext<'_> {
    fn is_full(&self) -> bool {
        self.up_to == MIN_UTC.fixed_offset()
    }

    /// A local path stands for the user's working copy; a clone is only a cache of the remote.
    fn includes_uncommitted_changes(&self) -> bool {
        !self.args.no_fetch && self.args.clone_dir.is_none()
    }

    fn hydrate_from_previous_feed(&self) {
        if let Some(feed) = self.previous_feed {
            hydrate_blog_posts_from_atom_feed(feed, &self.args.blog_url);
        }
    }
}

fn build_from_remote(context: &BuildContext<'_>, url: gix::Url) {
    let args = context.args;
    let remote = git::GitRemote {
        url,
        branch: args
            .branch
            .clone()
            .unwrap_or_else(|| DEFAULT_REMOTE_BRANCH.to_string()),
        subdir: context.subdir.clone(),
    };
    context.hydrate_from_previous_feed();
    let tree_ends = remote.fetch_since((!context.is_full()).then_some(&context.up_to));
    let updated_file_blobs = if context.is_full() {
        vec![]
    } else {
        let diff = remote.tree_diff(&tree_ends.up_to_tree, &tree_ends.head_tree);
        incremental_file_blobs(&diff, &tree_ends.head_tree)
    };
    let dest = remote
        .pull_files(&updated_file_blobs, context.output_dest.clone())
        .expect("fetch blobs");
    let fallback_name = || {
        tree_ends
            .head_author
            .clone()
            .unwrap_or_else(|| "blog".to_string())
    };
    let user_profile = if args.no_social {
        profile_without_hosting(fallback_name())
    } else {
        profile_or_fallback(update_profile(&args.repo, &dest, args.full), fallback_name)
    };
    finish_build(context, &dest, user_profile, &updated_file_blobs);
}

fn build_from_local_repo(context: &BuildContext<'_>, repo_root: &Path) -> ExitCode {
    let args = context.args;
    let repo = match gix::discover(repo_root) {
        Ok(repo) => repo,
        Err(e) => {
            eprintln!("failed to open repository at {}: {e}", repo_root.display());
            return ExitCode::FAILURE;
        }
    };
    context.hydrate_from_previous_feed();
    let local = GitLocal {
        repo_root: repo_root.to_path_buf(),
        branch: args.branch.clone(),
        subdir: context.subdir.clone(),
    };

    let uncommitted = if context.includes_uncommitted_changes() {
        local.uncommitted_changes().expect("read git status")
    } else {
        git::UncommittedChanges::default()
    };

    let updated_file_blobs = if context.is_full() {
        vec![]
    } else {
        let tree_ends = local.fetch_since(&context.up_to);
        let diff = local.tree_diff(&tree_ends.up_to_tree, &tree_ends.head_tree);
        if uncommitted.affect_every_page() {
            vec![]
        } else {
            incremental_file_blobs(&diff, &tree_ends.head_tree)
        }
    };
    let dest = local
        .pull_files(&updated_file_blobs, context.output_dest.clone())
        .expect("materialize from object database");

    if context.is_full() {
        local.index_publication_dates();
    }

    if !uncommitted.files.is_empty() {
        println!("Including {} uncommitted files", uncommitted.files.len());
    }
    uncommitted
        .apply(
            &dest,
            Utc::now().fixed_offset(),
            &args.frontmatter_delimiter,
        )
        .expect("apply uncommitted changes");

    let fallback_name = || {
        local
            .head_author()
            .unwrap_or_else(|| fallback_local_profile(&repo).username)
    };
    let user_profile = if args.no_social {
        profile_without_hosting(fallback_name())
    } else {
        profile_or_fallback(
            update_profile_for_local_repo(&repo, args.branch.as_deref(), &dest, args.full),
            fallback_name,
        )
    };
    finish_build(context, &dest, user_profile, &updated_file_blobs);
    ExitCode::SUCCESS
}

fn profile_without_hosting(username: String) -> UserProfileMeta {
    UserProfileMeta {
        username,
        ..UserProfileMeta::default()
    }
}

/// `path` as a relative path made of plain folder names; `None` when it designates the root.
fn normalize_repo_subdir(path: &Path) -> Result<Option<PathBuf>, String> {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => normalized.push(part),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(format!(
                    "--repo-subdir must be a relative path inside the repository, got {}",
                    path.display()
                ));
            }
        }
    }
    Ok((!normalized.as_os_str().is_empty()).then_some(normalized))
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
    context: &BuildContext<'_>,
    dest: &Path,
    user_profile: UserProfileMeta,
    updated_file_blobs: &[git::FileBlob],
) {
    let args = context.args;
    if let Err(e) = install_blog_templates(dest) {
        panic!("invalid file in templates/: {}", error_chain(&e));
    }
    let user_profile = profile_for_output(user_profile, dest);
    refresh_blog_posts_from_markdown(
        dest,
        updated_file_blobs,
        args.full,
        &args.frontmatter_delimiter,
    );
    prune_blog_posts_missing_from_worktree(dest);
    let posts = publishable_blog_posts(git::all_blog_posts(), dest);
    render_markdown_files(
        &user_profile,
        dest,
        &posts,
        &args.frontmatter_delimiter,
        args.gemini,
    );
    let generated_feed = feed::build_feed_from_blog_posts(
        &args.blog_url,
        &posts,
        context.previous_feed,
        &user_profile.username,
    );
    let xml = feed::generate(&generated_feed).expect("generate atom feed");
    std::fs::write(dest.join("atom.xml"), &xml).expect("write atom feed");
    println!("wrote atom.xml");
    write_index_from_blog_posts(dest, &user_profile, &posts);
    println!("wrote index.html");
    if args.gemini {
        write_index_gemtext(dest, &posts).expect("write gemini index");
        println!("wrote index.gmi");
    }
    if context.is_full() || updated_file_blobs.is_empty() {
        write_static_content(dest).expect("write static content");
    }
    println!("Blog built at {} ", dest.display());
}

fn error_chain(error: &dyn Error) -> String {
    let mut message = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        message.push_str(&format!(": {cause}"));
        source = cause.source();
    }
    message
}

/// Fills in what depends on the generated site: the avatar, and the links that have an icon.
fn profile_for_output(mut profile: UserProfileMeta, dest: &Path) -> UserProfileMeta {
    profile.avatar_url = dest
        .join("media")
        .join("avatar")
        .is_file()
        .then(|| "/media/avatar".to_string());
    profile
        .social_accounts
        .retain(|icon, _| site_has_icon(dest, icon));
    profile
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

/// Files to pull for an incremental build. An empty list makes the pull step check out the whole
/// tree, which is what a change to the templates or stylesheets needs.
fn incremental_file_blobs(
    diff: &HashMap<PathBuf, (State, Option<gix::ObjectId>)>,
    head_tree: &gix::objs::Tree,
) -> Vec<git::FileBlob> {
    if diff.keys().any(|path| git::changes_every_page(path)) {
        return Vec::new();
    }
    let mut blobs = collect_updated_file_blobs(diff);
    if blobs.is_empty() {
        return blobs;
    }
    let pulled: HashSet<PathBuf> = blobs.iter().map(|b| b.file_path.clone()).collect();
    blobs.extend(
        git::blobs_needed_for_rendering(head_tree)
            .into_iter()
            .filter(|blob| !pulled.contains(&blob.file_path)),
    );
    blobs
}

fn collect_updated_file_blobs(
    diff: &HashMap<PathBuf, (State, Option<gix::ObjectId>)>,
) -> Vec<gitblog::git::FileBlob> {
    diff.iter()
        .filter_map(|(path, (state, maybe_oid))| {
            if gitblog::path_is_ignored(path, false) {
                return None;
            }
            match (state, maybe_oid) {
                (State::Created | State::Modified, Some(oid)) => Some(gitblog::git::FileBlob {
                    file_path: path.clone(),
                    oid: oid.to_owned(),
                }),
                (State::Deleted, _) | (_, None) => None,
            }
        })
        .collect::<Vec<_>>()
}

#[cfg(test)]
#[path = "main_tests.rs"]
mod tests;
fn fetch_atom_feed(blog_url: &str) -> Result<feed::Feed, Box<dyn Error>> {
    let url = format!("{}/atom.xml", blog_url.trim_end_matches('/'));
    let body = reqwest::blocking::get(&url)?.error_for_status()?.text()?;
    let feed = feed::parse(&body)?;
    Ok(feed)
}

fn publishable_blog_posts(posts: Vec<BlogPost>, dest: &Path) -> Vec<BlogPost> {
    posts
        .into_iter()
        .filter(|p| !gitblog::path_is_not_a_post(&p.path))
        .filter(|p| p.path.extension().is_some_and(|e| e == "md"))
        .filter(|p| dest.join(&p.path).is_file())
        .collect()
}

/// Drop store entries whose markdown source is not present in the worktree copy.
fn prune_blog_posts_missing_from_worktree(dest: &Path) {
    for post in git::all_blog_posts() {
        if dest.join(&post.path).is_file() {
            continue;
        }
        gitblog::blog_post::remove_by_path(&post.path);
    }
}

fn hydrate_blog_posts_from_atom_feed(feed: &feed::Feed, blog_url: &str) {
    for entry in &feed.entries {
        let path = source_path_from_entry_url(blog_url, &entry.link.href);
        let mut post = BlogPost::from_source(
            path.clone(),
            entry.title.clone(),
            entry.summary.clone(),
            entry.updated,
            entry.effective_published(),
        );
        // Feeds without `<published>` must not hide the date found in the git history.
        post.publication_date = entry.published;
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
            let ts = gitblog::blog_post::get_by_path(rel)
                .map_or_else(|| file_last_updated(abs), |p| p.last_updated);
            git::update_blog_post_from_markdown_path(rel, &md, ts, frontmatter_delimiter);
        });
        return;
    }

    for blob in blobs {
        if !gitblog::is_post_source(&blob.file_path) {
            continue;
        }
        let markdown_path = dest.join(&blob.file_path);
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
    gitblog::path_is_not_a_post(relative_path)
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
        Some("html") | None => {
            path.set_extension("md");
            path
        }
        Some(_) => path,
    }
}

fn file_last_updated(path: &Path) -> DateTime<FixedOffset> {
    std::fs::metadata(path)
        .ok()
        .and_then(|meta| meta.modified().ok())
        .map_or_else(Utc::now, DateTime::<Utc>::from)
        .fixed_offset()
}
