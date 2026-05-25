use std::path::{Path, PathBuf};

use super::{is_draft_md, published_path_from_draft, section_from_path};

#[test]
fn published_path_from_draft_strips_suffix() {
    let draft = PathBuf::from("notes/post.draft.md");
    assert_eq!(
        published_path_from_draft(&draft),
        Some(PathBuf::from("notes/post.md"))
    );
}

#[test]
fn is_draft_md_matches_pattern() {
    assert!(is_draft_md(Path::new("a.draft.md")));
    assert!(!is_draft_md(Path::new("a.md")));
}

#[test]
fn section_from_path_uses_top_level_folder() {
    assert_eq!(
        section_from_path(Path::new("tech/hello.md")),
        Some("tech".to_string())
    );
    assert_eq!(
        section_from_path(Path::new("tech/sub/hello.md")),
        Some("tech".to_string())
    );
    assert_eq!(section_from_path(Path::new("hello.md")), None);
}
