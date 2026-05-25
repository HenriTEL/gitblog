use std::path::Path;

use super::path_is_ignored;

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
