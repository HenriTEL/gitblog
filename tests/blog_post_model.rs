use std::path::PathBuf;

use chrono::{DateTime, FixedOffset};
use gitblog::{
    blog_post::BlogPost, feed::build_feed_from_blog_posts, html::write_index_from_blog_posts,
};
use tempfile::tempdir;

fn parse_ts(s: &str) -> DateTime<FixedOffset> {
    DateTime::parse_from_rfc3339(s).expect("valid timestamp")
}

#[test]
fn builds_atom_feed_from_blog_posts() {
    let posts = vec![
        BlogPost::new(
            PathBuf::from("notes/first.md"),
            parse_ts("2026-04-01T10:00:00+02:00"),
            "First".to_string(),
            "Summary one".to_string(),
        ),
        BlogPost::new(
            PathBuf::from("notes/second.md"),
            parse_ts("2026-04-02T10:00:00+02:00"),
            "Second".to_string(),
            String::new(),
        ),
    ];

    let feed = build_feed_from_blog_posts("https://example.com/blog", &posts, None);
    assert_eq!(feed.entries.len(), 2);
    assert_eq!(feed.updated.to_rfc3339(), "2026-04-02T10:00:00+02:00");
    assert_eq!(
        feed.entries[0].link.href,
        "https://example.com/blog/notes/second.html"
    );
    assert_eq!(feed.entries[1].summary, "Summary one");
}

#[test]
fn writes_index_from_blog_posts() {
    let dir = tempdir().expect("temp dir");
    std::fs::create_dir_all(dir.path().join("media")).expect("media dir");

    let posts = vec![
        BlogPost::new(
            PathBuf::from("notes/zeta.md"),
            parse_ts("2026-04-01T10:00:00+02:00"),
            "Zeta".to_string(),
            "A summary".to_string(),
        ),
        BlogPost::new(
            PathBuf::from("notes/alpha.md"),
            parse_ts("2026-04-03T10:00:00+02:00"),
            "Alpha".to_string(),
            String::new(),
        ),
    ];

    write_index_from_blog_posts(dir.path(), &posts);
    let html = std::fs::read_to_string(dir.path().join("index.html")).expect("read index");
    assert!(html.contains("Alpha"));
    assert!(html.contains("Zeta"));
    assert!(html.contains("/atom.xml"));
}
