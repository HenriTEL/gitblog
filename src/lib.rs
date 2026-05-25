use std::path::Path;

use ignore::gitignore::GitignoreBuilder;

pub mod blog_post;
pub mod feed;
pub mod gemini;
pub mod git;
pub mod html;
pub mod markdown;
pub mod repo_uri;
pub mod static_content;
pub(crate) mod templates;
pub mod user_profile;

pub const IGNORE_FILES: &[&str] = &["**/*.draft.md", "**/draft.md", "LICENSE.md", "README.md"];

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
#[path = "lib_tests.rs"]
mod tests;
