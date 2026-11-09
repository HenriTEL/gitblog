use std::path::Path;

use super::{path_is_ignored, path_is_not_a_post};

#[test]
fn draft_and_github_folders_are_ignored_at_any_depth() {
    assert!(path_is_ignored(Path::new("draft/wip.md"), false));
    assert!(path_is_ignored(Path::new("tech/draft/wip.md"), false));
    assert!(path_is_ignored(Path::new("draft"), true));
    assert!(path_is_ignored(
        Path::new(".github/workflows/ci.yml"),
        false
    ));
    assert!(!path_is_ignored(Path::new("drafting/post.md"), false));
}

#[test]
fn markdown_in_asset_folders_is_not_a_post_but_is_kept() {
    for path in ["media/notes.md", "templates/note.md", "tech/media/a.md"] {
        assert!(path_is_not_a_post(Path::new(path)), "{path}");
        assert!(!path_is_ignored(Path::new(path), false), "{path}");
    }
    assert!(path_is_not_a_post(Path::new("draft/wip.md")));
    assert!(!path_is_not_a_post(Path::new("tech/post.md")));
    assert!(!path_is_not_a_post(Path::new("tech/media-review.md")));
}

#[test]
fn path_is_ignored_matches_ignore_patterns() {
    assert!(path_is_ignored(Path::new("hello.draft.md"), false));
    assert!(path_is_ignored(Path::new("draft/post.draft.md"), false));
    assert!(path_is_ignored(Path::new("tech/draft.md"), false));
    assert!(path_is_ignored(Path::new("LICENSE.md"), false));

    assert!(!path_is_ignored(Path::new("posts/post.md"), false));
    assert!(!path_is_ignored(
        Path::new("posts/how-to-write-a-draft.md"),
        false
    ));
}
