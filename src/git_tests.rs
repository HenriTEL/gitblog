use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use chrono::{TimeZone, Utc};
use gix::objs::Kind;

use crate::blog_post;

use super::{GitLocal, State, apply_tree_diff_to_blog_posts};

fn git(repo: &Path, args: &[&str], date: &str) {
    let status = Command::new("git")
        .args(args)
        .current_dir(repo)
        .env("GIT_AUTHOR_DATE", date)
        .env("GIT_COMMITTER_DATE", date)
        .env("GIT_AUTHOR_NAME", "Ada Author")
        .env("GIT_AUTHOR_EMAIL", "ada@example.com")
        .env("GIT_COMMITTER_NAME", "Ada Author")
        .env("GIT_COMMITTER_EMAIL", "ada@example.com")
        .status()
        .expect("run git");
    assert!(status.success(), "git {args:?}");
}

fn write_file(repo: &Path, path: &str, content: &str) {
    let full = repo.join(path);
    std::fs::create_dir_all(full.parent().unwrap()).unwrap();
    std::fs::write(full, content).unwrap();
}

fn commit_all(repo: &Path, date: &str) {
    git(repo, &["add", "-A"], date);
    git(repo, &["commit", "-qm", "change"], date);
}

fn new_repo(dir: &Path) {
    git(
        dir,
        &["init", "-q", "-b", "main"],
        "2020-01-01T00:00:00+00:00",
    );
}

fn local(repo: &Path, subdir: Option<&str>) -> GitLocal {
    GitLocal {
        repo_root: repo.to_path_buf(),
        branch: Some("main".to_string()),
        subdir: subdir.map(PathBuf::from),
    }
}

fn utc(date: &str) -> chrono::DateTime<chrono::FixedOffset> {
    chrono::DateTime::parse_from_rfc3339(date).unwrap()
}

#[test]
fn history_sets_created_and_updated_dates_for_posts_in_folders() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path();
    new_repo(repo);
    write_file(repo, "dated/a.md", "# A\n");
    commit_all(repo, "2021-01-01T10:00:00+00:00");
    write_file(repo, "dated/b.md", "# B\n");
    commit_all(repo, "2021-02-01T10:00:00+00:00");
    write_file(repo, "dated/a.md", "# A changed\n");
    commit_all(repo, "2021-03-01T10:00:00+00:00");

    local(repo, None).index_publication_dates();

    let a = blog_post::get_by_path(Path::new("dated/a.md")).expect("a");
    assert_eq!(a.publication_date, Some(utc("2021-01-01T10:00:00+00:00")));
    assert_eq!(a.last_updated, utc("2021-03-01T10:00:00+00:00"));
    let b = blog_post::get_by_path(Path::new("dated/b.md")).expect("b");
    assert_eq!(b.publication_date, Some(utc("2021-02-01T10:00:00+00:00")));
    assert_eq!(b.last_updated, utc("2021-02-01T10:00:00+00:00"));
}

#[test]
fn history_follows_the_first_parent_across_merges() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path();
    new_repo(repo);
    write_file(repo, "merged/kept.md", "# Kept\n");
    commit_all(repo, "2022-01-01T10:00:00+00:00");
    git(
        repo,
        &["checkout", "-qb", "side"],
        "2022-01-02T10:00:00+00:00",
    );
    write_file(repo, "merged/side.md", "# Side\n");
    commit_all(repo, "2022-01-03T10:00:00+00:00");
    git(
        repo,
        &["checkout", "-q", "main"],
        "2022-01-04T10:00:00+00:00",
    );
    write_file(repo, "merged/other.md", "# Other\n");
    commit_all(repo, "2022-01-05T10:00:00+00:00");
    git(
        repo,
        &["merge", "-q", "--no-ff", "-m", "merge", "side"],
        "2022-01-06T10:00:00+00:00",
    );

    local(repo, None).index_publication_dates();

    let kept = blog_post::get_by_path(Path::new("merged/kept.md")).expect("kept");
    assert_eq!(
        kept.publication_date,
        Some(utc("2022-01-01T10:00:00+00:00"))
    );
    assert_eq!(kept.last_updated, utc("2022-01-01T10:00:00+00:00"));
    let side = blog_post::get_by_path(Path::new("merged/side.md")).expect("side");
    assert_eq!(
        side.publication_date,
        Some(utc("2022-01-06T10:00:00+00:00"))
    );
}

#[test]
fn deleted_posts_do_not_stay_in_the_store() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path();
    new_repo(repo);
    write_file(repo, "gone/old.md", "# Old\n");
    write_file(repo, "gone/keep.md", "# Keep\n");
    commit_all(repo, "2023-01-01T10:00:00+00:00");
    std::fs::remove_file(repo.join("gone/old.md")).unwrap();
    commit_all(repo, "2023-02-01T10:00:00+00:00");

    local(repo, None).index_publication_dates();

    assert!(blog_post::get_by_path(Path::new("gone/old.md")).is_none());
    assert!(blog_post::get_by_path(Path::new("gone/keep.md")).is_some());
}

#[test]
fn incremental_range_leaves_unchanged_posts_dates_alone() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path();
    new_repo(repo);
    write_file(repo, "ranged/old.md", "# Old\n");
    commit_all(repo, "2024-01-01T10:00:00+00:00");
    write_file(repo, "ranged/new.md", "# New\n");
    commit_all(repo, "2024-03-01T10:00:00+00:00");

    let tree_ends = local(repo, None).fetch_since(&utc("2024-01-01T10:00:00+00:00"));

    assert!(blog_post::get_by_path(Path::new("ranged/old.md")).is_none());
    let new = blog_post::get_by_path(Path::new("ranged/new.md")).expect("new");
    assert_eq!(new.publication_date, Some(utc("2024-03-01T10:00:00+00:00")));
    let diff = super::tree_diff(&tree_ends.up_to_tree, &tree_ends.head_tree);
    assert_eq!(diff.len(), 1, "{diff:?}");
    assert!(diff.contains_key(Path::new("ranged/new.md")));
    assert_eq!(tree_ends.head_author.as_deref(), Some("Ada Author"));
}

#[test]
fn repo_subdir_scopes_history_and_checkout() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path();
    new_repo(repo);
    write_file(repo, "site/blog/tech/in.md", "# In\n");
    write_file(repo, "elsewhere/out.md", "# Out\n");
    commit_all(repo, "2025-01-01T10:00:00+00:00");

    let scoped = local(repo, Some("site/blog"));
    let dest = tempfile::tempdir().unwrap();
    scoped
        .pull_files(&[], Some(dest.path().to_path_buf()))
        .expect("pull");
    scoped.index_publication_dates();

    assert!(dest.path().join("tech/in.md").is_file());
    assert!(!dest.path().join("elsewhere").exists());
    assert!(blog_post::get_by_path(Path::new("tech/in.md")).is_some());
    assert!(blog_post::get_by_path(Path::new("elsewhere/out.md")).is_none());
}

#[test]
fn missing_repo_subdir_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path();
    new_repo(repo);
    write_file(repo, "a.md", "# A\n");
    commit_all(repo, "2025-01-01T10:00:00+00:00");

    let dest = tempfile::tempdir().unwrap();
    let result = local(repo, Some("nope")).pull_files(&[], Some(dest.path().to_path_buf()));
    assert!(result.is_err_and(|error| error.to_string().contains("`nope`")));
}

#[test]
fn head_author_reads_the_branch_tip() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path();
    new_repo(repo);
    write_file(repo, "a.md", "# A\n");
    commit_all(repo, "2025-01-01T10:00:00+00:00");

    assert_eq!(
        local(repo, None).head_author().as_deref(),
        Some("Ada Author")
    );
}

#[test]
fn asset_folder_markdown_never_becomes_a_post() {
    let path = PathBuf::from("media/gitblog-test-notes.md");
    let oid = gix::objs::compute_hash(gix::hash::Kind::Sha1, Kind::Blob, b"# Notes\n");
    let mut diff = HashMap::new();
    diff.insert(path.clone(), (State::Created, Some(oid)));

    apply_tree_diff_to_blog_posts(&diff, utc("2024-06-01T10:00:00+00:00"));

    assert!(blog_post::get_by_path(&path).is_none());
}

fn local_on_head(repo: &Path, subdir: Option<&str>) -> GitLocal {
    GitLocal {
        branch: None,
        ..local(repo, subdir)
    }
}

fn uncommitted_files(local: &GitLocal) -> Vec<(String, State)> {
    local
        .uncommitted_changes()
        .expect("git status")
        .files
        .into_iter()
        .map(|(path, state)| (path.to_string_lossy().into_owned(), state))
        .collect()
}

/// A repository with one commit, then every kind of uncommitted change.
fn dirty_repo(repo: &Path) {
    new_repo(repo);
    write_file(repo, ".gitignore", "ignored.md\n");
    write_file(repo, "kept.md", "# Kept\n");
    write_file(repo, "edited.md", "# Edited\n");
    write_file(repo, "gone.md", "# Gone\n");
    write_file(repo, "notes/tracked.txt", "committed\n");
    commit_all(repo, "2021-01-01T10:00:00+00:00");

    write_file(repo, "edited.md", "# Edited again\n");
    std::fs::remove_file(repo.join("gone.md")).unwrap();
    write_file(repo, "notes/tracked.txt", "edited\n");
    write_file(repo, "new.md", "# New\n");
    write_file(repo, "section/staged.md", "# Staged\n");
    git(
        repo,
        &["add", "section/staged.md"],
        "2021-01-02T10:00:00+00:00",
    );
    write_file(repo, "templates/navbar.html.j2", "<nav>new</nav>");
    write_file(repo, "css/theme.css", "body{}");
    write_file(repo, "media/pictures/new.png", "png");

    write_file(repo, "notes.txt", "not part of the blog");
    write_file(repo, "css/other.css", "body{}");
    write_file(repo, "templates/nested/x.html.j2", "unused");
    write_file(repo, "new.draft.md", "# Draft\n");
    write_file(repo, "README.md", "# Readme\n");
    write_file(repo, "media/notes.md", "# Not a post\n");
    write_file(repo, "ignored.md", "# Ignored\n");
}

#[test]
fn uncommitted_changes_keep_edits_deletions_and_new_files_in_expected_places() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path();
    dirty_repo(repo);

    let expected = [
        ("css/theme.css", State::Created),
        ("edited.md", State::Modified),
        ("gone.md", State::Deleted),
        ("media/notes.md", State::Created),
        ("media/pictures/new.png", State::Created),
        ("new.md", State::Created),
        ("notes/tracked.txt", State::Modified),
        ("section/staged.md", State::Created),
        ("templates/navbar.html.j2", State::Created),
    ]
    .map(|(path, state)| (path.to_string(), state));
    assert_eq!(uncommitted_files(&local_on_head(repo, None)), expected);
}

#[test]
fn uncommitted_changes_are_relative_to_the_repo_subdir() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path();
    new_repo(repo);
    write_file(repo, "blog/post.md", "# Post\n");
    write_file(repo, "other/post.md", "# Other\n");
    commit_all(repo, "2021-01-01T10:00:00+00:00");

    write_file(repo, "blog/post.md", "# Edited\n");
    write_file(repo, "blog/new.md", "# New\n");
    write_file(repo, "other/post.md", "# Edited\n");

    assert_eq!(
        uncommitted_files(&local_on_head(repo, Some("blog"))),
        [
            ("new.md".to_string(), State::Created),
            ("post.md".to_string(), State::Modified),
        ]
    );
}

#[test]
fn uncommitted_changes_only_apply_to_the_checked_out_branch() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path();
    new_repo(repo);
    write_file(repo, "post.md", "# Post\n");
    commit_all(repo, "2021-01-01T10:00:00+00:00");
    git(repo, &["branch", "other"], "2021-01-01T10:00:00+00:00");
    write_file(repo, "post.md", "# Edited\n");

    let edited = [("post.md".to_string(), State::Modified)];
    assert_eq!(uncommitted_files(&local(repo, None)), edited);
    let other_branch = GitLocal {
        branch: Some("other".to_string()),
        ..local(repo, None)
    };
    assert!(uncommitted_files(&other_branch).is_empty());
}

#[test]
fn only_templates_and_stylesheets_affect_every_page() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path();
    new_repo(repo);
    write_file(repo, "post.md", "# Post\n");
    commit_all(repo, "2021-01-01T10:00:00+00:00");
    let local = local_on_head(repo, None);

    write_file(repo, "post.md", "# Edited\n");
    assert!(!local.uncommitted_changes().unwrap().affect_every_page());

    write_file(repo, "templates/footer.html.j2", "<footer></footer>");
    assert!(local.uncommitted_changes().unwrap().affect_every_page());
}

#[test]
fn applying_uncommitted_changes_updates_files_and_dates_articles_now() {
    let dir = tempfile::tempdir().unwrap();
    let dest = tempfile::tempdir().unwrap();
    let repo = dir.path();
    dirty_repo(repo);
    let now = utc("2030-05-05T12:00:00+00:00");
    let committed = utc("2021-01-01T10:00:00+00:00");

    let local = local_on_head(repo, None);
    let uncommitted = local.uncommitted_changes().unwrap();
    local.pull_files(&[], Some(dest.path().into())).unwrap();
    local.index_publication_dates();
    uncommitted.apply(dest.path(), now, "---").unwrap();

    let read = |path: &str| std::fs::read_to_string(dest.path().join(path)).unwrap();
    assert_eq!(read("edited.md"), "# Edited again\n");
    assert_eq!(read("new.md"), "# New\n");
    assert_eq!(read("templates/navbar.html.j2"), "<nav>new</nav>");
    assert!(!dest.path().join("gone.md").exists());
    assert!(!dest.path().join("notes.txt").exists());

    let kept = blog_post::get_by_path(Path::new("kept.md")).expect("kept");
    assert_eq!(kept.last_updated, committed);
    let edited = blog_post::get_by_path(Path::new("edited.md")).expect("edited");
    assert_eq!(edited.last_updated, now);
    assert_eq!(edited.publication_date, Some(committed));
    let new_post = blog_post::get_by_path(Path::new("new.md")).expect("new");
    assert_eq!(new_post.effective_publication_date(), now);
    let staged = blog_post::get_by_path(Path::new("section/staged.md")).expect("staged");
    assert_eq!(staged.last_updated, now);
}

#[test]
fn committed_checkout_ignores_uncommitted_changes() {
    let dir = tempfile::tempdir().unwrap();
    let dest = tempfile::tempdir().unwrap();
    let repo = dir.path();
    dirty_repo(repo);

    local_on_head(repo, None)
        .pull_files(&[], Some(dest.path().into()))
        .unwrap();

    let read = |path: &str| std::fs::read_to_string(dest.path().join(path)).unwrap();
    assert_eq!(read("edited.md"), "# Edited\n");
    assert_eq!(read("gone.md"), "# Gone\n");
    assert!(!dest.path().join("new.md").exists());
}

#[test]
fn head_is_built_when_no_branch_is_requested() {
    let dir = tempfile::tempdir().unwrap();
    let dest = tempfile::tempdir().unwrap();
    let repo = dir.path();
    new_repo(repo);
    write_file(repo, "post.md", "# On main\n");
    commit_all(repo, "2021-01-01T10:00:00+00:00");
    git(
        repo,
        &["checkout", "-q", "-b", "feature"],
        "2021-01-01T10:00:00+00:00",
    );
    write_file(repo, "post.md", "# On feature\n");
    commit_all(repo, "2021-02-01T10:00:00+00:00");

    local_on_head(repo, None)
        .pull_files(&[], Some(dest.path().into()))
        .unwrap();

    assert_eq!(
        std::fs::read_to_string(dest.path().join("post.md")).unwrap(),
        "# On feature\n"
    );
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

    let local = GitLocal {
        repo_root: repo.to_path_buf(),
        branch: Some("main".to_string()),
        subdir: None,
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
