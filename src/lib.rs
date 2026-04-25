use std::path::Path;

use ignore::gitignore::GitignoreBuilder;

pub mod blog_post;
pub mod feed;
pub mod gemini;
pub mod git;
pub mod html;
pub mod markdown;
pub mod push;
pub mod static_content;
pub(crate) mod templates;

pub const IGNORE_FILES: &[&str] = &["draft/", "LICENSE.md"];

pub fn path_is_ignored(relative_path: &Path, is_dir: bool) -> bool {
    let mut builder = GitignoreBuilder::new("/");
    for pattern in IGNORE_FILES {
        builder
            .add_line(None, pattern)
            .unwrap_or_else(|e| panic!("invalid ignore pattern `{pattern}`: {e}"));
    }
    let matcher = builder
        .build()
        .unwrap_or_else(|e| panic!("failed building ignore matcher: {e}"));
    matcher
        .matched_path_or_any_parents(relative_path, is_dir)
        .is_ignore()
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::path_is_ignored;

    #[test]
    fn path_is_ignored_matches_ignore_patterns() {
        assert!(path_is_ignored(Path::new("draft"), true));
        assert!(path_is_ignored(Path::new("draft/post.md"), false));
        assert!(path_is_ignored(Path::new("LICENSE.md"), false));
        assert!(!path_is_ignored(Path::new("posts/post.md"), false));
    }
}
