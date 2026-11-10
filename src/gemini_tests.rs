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

#[test]
fn markdown_extension_markup_is_left_out() {
    let gemtext = markdown_to_gemtext(
        "[TOC]\n\n## Title {: #custom }\n\nText\n{: .lead }\n\n*[HTML]: Hyper Text\n\n```\n[TOC]\n```\n",
    );
    assert_eq!(gemtext, "\n## Title\n\nText\n\n\n```\n[TOC]\n```\n");
}

#[test]
fn toc_token_variations_are_left_out_except_in_code() {
    let gemtext = markdown_to_gemtext("[ toc ]\n\nText\n\n[Toc]\n\n```\n[ toc ]\n```\n");
    assert_eq!(gemtext, "\nText\n\n\n```\n[ toc ]\n```\n");
}

#[test]
fn alerts_become_a_quote_headed_by_their_title() {
    assert_eq!(
        markdown_to_gemtext("> [!WARNING]\n> Careful.\n\n> [!note] Heads up\n> Read.\n"),
        "> Warning\n> Careful.\n\n> Heads up\n> Read.\n"
    );
}

#[test]
fn alert_syntax_in_code_or_inside_a_quote_is_left_alone() {
    let markdown = "```\n> [!NOTE]\n```\n\n> Quote\n> [!NOTE]\n";
    assert_eq!(markdown_to_gemtext(markdown), markdown);
}

#[test]
fn bold_and_italic_markers_are_removed() {
    for (markdown, expected) in [
        ("**bold**", "bold"),
        ("__bold__", "bold"),
        ("*italic*", "italic"),
        ("_italic_", "italic"),
        ("***both***", "both"),
        ("___both___", "both"),
        ("***a** b*", "a b"),
        ("**a *b* c**", "a b c"),
        ("mid*word*emphasis", "midwordemphasis"),
        ("(**punctuated**), yes", "(punctuated), yes"),
    ] {
        assert_eq!(markdown_to_gemtext(markdown), format!("{expected}\n"));
    }
}

#[test]
fn emphasis_is_removed_in_every_kind_of_line() {
    assert_eq!(
        markdown_to_gemtext(
            "## A **big** title\n\n- *item*\n1. __first__\n\n> _quoted_\n\n[**label**](https://x.io)\n\nSee [*this*](https://x.io) now\n"
        ),
        "## A big title\n\n* item\n* first\n\n> quoted\n\n=> https://x.io label\n\nSee this (https://x.io) now\n"
    );
}

#[test]
fn emphasis_can_span_the_lines_of_a_paragraph() {
    assert_eq!(
        markdown_to_gemtext("Some **bold\nacross lines** here\n"),
        "Some bold\nacross lines here\n"
    );
}

#[test]
fn emphasis_does_not_pair_across_separate_blocks() {
    for markdown in ["a *b\n\nc* d\n", "- a *b\n- c* d\n", "# a *b\nc* d\n"] {
        let expected = markdown.replace("- ", "* ");
        assert_eq!(markdown_to_gemtext(markdown), expected, "{markdown}");
    }
}

#[test]
fn text_that_only_looks_like_emphasis_is_kept() {
    for text in [
        "snake_case_name and __no_op",
        "2 * 3 * 4 = 24",
        "a ** b ** c",
        "an unmatched *star and _underscore",
        r"\*escaped\* and \_too\_",
        "`*code*` and ``**more** ` code``",
        "https://x.io/_a_/*b*/c",
        "<https://x.io/_a_>",
        r#"<span title="*x*">y</span>"#,
        "$a*b*c$ and $$d*e*f$$",
    ] {
        assert_eq!(markdown_to_gemtext(text), format!("{text}\n"));
    }
}

#[test]
fn link_targets_keep_their_markers() {
    assert_eq!(
        markdown_to_gemtext("[a](https://x.io/_b_/*c*)\n\nSee [a](/_b_) now\n"),
        "=> https://x.io/_b_/*c* a\n\nSee a (/_b_) now\n"
    );
}

#[test]
fn bold_around_a_url_is_removed_without_touching_it() {
    assert_eq!(
        markdown_to_gemtext("**https://x.io/a_b_c**\n"),
        "https://x.io/a_b_c\n"
    );
}

#[test]
fn block_syntax_made_of_markers_is_kept() {
    assert_eq!(
        markdown_to_gemtext("* item *one*\n\n***\n\n___\n\n* * *\n"),
        "* item one\n\n***\n\n___\n\n* * *\n"
    );
}

#[test]
fn code_blocks_keep_their_markers() {
    let markdown = "```\n**bold** _x_\n```\n\n    **indented** _code_\n";
    assert_eq!(
        markdown_to_gemtext(markdown),
        "```\n**bold** _x_\n```\n\n**indented** _code_\n"
    );
}

#[test]
fn markup_only_lines_survive_so_they_can_be_dropped() {
    assert_eq!(
        markdown_to_gemtext("*[HTML]: Hyper *Text*\n\nDone\n"),
        "\nDone\n"
    );
}

#[test]
fn titles_lose_their_markers_on_pages_and_in_the_index() {
    let dir = tempdir().expect("temp dir");
    let source = dir.path().join("post.md");
    std::fs::write(&source, "# A **bold** title\n\nBody\n").expect("write source");
    markdown_file_to_gemtext(&source, "---").expect("write gemtext");
    let page = std::fs::read_to_string(source.with_extension("gmi")).expect("read page");
    assert!(page.starts_with("# A bold title\n"), "{page}");

    let date = parse_ts("2024-01-01T00:00:00+00:00");
    let post = BlogPost::from_source(
        PathBuf::from("post.md"),
        "An *italic* title".to_string(),
        String::new(),
        date,
        date,
    );
    write_index_gemtext(dir.path(), &[post]).expect("write index");
    let index = std::fs::read_to_string(dir.path().join("index.gmi")).expect("read index");
    assert!(
        index.contains("=> /post.gmi An italic title (2024-01-01)"),
        "{index}"
    );
}
