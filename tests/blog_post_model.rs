use std::path::PathBuf;

use chrono::{DateTime, FixedOffset};
use gitblog::{
    blog_post::BlogPost,
    feed::{build_feed_from_blog_posts, generate},
    gemini::write_index_gemtext,
    html::write_index_from_blog_posts,
    user_profile::UserProfileMeta,
};
use tempfile::tempdir;

fn parse_ts(s: &str) -> DateTime<FixedOffset> {
    DateTime::parse_from_rfc3339(s).expect("valid timestamp")
}

#[test]
fn builds_atom_feed_from_blog_posts() {
    let mut first = BlogPost::new(
        PathBuf::from("notes/first.md"),
        parse_ts("2026-04-01T10:00:00+02:00"),
        "First".to_string(),
        "Summary one".to_string(),
    );
    first.publication_date = Some(parse_ts("2026-04-01T10:00:00+02:00"));
    let mut second = BlogPost::new(
        PathBuf::from("notes/second.md"),
        parse_ts("2026-04-02T10:00:00+02:00"),
        "Second".to_string(),
        String::new(),
    );
    second.publication_date = Some(parse_ts("2026-04-02T10:00:00+02:00"));
    let posts = vec![first, second];

    let feed = build_feed_from_blog_posts("https://example.com/blog", &posts, None);
    assert_eq!(feed.entries.len(), 2);
    assert_eq!(feed.updated.to_rfc3339(), "2026-04-02T10:00:00+02:00");
    assert_eq!(
        feed.entries[0].link.href,
        "https://example.com/blog/notes/second.html"
    );
    assert_eq!(feed.entries[1].summary, "Summary one");

    let xml = generate(&feed).expect("atom xml generation");
    assert!(xml.contains("<published>2026-04-02T10:00:00+02:00</published>"));
    assert!(xml.contains("<published>2026-04-01T10:00:00+02:00</published>"));
    assert!(xml.contains("<updated>2026-04-02T10:00:00+02:00</updated>"));
    assert!(xml.contains("<updated>2026-04-01T10:00:00+02:00</updated>"));
    assert!(!xml.contains("T10:00:00.000"));
}

#[test]
fn writes_index_from_blog_posts() {
    let dir = tempdir().expect("temp dir");
    std::fs::create_dir_all(dir.path().join("media")).expect("media dir");

    let mut tech_post = BlogPost::new(
        PathBuf::from("tech/hello-world.md"),
        parse_ts("2026-04-01T10:00:00+02:00"),
        "Hello World".to_string(),
        "Tech summary".to_string(),
    );
    tech_post.publication_date = Some(parse_ts("2026-04-01T10:00:00+02:00"));
    assert_eq!(tech_post.section.as_deref(), Some("tech"));

    let mut rant_post = BlogPost::new(
        PathBuf::from("rant/tenant-hell.md"),
        parse_ts("2026-04-03T10:00:00+02:00"),
        "Tenant Hell".to_string(),
        String::new(),
    );
    rant_post.publication_date = Some(parse_ts("2026-04-03T10:00:00+02:00"));
    assert_eq!(rant_post.section.as_deref(), Some("rant"));

    let posts = vec![tech_post, rant_post];

    let profile = UserProfileMeta {
        username: "author".into(),
        bio: String::new(),
    };

    write_index_from_blog_posts(dir.path(), &profile, &posts);
    let home = std::fs::read_to_string(dir.path().join("index.html")).expect("read index");
    assert!(home.contains("Hello World"));
    assert!(home.contains("Tenant Hell"));
    assert!(home.contains("/atom.xml"));
    assert!(home.contains(r#"href="/tech""#));
    assert!(home.contains(r#"href="/rant""#));

    let tech_index =
        std::fs::read_to_string(dir.path().join("tech/index.html")).expect("read tech index");
    assert!(tech_index.contains("Hello World"));
    assert!(!tech_index.contains("Tenant Hell"));

    let rant_index =
        std::fs::read_to_string(dir.path().join("rant/index.html")).expect("read rant index");
    assert!(rant_index.contains("Tenant Hell"));
    assert!(!rant_index.contains("Hello World"));

    write_index_gemtext(dir.path(), &posts).expect("write gemini index");
    let gmi = std::fs::read_to_string(dir.path().join("index.gmi")).expect("read gemini index");
    assert!(gmi.contains("# Blog"));
    assert!(gmi.contains("=> /rant/tenant-hell.gmi Tenant Hell"));
    assert!(gmi.contains("=> /tech/hello-world.gmi Hello World"));
    let rant_idx = gmi
        .find("=> /rant/tenant-hell.gmi Tenant Hell")
        .expect("rant entry");
    let tech_idx = gmi
        .find("=> /tech/hello-world.gmi Hello World")
        .expect("tech entry");
    assert!(rant_idx < tech_idx);
}
