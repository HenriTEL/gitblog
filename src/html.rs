use std::collections::HashMap;
use std::path::Path;

use chrono::{DateTime, FixedOffset, Local, SecondsFormat, Utc};
use serde::Serialize;
use tera::{Context, Tera};

use crate::blog_post::{self, BlogPost as DomainBlogPost, fallback_title};
use crate::markdown::{
    article_description_html_fragment, article_render_parts, markdown_fragment_to_plain_text,
    render_markdown_to_html,
};
use crate::templates;
use crate::user_profile::UserProfileMeta;

/// Metadata for a single post, shared by article and index templates.
#[derive(Debug, Clone, Serialize)]
pub struct RenderBlogPost {
    pub title: String,
    pub author: String,
    /// Plain text for `<meta name="description">`, the index listing, and Atom summaries.
    pub description: Option<String>,
    /// Rendered HTML for the article header lede (may contain inline markup).
    pub description_html: Option<String>,
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
    author_name: String,
    blog_posts: Vec<RenderBlogPost>,
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

fn resolve_article_dates(
    store: Option<&DomainBlogPost>,
    frontmatter_publication: Option<DateTime<FixedOffset>>,
    frontmatter_last_modified: Option<DateTime<FixedOffset>>,
) -> (DateTime<FixedOffset>, DateTime<FixedOffset>) {
    let published = frontmatter_publication
        .or_else(|| store.map(DomainBlogPost::effective_publication_date))
        .or(frontmatter_last_modified)
        .unwrap_or_else(|| Utc::now().fixed_offset());
    let last_updated = frontmatter_last_modified
        .or_else(|| store.map(|p| p.last_updated))
        .unwrap_or(published);
    (published, last_updated)
}

/// Renders the site index (`index.html.j2`).
pub fn render_index_html(
    tera: &Tera,
    title: String,
    author_name: String,
    blog_posts: Vec<RenderBlogPost>,
    sections: Vec<String>,
    avatar_url: Option<String>,
    social_accounts: HashMap<String, String>,
) -> Result<String, tera::Error> {
    let ctx = IndexPageContext {
        title,
        author_name,
        blog_posts,
        sections,
        avatar_url,
        social_accounts,
    };
    tera.render(
        "index.html.j2",
        &Context::from_serialize(&ctx).expect("index context"),
    )
}

pub fn write_index_from_blog_posts(
    dest: &Path,
    user_profile: &UserProfileMeta,
    posts: &[DomainBlogPost],
) {
    let mut rendered_posts = posts
        .iter()
        .map(|post| {
            let published = post.effective_publication_date();
            let updated = post.last_updated;
            RenderBlogPost {
                title: post.title.clone(),
                author: user_profile.username.clone(),
                description: if post.summary.is_empty() {
                    None
                } else {
                    Some(markdown_fragment_to_plain_text(&post.summary))
                },
                description_html: None,
                creation_dt: published,
                last_update_dt: updated,
                creation_dt_rfc3339: format_rfc3339_seconds(published),
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
    rendered_posts.sort_by(|a, b| b.creation_dt.cmp(&a.creation_dt));

    let html = render_index_html(
        templates::tera(),
        "Home".to_string(),
        user_profile.username.clone(),
        rendered_posts,
        Vec::new(),
        None,
        HashMap::new(),
    )
    .expect("render index");
    std::fs::write(dest.join("index.html"), html).expect("write index");
}

/// Converts a Markdown file to a full HTML page using the embedded article template.
pub fn markdown_file_to_html(
    user_profile: &UserProfileMeta,
    markdown_path: &Path,
    source_rel_path: &Path,
    frontmatter_delimiter: &str,
) {
    let md_content = std::fs::read_to_string(markdown_path).expect("read markdown");

    let fallback_title = fallback_title(source_rel_path);
    let parts = article_render_parts(&md_content, &fallback_title, frontmatter_delimiter);
    let store_post = blog_post::get_by_path(source_rel_path);
    let (published, last_updated) = resolve_article_dates(
        store_post.as_ref(),
        parts.publication_date,
        parts.last_modified,
    );

    let relative_path = source_rel_path
        .with_extension("html")
        .to_string_lossy()
        .to_string();

    let description_plain = markdown_fragment_to_plain_text(&parts.summary_markdown);
    let blog_post = RenderBlogPost {
        title: parts.title,
        author: user_profile.username.clone(),
        description: if description_plain.is_empty() {
            None
        } else {
            Some(description_plain)
        },
        description_html: article_description_html_fragment(&parts.summary_markdown),
        creation_dt: published,
        last_update_dt: last_updated,
        creation_dt_rfc3339: format_rfc3339_seconds(published),
        last_update_dt_rfc3339: format_rfc3339_seconds(last_updated),
        human_time: human_time(last_updated),
        relative_path,
    };

    let main_content = render_markdown_to_html(&parts.body_markdown, frontmatter_delimiter);

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
    use std::path::{Path, PathBuf};

    use tempfile::NamedTempFile;

    use chrono::TimeZone;

    use crate::blog_post::BlogPost;
    use crate::user_profile::UserProfileMeta;

    use super::markdown_file_to_html;

    #[test]
    fn markdown_to_full_html_page() {
        let mut f = NamedTempFile::new().unwrap();
        writeln!(f, "# Hello\n\nBody **bold**.").unwrap();
        let profile = UserProfileMeta {
            username: "tester".into(),
            bio: String::new(),
        };
        markdown_file_to_html(&profile, f.path(), Path::new("post.md"), "---");
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

    #[test]
    fn markdown_uses_blog_post_metadata_for_article_dates() {
        let mut f = NamedTempFile::new().unwrap();
        writeln!(f, "# Hello\n\nBody.").unwrap();
        let published = chrono::Utc
            .with_ymd_and_hms(2020, 1, 2, 10, 0, 0)
            .unwrap()
            .fixed_offset();
        let updated = chrono::Utc
            .with_ymd_and_hms(2024, 5, 6, 15, 30, 0)
            .unwrap()
            .fixed_offset();
        let mut post = BlogPost::new(
            PathBuf::from("notes/from-store.md"),
            updated,
            "Stored".to_string(),
            String::new(),
        );
        post.publication_date = Some(published);
        crate::blog_post::upsert(post);

        let profile = UserProfileMeta {
            username: "tester".into(),
            bio: String::new(),
        };
        markdown_file_to_html(
            &profile,
            f.path(),
            Path::new("notes/from-store.md"),
            "---",
        );
        let html = std::fs::read_to_string(f.path().with_extension("html")).unwrap();
        assert!(html.contains("2020-01-02T10:00:00+00:00"));
        assert!(html.contains("2024-05-06T15:30:00+00:00"));
    }
}
