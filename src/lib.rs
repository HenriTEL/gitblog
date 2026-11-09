use std::path::Path;

use ignore::gitignore::GitignoreBuilder;

pub mod blog_post;
pub mod feed;
pub mod gemini;
pub mod git;
pub mod html;
pub mod markdown;
mod markdown_extensions;
pub mod repo_uri;
pub mod static_content;
pub mod templates;
pub mod user_profile;

/// Source paths that never reach the output.
pub const IGNORE_FILES: &[&str] = &[
    "**/*.draft.md",
    "**/draft.md",
    "**/draft/",
    "**/.github/",
    "LICENSE.md",
    "README.md",
];

/// Folders holding site assets: their files are kept, but Markdown inside them is not a post.
const NON_POST_DIRS: &[&str] = &["media", "templates"];

/// Whether a Markdown file at `relative_path` must not be treated as a blog post.
#[must_use]
pub fn path_is_not_a_post(relative_path: &Path) -> bool {
    path_is_ignored(relative_path, false)
        || relative_path
            .components()
            .any(|c| NON_POST_DIRS.iter().any(|dir| c.as_os_str() == *dir))
}

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
