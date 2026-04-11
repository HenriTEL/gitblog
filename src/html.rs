use std::collections::HashMap;
use std::path::Path;

use chrono::{DateTime, Local, Utc};
use comrak::{markdown_to_html, Options};
use serde::Serialize;
use tera::{Context, Tera};

use crate::templates;

/// Metadata for a single post, shared by article and index templates.
#[derive(Debug, Clone, Serialize)]
pub struct BlogPost {
    pub title: String,
    pub author: String,
    pub description: Option<String>,
    pub creation_dt: DateTime<Utc>,
    pub last_update_dt: DateTime<Utc>,
    pub human_time: String,
    /// Site-relative URL path (e.g. `notes/foo.html`).
    pub relative_path: String,
}

#[derive(Serialize)]
struct ArticlePageContext {
    blog_post: BlogPost,
    main_content: String,
    sections: Vec<String>,
    avatar_url: Option<String>,
    social_accounts: HashMap<String, String>,
}

#[derive(Serialize)]
struct IndexPageContext {
    title: String,
    blog_posts: Vec<BlogPost>,
    feeds: HashMap<String, String>,
    sections: Vec<String>,
    avatar_url: Option<String>,
    social_accounts: HashMap<String, String>,
}

fn human_time(dt: DateTime<Utc>) -> String {
    let local = dt.with_timezone(&Local);
    local.format("%-d %B %Y").to_string()
}

fn extract_title(markdown: &str, fallback: &str) -> String {
    for line in markdown.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("# ") {
            if !rest.starts_with('#') {
                return rest.trim().to_string();
            }
        }
    }
    fallback.to_string()
}

fn file_times(path: &Path) -> (DateTime<Utc>, DateTime<Utc>) {
    let meta = std::fs::metadata(path).expect("file metadata");
    let modified = meta
        .modified()
        .ok()
        .map(DateTime::<Utc>::from)
        .unwrap_or_else(Utc::now);
    (modified, modified)
}

/// Renders the site index (`index.html.j2`).
pub fn render_index_html(
    tera: &Tera,
    page_title: &str,
    blog_posts: Vec<BlogPost>,
    feeds: HashMap<String, String>,
    sections: Vec<String>,
    avatar_url: Option<String>,
    social_accounts: HashMap<String, String>,
) -> Result<String, tera::Error> {
    let ctx = IndexPageContext {
        title: page_title.to_string(),
        blog_posts,
        feeds,
        sections,
        avatar_url,
        social_accounts,
    };
    tera.render(
        "index.html.j2",
        &Context::from_serialize(&ctx).expect("index context"),
    )
}

/// Converts a Markdown file to a full HTML page using the embedded article template.
pub fn markdown_file_to_html(markdown_path: &Path) {
    let md_content = std::fs::read_to_string(markdown_path).expect("read markdown");
    let (created, modified) = file_times(markdown_path);

    let fallback_title = markdown_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("post")
        .to_string();
    let title = extract_title(&md_content, &fallback_title);

    let relative_path = format!(
        "{}.html",
        markdown_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("post")
    );

    let blog_post = BlogPost {
        title,
        author: "Unknown".to_string(),
        description: None,
        creation_dt: created,
        last_update_dt: modified,
        human_time: human_time(modified),
        relative_path,
    };

    let mut options = Options::default();
    options.extension.footnotes = true;
    options.extension.strikethrough = true;
    options.extension.table = true;
    options.extension.tasklist = true;

    let main_content = markdown_to_html(&md_content, &options);

    let ctx = ArticlePageContext {
        blog_post,
        main_content,
        sections: Vec::new(),
        avatar_url: None,
        social_accounts: HashMap::new(),
    };

    let tera = templates::tera();
    let html = tera
        .render(
            "article.html.j2",
            &Context::from_serialize(&ctx).expect("article context"),
        )
        .unwrap_or_else(|e| panic!("render article: {e}"));

    let mut html_path = markdown_path.to_path_buf();
    html_path.set_extension("html");
    std::fs::write(&html_path, html).expect("write html");
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use tempfile::NamedTempFile;

    use super::markdown_file_to_html;

    #[test]
    fn markdown_to_full_html_page() {
        let mut f = NamedTempFile::new().unwrap();
        writeln!(f, "# Hello\n\nBody **bold**.").unwrap();
        markdown_file_to_html(f.path());
        let html_path = f.path().with_extension("html");
        let html = std::fs::read_to_string(&html_path).unwrap();
        assert!(html.contains("<!doctype html>"));
        assert!(html.contains("Hello"));
        assert!(html.contains("<strong>bold</strong>"));
    }
}
