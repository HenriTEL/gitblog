use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use clap::Parser;
use gitblog::blog_post::BlogPost;
use gitblog::repo_uri::{self, parse_repo_url};
use gitblog::user_profile::UserProfileMeta;
use gix::bstr::BStr;
use gix::url::Scheme;
use tempfile::tempdir_in;

use super::{
    BuildContext, CliArgs, MIN_UTC, UpdateProfileError, clear_output_dir,
    collect_updated_file_blobs, fallback_local_profile, github_owner_from_git_path,
    incremental_file_blobs, is_github_dot_com_host, list_output_dir_toplevel,
    normalize_repo_subdir, output_dir_is_nonempty, prepare_output_dir, profile_for_output,
    profile_or_fallback, render_markdown_files, resolve_fetch_remote_url,
};

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
        parents: std::iter::empty().collect(),
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
    let url = resolve_fetch_remote_url(&repo, Some("main"))
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
fn repo_defaults_to_the_current_directory() {
    let args = CliArgs::parse_from(["gitblog", "--blog-url", "https://example.com"]);
    assert_eq!(args.repo, "./");
    assert!(args.output.is_none());
}

#[test]
fn blog_url_stays_required() {
    assert!(
        CliArgs::try_parse_from(["gitblog", "./"]).is_err() || std::env::var("BASE_URL").is_ok()
    );
}

#[test]
fn source_options_parse() {
    let args = CliArgs::parse_from([
        "gitblog",
        "https://example.com/repo.git",
        "--blog-url",
        "https://example.com",
        "--repo-subdir",
        "site/blog",
        "--clone-dir",
        "/tmp/clone",
        "--no-social",
    ]);
    assert_eq!(args.repo_subdir.as_deref(), Some(Path::new("site/blog")));
    assert_eq!(args.clone_dir.as_deref(), Some(Path::new("/tmp/clone")));
    assert!(args.no_social);
}

#[test]
fn normalize_repo_subdir_accepts_relative_folders_only() {
    assert_eq!(
        normalize_repo_subdir(Path::new("./site/blog/")).unwrap(),
        Some(PathBuf::from("site/blog"))
    );
    assert_eq!(normalize_repo_subdir(Path::new(".")).unwrap(), None);
    assert!(normalize_repo_subdir(Path::new("../other")).is_err());
    assert!(normalize_repo_subdir(Path::new("/etc")).is_err());
}

#[test]
fn hosts_without_a_profile_fall_back_to_the_given_name() {
    let profile = profile_or_fallback(
        Err(UpdateProfileError::UnsupportedHost("gitlab.com".into())),
        || "Ada".to_string(),
    );
    assert_eq!(profile.username, "Ada");
    assert_eq!(
        profile
            .social_accounts
            .get("syndication")
            .map(String::as_str),
        Some("/atom.xml")
    );
    assert!(profile.avatar_url.is_none());

    let no_remote = profile_or_fallback(Err(UpdateProfileError::NoRemote), || "Ada".to_string());
    assert_eq!(no_remote.username, "Ada");
}

#[test]
fn profile_for_output_reports_avatar_and_drops_links_without_icons() {
    let dir = tempdir_in(test_git_dir()).expect("tempdir");
    let profile = UserProfileMeta {
        username: "u".into(),
        social_accounts: HashMap::from([
            ("github".to_string(), "https://github.com/u".to_string()),
            ("bluesky".to_string(), "https://bsky.app/u".to_string()),
        ]),
        ..UserProfileMeta::default()
    };

    let without_avatar = profile_for_output(profile.clone(), dir.path());
    assert!(without_avatar.avatar_url.is_none());
    assert!(without_avatar.social_accounts.contains_key("github"));
    assert!(!without_avatar.social_accounts.contains_key("bluesky"));

    std::fs::create_dir_all(dir.path().join("media")).expect("media");
    std::fs::write(dir.path().join("media/avatar"), "img").expect("avatar");
    let with_avatar = profile_for_output(profile, dir.path());
    assert_eq!(with_avatar.avatar_url.as_deref(), Some("/media/avatar"));
}

#[test]
fn template_or_stylesheet_changes_force_a_full_checkout() {
    let oid = gix::objs::compute_hash(gix::hash::Kind::Sha1, gix::objs::Kind::Blob, b"x");
    let head = gix::objs::Tree { entries: vec![] };
    for changed in ["templates/navbar.html.j2", "css/theme.css"] {
        let mut diff: HashMap<PathBuf, (gitblog::git::State, Option<gix::ObjectId>)> =
            HashMap::new();
        diff.insert(
            PathBuf::from("tech/post.md"),
            (gitblog::git::State::Modified, Some(oid)),
        );
        diff.insert(
            PathBuf::from(changed),
            (gitblog::git::State::Modified, Some(oid)),
        );
        assert!(incremental_file_blobs(&diff, &head).is_empty(), "{changed}");
    }
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

fn includes_uncommitted_changes(cli: &[&str]) -> bool {
    let args = CliArgs::parse_from(cli);
    let context = BuildContext {
        args: &args,
        up_to: MIN_UTC.fixed_offset(),
        previous_feed: None,
        output_dest: None,
        subdir: None,
    };
    context.includes_uncommitted_changes()
}

#[test]
fn only_a_local_path_without_no_fetch_includes_uncommitted_changes() {
    let base = ["gitblog", "--blog-url", "https://example.com"];
    assert!(includes_uncommitted_changes(&base));
    assert!(includes_uncommitted_changes(
        &[&base[..], &["./blog"]].concat()
    ));
    assert!(!includes_uncommitted_changes(
        &[&base[..], &["--no-fetch"]].concat()
    ));
    assert!(!includes_uncommitted_changes(
        &[
            &base[..],
            &["https://example.com/repo.git", "--clone-dir", "/tmp/clone"]
        ]
        .concat()
    ));
}

#[test]
fn branch_is_unset_unless_requested() {
    let args = CliArgs::parse_from(["gitblog", "--blog-url", "https://example.com"]);
    assert_eq!(args.branch, None);
    let args = CliArgs::parse_from([
        "gitblog",
        "--blog-url",
        "https://example.com",
        "--branch",
        "published",
    ]);
    assert_eq!(args.branch.as_deref(), Some("published"));
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
    let mut diff: HashMap<PathBuf, (gitblog::git::State, Option<gix::ObjectId>)> = HashMap::new();
    let keep_oid = gix::objs::compute_hash(gix::hash::Kind::Sha1, gix::objs::Kind::Blob, b"ok");
    let ignored_oid =
        gix::objs::compute_hash(gix::hash::Kind::Sha1, gix::objs::Kind::Blob, b"ignored");
    diff.insert(
        PathBuf::from("posts/hello.md"),
        (gitblog::git::State::Modified, Some(keep_oid)),
    );
    diff.insert(
        PathBuf::from("tech/wip.draft.md"),
        (gitblog::git::State::Created, Some(ignored_oid)),
    );

    let blobs = collect_updated_file_blobs(&diff);

    let [blob] = blobs.as_slice() else {
        panic!("expected exactly one blob");
    };
    assert_eq!(blob.file_path, PathBuf::from("posts/hello.md"));
    assert_eq!(blob.oid, keep_oid);
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
            ..UserProfileMeta::default()
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
            ..UserProfileMeta::default()
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
