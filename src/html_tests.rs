use std::io::Write;
use std::path::{Path, PathBuf};

use chrono::TimeZone;
use tempfile::NamedTempFile;

use crate::blog_post::BlogPost;
use crate::user_profile::UserProfileMeta;

use super::markdown_file_to_html;

#[test]
fn markdown_to_full_html_page() {
    let mut f = NamedTempFile::new().unwrap();
    writeln!(f, "# Hello\n\nBody **bold**.").unwrap();
    let profile = UserProfileMeta {
        username: "tester".into(),
        ..UserProfileMeta::default()
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
        ..UserProfileMeta::default()
    };
    markdown_file_to_html(&profile, f.path(), Path::new("notes/from-store.md"), "---");
    let html = std::fs::read_to_string(f.path().with_extension("html")).unwrap();
    assert!(html.contains("2020-01-02T10:00:00+00:00"));
    assert!(html.contains("2024-05-06T15:30:00+00:00"));
}
