use chrono::{DateTime, FixedOffset, NaiveDate, TimeZone};
use comrak::{Options, markdown_to_html};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frontmatter {
    pub title: Option<String>,
    pub description: Option<String>,
    /// Publication time (`date` in front matter).
    pub date: Option<DateTime<FixedOffset>>,
    /// Last modified time (`lastmod` in front matter).
    pub lastmod: Option<DateTime<FixedOffset>>,
}

impl Frontmatter {
    pub fn is_empty(&self) -> bool {
        self.title.is_none()
            && self.description.is_none()
            && self.date.is_none()
            && self.lastmod.is_none()
    }
}

fn frontmatter_delimiter_option(frontmatter_delimiter: &str) -> Option<String> {
    if frontmatter_delimiter.trim().is_empty() {
        None
    } else {
        Some(frontmatter_delimiter.to_string())
    }
}

fn parse_frontmatter_fields(raw: &str) -> Frontmatter {
    let mut frontmatter = Frontmatter {
        title: None,
        description: None,
        date: None,
        lastmod: None,
    };

    for line in raw.lines() {
        let Some((raw_key, raw_value)) = line.split_once(':') else {
            continue;
        };
        let key = raw_key.trim();
        let value = raw_value.trim().trim_matches('"').trim_matches('\'');
        if value.is_empty() {
            continue;
        }

        match key {
            "title" => frontmatter.title = Some(value.to_string()),
            "description" => frontmatter.description = Some(value.to_string()),
            "date" => frontmatter.date = parse_frontmatter_date(value),
            "lastmod" => frontmatter.lastmod = parse_frontmatter_date(value),
            _ => {}
        }
    }

    frontmatter
}

fn parse_frontmatter_date(value: &str) -> Option<DateTime<FixedOffset>> {
    DateTime::parse_from_rfc3339(value).ok().or_else(|| {
        NaiveDate::parse_from_str(value, "%Y-%m-%d")
            .ok()
            .and_then(|date| {
                FixedOffset::east_opt(0)?
                    .from_local_datetime(&date.and_hms_opt(0, 0, 0)?)
                    .single()
            })
    })
}

pub fn parse_title_and_summary(markdown: &str, fallback: &str) -> (String, String) {
    let lines: Vec<&str> = markdown.lines().collect();
    let mut title = fallback.to_string();
    let mut title_idx = None;

    for (idx, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("# ") {
            if !rest.starts_with('#') {
                title = rest.trim().to_string();
                title_idx = Some(idx);
                break;
            }
        }
    }

    let Some(mut idx) = title_idx.map(|i| i + 1) else {
        return (title, String::new());
    };

    while idx < lines.len() && lines[idx].trim().is_empty() {
        idx += 1;
    }

    let mut quote_lines = Vec::new();
    while idx < lines.len() {
        let trimmed = lines[idx].trim();
        if let Some(rest) = trimmed.strip_prefix('>') {
            let content = rest.strip_prefix(' ').unwrap_or(rest).to_string();
            quote_lines.push(content);
            idx += 1;
            continue;
        }
        break;
    }

    (title, quote_lines.join("\n"))
}

/// YAML / metadata block without delimiters, paired with markdown body after the closing delimiter.
fn split_frontmatter_raw(markdown: &str, delimiter: &str) -> (String, String) {
    let d = delimiter.trim();
    if d.is_empty() {
        return (String::new(), markdown.to_string());
    }
    let lines: Vec<&str> = markdown.lines().collect();
    if lines.is_empty() || lines[0].trim() != d {
        return (String::new(), markdown.to_string());
    }
    let mut close_idx = None;
    for i in 1..lines.len() {
        if lines[i].trim() == d {
            close_idx = Some(i);
            break;
        }
    }
    let Some(close) = close_idx else {
        return (String::new(), markdown.to_string());
    };
    let fm_inner = lines[1..close].join("\n");
    let rest = lines[close + 1..].join("\n");
    (fm_inner, rest)
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum BodySummaryKind {
    None,
    /// De-quoted lines joined with `\n`, same shape as [`parse_title_and_summary`].
    Blockquote {
        start: usize,
        end: usize,
        markdown: String,
    },
    /// First paragraph after the title when it reads like a TL;DR lede.
    TldrParagraph {
        start: usize,
        end: usize,
        markdown: String,
    },
}

/// Title, summary markdown, optional dates, and markdown for `<main>` (leading title / lede removed).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArticleRenderParts {
    pub title: String,
    pub summary_markdown: String,
    pub publication_date: Option<DateTime<FixedOffset>>,
    pub last_modified: Option<DateTime<FixedOffset>>,
    pub body_markdown: String,
}

fn line_looks_like_tldr_lede(text: &str) -> bool {
    let lower = text.to_lowercase();
    lower.contains("tl;dr") || lower.contains("tldr")
}

fn parse_body_intro(
    lines: &[&str],
    fallback_title: &str,
) -> (String, Option<usize>, BodySummaryKind) {
    let mut i = 0usize;
    while i < lines.len() && lines[i].trim().is_empty() {
        i += 1;
    }
    if i >= lines.len() {
        return (fallback_title.to_string(), None, BodySummaryKind::None);
    }

    let trimmed = lines[i].trim();
    let mut h1_idx = None;
    let mut body_title = fallback_title.to_string();
    if let Some(rest) = trimmed.strip_prefix("# ") {
        if !rest.starts_with('#') {
            body_title = rest.trim().to_string();
            h1_idx = Some(i);
            i += 1;
        }
    }

    while i < lines.len() && lines[i].trim().is_empty() {
        i += 1;
    }

    if i >= lines.len() {
        return (body_title, h1_idx, BodySummaryKind::None);
    }

    if lines[i].trim_start().starts_with('>') {
        let start = i;
        let mut dequoted = Vec::new();
        while i < lines.len() {
            let t = lines[i].trim();
            if let Some(rest) = t.strip_prefix('>') {
                let content = rest.strip_prefix(' ').unwrap_or(rest);
                dequoted.push(content.to_string());
                i += 1;
            } else {
                break;
            }
        }
        if dequoted.is_empty() {
            return (body_title, h1_idx, BodySummaryKind::None);
        }
        let end = i - 1;
        let markdown = dequoted.join("\n");
        return (
            body_title,
            h1_idx,
            BodySummaryKind::Blockquote {
                start,
                end,
                markdown,
            },
        );
    }

    // First paragraph run (until blank line).
    let start = i;
    let mut para = Vec::new();
    while i < lines.len() && !lines[i].trim().is_empty() {
        para.push(lines[i]);
        i += 1;
    }
    let joined = para.join("\n");
    if joined.is_empty() || !line_looks_like_tldr_lede(&joined) {
        return (body_title, h1_idx, BodySummaryKind::None);
    }
    let end = i - 1;
    (
        body_title,
        h1_idx,
        BodySummaryKind::TldrParagraph {
            start,
            end,
            markdown: joined,
        },
    )
}

fn build_body_markdown_after_strip(
    rest_lines: &[&str],
    h1_idx: Option<usize>,
    body_summary: &BodySummaryKind,
    strip_body_summary: bool,
) -> String {
    let mut skip = std::collections::HashSet::new();
    if let Some(idx) = h1_idx {
        skip.insert(idx);
    }
    if strip_body_summary {
        match body_summary {
            BodySummaryKind::Blockquote { start, end, .. }
            | BodySummaryKind::TldrParagraph { start, end, .. } => {
                for line in *start..=*end {
                    skip.insert(line);
                }
            }
            BodySummaryKind::None => {}
        }
    }
    let out: Vec<&str> = rest_lines
        .iter()
        .enumerate()
        .filter_map(|(idx, line)| {
            if skip.contains(&idx) {
                None
            } else {
                Some(*line)
            }
        })
        .collect();
    out.join("\n").trim().to_string()
}

/// Parses front matter and body intro, then returns metadata plus markdown for the article body
/// without the displayed title and lede (blockquote summary, TL;DR paragraph, or nothing when
/// description comes only from front matter).
pub fn article_render_parts(
    markdown: &str,
    fallback_title: &str,
    frontmatter_delimiter: &str,
) -> ArticleRenderParts {
    let (fm_inner, rest) = split_frontmatter_raw(markdown, frontmatter_delimiter);
    let frontmatter = parse_frontmatter_fields(&fm_inner);
    let rest_lines: Vec<&str> = rest.lines().collect();
    let (body_title, h1_idx, body_summary_kind) = parse_body_intro(&rest_lines, fallback_title);

    let title = frontmatter
        .title
        .clone()
        .unwrap_or_else(|| body_title.clone());

    let summary_from_body = match &body_summary_kind {
        BodySummaryKind::Blockquote { markdown, .. }
        | BodySummaryKind::TldrParagraph { markdown, .. } => markdown.clone(),
        BodySummaryKind::None => String::new(),
    };

    let summary_markdown = frontmatter.description.clone().unwrap_or(summary_from_body);

    let strip_body_summary =
        frontmatter.description.is_none() && !matches!(body_summary_kind, BodySummaryKind::None);

    let body_markdown = build_body_markdown_after_strip(
        &rest_lines,
        h1_idx,
        &body_summary_kind,
        strip_body_summary,
    );

    ArticleRenderParts {
        title,
        summary_markdown,
        publication_date: frontmatter.date,
        last_modified: frontmatter.lastmod,
        body_markdown,
    }
}

pub fn parse_content_metadata(
    markdown: &str,
    fallback_title: &str,
    frontmatter_delimiter: &str,
) -> (
    String,
    String,
    Option<DateTime<FixedOffset>>,
    Option<DateTime<FixedOffset>>,
) {
    let parts = article_render_parts(markdown, fallback_title, frontmatter_delimiter);
    (
        parts.title,
        parts.summary_markdown,
        parts.publication_date,
        parts.last_modified,
    )
}

/// Renders a short markdown fragment (no front matter) to HTML.
pub fn render_markdown_fragment_to_html(markdown: &str) -> String {
    render_markdown_to_html(markdown, "")
}

/// If `html` is a single top-level `<p>…</p>` from comrak, returns the inner HTML for embedding.
pub fn unwrap_single_paragraph_html(html: &str) -> String {
    let t = html.trim();
    let Some(rest) = t.strip_prefix("<p>") else {
        return t.to_string();
    };
    let Some(inner) = rest.strip_suffix("</p>") else {
        return t.to_string();
    };
    if inner.contains("</p>") {
        return t.to_string();
    }
    inner.trim().to_string()
}

/// Plain text suitable for `<meta name="description">` and Atom summaries.
pub fn markdown_fragment_to_plain_text(markdown: &str) -> String {
    if markdown.trim().is_empty() {
        return String::new();
    }
    let html = render_markdown_fragment_to_html(markdown);
    strip_html_tags_collapse_ws(&html)
}

fn strip_html_tags_collapse_ws(html: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    for ch in html.chars() {
        match ch {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub fn article_description_html_fragment(summary_markdown: &str) -> Option<String> {
    let t = summary_markdown.trim();
    if t.is_empty() {
        return None;
    }
    let html = render_markdown_fragment_to_html(t);
    let inner = unwrap_single_paragraph_html(&html);
    if inner.is_empty() { None } else { Some(inner) }
}

pub fn render_markdown_to_html(markdown: &str, frontmatter_delimiter: &str) -> String {
    let mut options = Options::default();
    options.extension.footnotes = true;
    options.extension.strikethrough = true;
    options.extension.table = true;
    options.extension.tasklist = true;
    options.extension.alerts = true;
    options.extension.front_matter_delimiter = frontmatter_delimiter_option(frontmatter_delimiter);

    markdown_to_html(markdown, &options)
}

#[cfg(test)]
mod tests {
    use chrono::{Datelike, Timelike};

    use super::{
        article_render_parts, parse_content_metadata, parse_title_and_summary,
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
        let md = r#"---
title: Frontmatter Title
description: Frontmatter Description
date: 2026-04-24
---
# Body Title

> Body summary
"#;
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
        let md = r#"---
date: 2026-04-24T10:15:30+02:00
---
Body
"#;
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
        let md = r#"---
date: 2026-01-01
lastmod: 2026-06-15T12:00:00+02:00
---
Body
"#;
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
        let md = r#"---
author: me
---
# Body Title

> Body summary
"#;
        let (title, summary, date, lastmod) = parse_content_metadata(md, "fallback", "---");
        assert_eq!(title, "Body Title");
        assert_eq!(summary, "Body summary");
        assert!(date.is_none());
        assert!(lastmod.is_none());
    }

    #[test]
    fn supports_custom_frontmatter_delimiter() {
        let md = r#"+++
title: Custom
description: Uses custom delimiter
+++
# Body Title

> Body summary
"#;
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
        let md = r#"---
description: From YAML
---
# Body Title

> Body summary

Rest
"#;
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
}
