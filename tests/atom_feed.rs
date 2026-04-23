use gitblog::feed::parse;
use std::path::Path;

#[test]
fn test_parse_example_feed() {
    let xml = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/atom_example.xml"),
    )
    .expect("read tests/atom_example.xml");

    let feed = parse(&xml).expect("parse xml");

    assert_eq!(feed.title, "The latest news from HenriTEL");
    assert_eq!(feed.updated.to_rfc3339(), "2025-09-10T10:59:39+02:00");
    assert_eq!(feed.author.name, "HenriTEL");
    assert_eq!(feed.link.href, "https://blog.henritel.com");
    assert_eq!(feed.logo.as_deref(), Some("https://blog.henritel.com/media/favicon.svg"));
    assert_eq!(feed.subtitle.as_deref(), Some("The latest news from HenriTEL"));

    assert_eq!(feed.entries.len(), 18);

    let first = &feed.entries[0];
    assert_eq!(first.title, "Embedded Systems Level 1");
    assert_eq!(first.link.href, "https://blog.henritel.com/tech/embedded-systems-level-1");
    assert_eq!(first.link.rel, "alternate");
    assert!(!first.summary.is_empty());

    let android = feed.entries.iter().find(|e| e.title == "Android Sucks on Google Phones").unwrap();
    assert!(android.summary.is_empty());
}
