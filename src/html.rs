use std::collections::HashMap;
use std::path::Path;

use chrono::{DateTime, FixedOffset, Local, SecondsFormat, Utc};
use serde::Serialize;
use tera::{Context, Tera};

use crate::blog_post::{BlogPost as DomainBlogPost, fallback_title};
use crate::markdown::{parse_title_and_summary, render_markdown_to_html};
use crate::templates;

/// Metadata for a single post, shared by article and index templates.
#[derive(Debug, Clone, Serialize)]
pub struct RenderBlogPost {
    pub title: String,
    pub author: String,
    pub description: Option<String>,
    pub creation_dt: DateTime<FixedOffset>,
    pub last_update_dt: DateTime<FixedOffset>,
    pub creation_dt_rfc3339: String,
    pub last_update_dt_rfc3339: String,
    pub human_time: String,
    /// Site-relative URL path (e.g. `notes/foo.html`).
    pub relative_path: String,
}

#[derive(Serialize)]
struct ArticlePageContext {
    blog_post: RenderBlogPost,
    main_content: String,
    sections: Vec<String>,
    avatar_url: Option<String>,
    social_accounts: HashMap<String, String>,
}

#[derive(Serialize)]
struct IndexPageContext {
    title: String,
    blog_posts: Vec<RenderBlogPost>,
    feeds: HashMap<String, String>,
    sections: Vec<String>,
    avatar_url: Option<String>,
    social_accounts: HashMap<String, String>,
}

fn human_time(dt: DateTime<FixedOffset>) -> String {
    let local = dt.with_timezone(&Local);
    local.format("%-d %B %Y").to_string()
}

fn format_rfc3339_seconds(dt: DateTime<FixedOffset>) -> String {
    dt.to_rfc3339_opts(SecondsFormat::Secs, false)
}

fn file_times(path: &Path) -> (DateTime<FixedOffset>, DateTime<FixedOffset>) {
    let meta = std::fs::metadata(path).expect("file metadata");
    let modified = meta
        .modified()
        .ok()
        .map(DateTime::<Utc>::from)
        .unwrap_or_else(Utc::now)
        .with_timezone(&Local)
        .fixed_offset();
    (modified, modified)
}

/// Renders the site index (`index.html.j2`).
pub fn render_index_html(
    tera: &Tera,
    page_title: &str,
    blog_posts: Vec<RenderBlogPost>,
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

pub fn write_index_from_blog_posts(dest: &Path, posts: &[DomainBlogPost]) {
    let mut rendered_posts = posts
        .iter()
        .map(|post| {
            let updated = post.last_updated;
            RenderBlogPost {
                title: post.title.clone(),
                author: "Unknown".to_string(),
                description: if post.summary.is_empty() {
                    None
                } else {
                    Some(post.summary.clone())
                },
                creation_dt: updated,
                last_update_dt: updated,
                creation_dt_rfc3339: format_rfc3339_seconds(updated),
                last_update_dt_rfc3339: format_rfc3339_seconds(updated),
                human_time: human_time(updated),
                relative_path: post
                    .path
                    .with_extension("html")
                    .to_string_lossy()
                    .to_string(),
            }
        })
        .collect::<Vec<_>>();
    rendered_posts.sort_by(|a, b| b.last_update_dt.cmp(&a.last_update_dt));

    let feeds = HashMap::from([("atom".to_string(), "/atom.xml".to_string())]);
    let html = render_index_html(
        templates::tera(),
        "Blog",
        rendered_posts,
        feeds,
        Vec::new(),
        None,
        HashMap::new(),
    )
    .expect("render index");
    std::fs::write(dest.join("index.html"), html).expect("write index");
}

/// Converts a Markdown file to a full HTML page using the embedded article template.
pub fn markdown_file_to_html(markdown_path: &Path) {
    let md_content = std::fs::read_to_string(markdown_path).expect("read markdown");
    // TODO: use git commit times
    let (created, modified) = file_times(markdown_path);

    let fallback_title = fallback_title(markdown_path);
    let (title, summary) = parse_title_and_summary(&md_content, &fallback_title);

    let relative_path = format!(
        "{}.html",
        markdown_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("post")
    );

    let blog_post = RenderBlogPost {
        title,
        author: "Unknown".to_string(),
        description: if summary.is_empty() {
            None
        } else {
            Some(summary)
        },
        creation_dt: created,
        last_update_dt: modified,
        creation_dt_rfc3339: format_rfc3339_seconds(created),
        last_update_dt_rfc3339: format_rfc3339_seconds(modified),
        human_time: human_time(modified),
        relative_path,
    };

    let main_content = render_markdown_to_html(&md_content);

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
        let published_line = html
            .lines()
            .find(|line| line.contains("article:published_time"))
            .expect("published meta tag");
        let modified_line = html
            .lines()
            .find(|line| line.contains("article:modified_time"))
            .expect("modified meta tag");

        assert!(!published_line.contains('.'));
        assert!(!modified_line.contains('.'));
        assert!(!published_line.contains('Z'));
        assert!(!modified_line.contains('Z'));
    }
}
