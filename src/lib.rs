pub mod blog_post;
pub mod feed;
pub mod gemini;
pub mod git;
pub mod html;
pub mod markdown;
pub mod static_content;
pub(crate) mod templates;

pub const IGNORED_FILES: &[&str] = &["draft/"];
