use std::{io::Write, path::PathBuf};

use chrono::{DateTime, FixedOffset};
use tempfile::{NamedTempFile, tempdir};

use crate::blog_post::BlogPost;

use super::{markdown_file_to_gemtext, markdown_to_gemtext, write_index_gemtext};

fn parse_ts(s: &str) -> DateTime<FixedOffset> {
    DateTime::parse_from_rfc3339(s).expect("valid timestamp")
}

#[test]
fn markdown_to_gemtext_converts_core_structures() {
    let md = "# Title\n\nSome [link](https://example.com).\n\n- One\n1. Two\n\n[Standalone](gemini://capsule/path)\n";
    let gemtext = markdown_to_gemtext(md);
    assert!(gemtext.contains("# Title"));
    assert!(gemtext.contains("Some link (https://example.com)."));
    assert!(gemtext.contains("* One"));
    assert!(gemtext.contains("* Two"));
    assert!(gemtext.contains("=> gemini://capsule/path Standalone"));
}

#[test]
fn markdown_file_writes_sibling_gmi() {
    let mut f = NamedTempFile::new().expect("temp file");
    writeln!(f, "# Custom Title\n\n## Hello\n\nBody").expect("write markdown");

    markdown_file_to_gemtext(f.path(), "").expect("generate gemtext file");

    let output_path = f.path().with_extension("gmi");
    let gemtext = std::fs::read_to_string(output_path).expect("read gemtext");
    assert!(gemtext.contains("# Custom Title"));
    assert!(gemtext.contains("## Hello"));
    assert!(gemtext.contains("Body"));
}

#[test]
fn writes_sorted_index_gemtext() {
    let dir = tempdir().expect("temp dir");
    let posts = vec![
        BlogPost::new(
            PathBuf::from("notes/older.md"),
            parse_ts("2026-04-01T10:00:00+02:00"),
            "Older".to_string(),
            String::new(),
        ),
        BlogPost::new(
            PathBuf::from("notes/newer.md"),
            parse_ts("2026-04-03T10:00:00+02:00"),
            "Newer".to_string(),
            String::new(),
        ),
    ];

    write_index_gemtext(dir.path(), &posts).expect("write index");
    let index = std::fs::read_to_string(dir.path().join("index.gmi")).expect("read index");
    assert!(index.starts_with("# Blog"));
    let newer_idx = index
        .find("=> /notes/newer.gmi Newer")
        .expect("newer entry");
    let older_idx = index
        .find("=> /notes/older.gmi Older")
        .expect("older entry");
    assert!(newer_idx < older_idx);
}
