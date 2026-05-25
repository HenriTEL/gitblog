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
    CliArgs, clear_output_dir, collect_updated_file_blobs, fallback_local_profile,
    github_owner_from_git_path, is_github_dot_com_host, list_output_dir_toplevel,
    output_dir_is_nonempty, prepare_output_dir, render_markdown_files, resolve_fetch_remote_url,
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
