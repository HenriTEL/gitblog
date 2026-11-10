use chrono::{Datelike, Timelike};

use super::{
    alert_title, article_render_parts, parse_content_metadata, parse_title_and_summary,
    render_markdown_to_html,
};

#[test]
fn extracts_title_and_multiline_summary() {
    let md = "# Hello\n\n> Line one\n> Line two\n\nBody";
    let (title, summary) = parse_title_and_summary(md, "fallback");
    assert_eq!(title, "Hello");
    assert_eq!(summary, "Line one\nLine two");
}

#[test]
fn empty_summary_when_no_quote_after_title() {
    let md = "# Hello\n\nBody";
    let (title, summary) = parse_title_and_summary(md, "fallback");
    assert_eq!(title, "Hello");
    assert!(summary.is_empty());
}

#[test]
fn ignores_quote_not_after_title() {
    let md = "# Hello\n\nBody\n\n> Later quote";
    let (title, summary) = parse_title_and_summary(md, "fallback");
    assert_eq!(title, "Hello");
    assert!(summary.is_empty());
}

#[test]
fn markdown_render_supports_tables_and_tasklists() {
    let md = "| h |\n| - |\n| a |\n\n- [x] done";
    let html = render_markdown_to_html(md, "---");
    assert!(html.contains("<table>"));
    assert!(html.contains("type=\"checkbox\""));
}

#[test]
fn frontmatter_overrides_title_and_summary_and_sets_date() {
    let md = r"---
title: Frontmatter Title
description: Frontmatter Description
date: 2026-04-24
---
# Body Title

> Body summary
";
    let (title, summary, date, lastmod) = parse_content_metadata(md, "fallback", "---");
    assert_eq!(title, "Frontmatter Title");
    assert_eq!(summary, "Frontmatter Description");
    assert!(lastmod.is_none());
    let date = date.expect("date should be parsed");
    assert_eq!(date.year(), 2026);
    assert_eq!(date.month(), 4);
    assert_eq!(date.day(), 24);
    assert_eq!(date.hour(), 0);
    assert_eq!(date.minute(), 0);
}

#[test]
fn frontmatter_date_parses_rfc3339() {
    let md = r"---
date: 2026-04-24T10:15:30+02:00
---
Body
";
    let (_title, _summary, date, lastmod) = parse_content_metadata(md, "fallback", "---");
    let date = date.expect("date should be parsed");
    assert!(lastmod.is_none());
    assert_eq!(date.year(), 2026);
    assert_eq!(date.month(), 4);
    assert_eq!(date.day(), 24);
    assert_eq!(date.hour(), 10);
    assert_eq!(date.minute(), 15);
}

#[test]
fn frontmatter_lastmod_parses_independently_of_date() {
    let md = r"---
date: 2026-01-01
lastmod: 2026-06-15T12:00:00+02:00
---
Body
";
    let (_title, _summary, published, lastmod) = parse_content_metadata(md, "fallback", "---");
    let published = published.expect("date");
    let lastmod = lastmod.expect("lastmod");
    assert_eq!(published.day(), 1);
    assert_eq!(published.month(), 1);
    assert_eq!(lastmod.day(), 15);
    assert_eq!(lastmod.month(), 6);
    assert_eq!(lastmod.hour(), 12);
}

#[test]
fn falls_back_to_markdown_when_frontmatter_missing_keys() {
    let md = r"---
author: me
---
# Body Title

> Body summary
";
    let (title, summary, date, lastmod) = parse_content_metadata(md, "fallback", "---");
    assert_eq!(title, "Body Title");
    assert_eq!(summary, "Body summary");
    assert!(date.is_none());
    assert!(lastmod.is_none());
}

#[test]
fn supports_custom_frontmatter_delimiter() {
    let md = r"+++
title: Custom
description: Uses custom delimiter
+++
# Body Title

> Body summary
";
    let (title, summary, date, lastmod) = parse_content_metadata(md, "fallback", "+++");
    assert_eq!(title, "Custom");
    assert_eq!(summary, "Uses custom delimiter");
    assert!(date.is_none());
    assert!(lastmod.is_none());
}

#[test]
fn article_render_parts_strips_h1_and_blockquote_from_main() {
    let md = "# Hello\n\n> Line one\n> Line two\n\nBody";
    let p = article_render_parts(md, "fallback", "");
    assert_eq!(p.title, "Hello");
    assert_eq!(p.summary_markdown, "Line one\nLine two");
    assert_eq!(p.body_markdown, "Body");
}

#[test]
fn article_render_parts_keeps_blockquote_in_body_when_description_is_frontmatter() {
    let md = r"---
description: From YAML
---
# Body Title

> Body summary

Rest
";
    let p = article_render_parts(md, "fallback", "---");
    assert_eq!(p.summary_markdown, "From YAML");
    assert!(p.body_markdown.contains("> Body summary"));
    assert!(p.body_markdown.contains("Rest"));
}

#[test]
fn article_render_parts_strips_tldr_lede_paragraph() {
    let md = "# Paginate\n\n**TL;DR** One line.\n\nDetails here.";
    let p = article_render_parts(md, "fallback", "");
    assert_eq!(p.title, "Paginate");
    assert_eq!(p.summary_markdown, "**TL;DR** One line.");
    assert_eq!(p.body_markdown, "Details here.");
}

#[test]
fn article_render_parts_keeps_first_paragraph_in_main_when_not_tldr() {
    let md = "# Hi\n\nIntro without keyword.\n\nMore.";
    let p = article_render_parts(md, "fallback", "");
    assert_eq!(p.summary_markdown, "");
    assert!(p.body_markdown.contains("Intro without keyword"));
}

#[test]
fn all_alert_types_render_with_their_own_class_and_title() {
    for (kind, title) in [
        ("NOTE", "Note"),
        ("TIP", "Tip"),
        ("IMPORTANT", "Important"),
        ("WARNING", "Warning"),
        ("CAUTION", "Caution"),
    ] {
        let html = render_markdown_to_html(&format!("> [!{kind}]\n> Body\n"), "---");
        let class = format!("markdown-alert-{}", kind.to_lowercase());
        assert!(html.contains(&class), "{html}");
        assert!(
            html.contains(&format!(r#"<p class="markdown-alert-title">{title}</p>"#)),
            "{html}"
        );
    }
}

#[test]
fn alert_title_follows_the_same_rule_as_the_renderer() {
    assert_eq!(alert_title("> [!NOTE]").as_deref(), Some("Note"));
    assert_eq!(alert_title("> [!warning]").as_deref(), Some("Warning"));
    assert_eq!(
        alert_title("> [!tip] Read this  ").as_deref(),
        Some("Read this")
    );
    for not_an_alert in [
        "> [!UNKNOWN]",
        ">[!NOTE]",
        ">  [!NOTE]",
        "[!NOTE]",
        "> text",
    ] {
        assert_eq!(alert_title(not_an_alert), None, "{not_an_alert}");
        let html = render_markdown_to_html(&format!("{not_an_alert}\n"), "---");
        assert!(!html.contains("markdown-alert"), "{not_an_alert}: {html}");
    }
}

#[test]
fn alert_under_the_title_stays_in_the_body_instead_of_becoming_the_summary() {
    let markdown = "# Title\n\n> [!WARNING]\n> Careful.\n\nBody\n";

    let parts = article_render_parts(markdown, "fallback", "---");
    assert_eq!(parts.summary_markdown, "");
    assert!(parts.body_markdown.contains("> [!WARNING]"), "{parts:?}");

    let (_, summary) = parse_title_and_summary(markdown, "fallback");
    assert_eq!(summary, "");
}

#[test]
fn plain_quote_under_the_title_is_still_the_summary() {
    let parts = article_render_parts("# Title\n\n> A lede.\n\nBody\n", "fallback", "---");
    assert_eq!(parts.summary_markdown, "A lede.");
    assert!(!parts.body_markdown.contains("A lede."), "{parts:?}");
}
