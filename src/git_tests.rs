use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use chrono::{TimeZone, Utc};
use gix::objs::Kind;

use crate::blog_post;

use super::{GitLocal, State, apply_tree_diff_to_blog_posts, worktree_copy_is_skipped};

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

    let local = GitLocal {
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
