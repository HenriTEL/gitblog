use chrono::{DateTime, FixedOffset, NaiveDate, TimeZone};

use crate::markdown_extensions;

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
    #[must_use]
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

#[must_use]
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

    let Some(start) = title_idx.map(|i| i + 1) else {
        return (title, String::new());
    };

    let quote_lines: Vec<&str> = lines
        .iter()
        .copied()
        .skip(start)
        .skip_while(|line| line.trim().is_empty())
        .map_while(blockquote_text)
        .collect();

    (title, quote_lines.join("\n"))
}

/// Text of a blockquote line, without its `>` marker; `None` for any other line.
fn blockquote_text(line: &str) -> Option<&str> {
    let rest = line.trim().strip_prefix('>')?;
    Some(rest.strip_prefix(' ').unwrap_or(rest))
}

/// Index and text of the first non-blank line at or after `from`.
fn first_non_blank<'a>(lines: &[&'a str], from: usize) -> Option<(usize, &'a str)> {
    lines
        .iter()
        .copied()
        .enumerate()
        .skip(from)
        .find(|(_, line)| !line.trim().is_empty())
}

/// YAML / metadata block without delimiters, paired with markdown body after the closing delimiter.
fn split_frontmatter_raw(markdown: &str, delimiter: &str) -> (String, String) {
    let no_frontmatter = || (String::new(), markdown.to_string());
    let d = delimiter.trim();
    if d.is_empty() {
        return no_frontmatter();
    }
    let lines: Vec<&str> = markdown.lines().collect();
    let Some((first, after_open)) = lines.split_first() else {
        return no_frontmatter();
    };
    if first.trim() != d {
        return no_frontmatter();
    }
    let mut parts = after_open.splitn(2, |line| line.trim() == d);
    match (parts.next(), parts.next()) {
        (Some(inner), Some(rest)) => (inner.join("\n"), rest.join("\n")),
        _ => no_frontmatter(),
    }
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
    let Some((first, first_line)) = first_non_blank(lines, 0) else {
        return (fallback_title.to_string(), None, BodySummaryKind::None);
    };

    let mut h1_idx = None;
    let mut body_title = fallback_title.to_string();
    let mut after_title = first;
    if let Some(rest) = first_line.trim().strip_prefix("# ") {
        if !rest.starts_with('#') {
            body_title = rest.trim().to_string();
            h1_idx = Some(first);
            after_title = first + 1;
        }
    }

    let Some((start, intro_line)) = first_non_blank(lines, after_title) else {
        return (body_title, h1_idx, BodySummaryKind::None);
    };

    if intro_line.trim_start().starts_with('>') {
        let dequoted: Vec<&str> = lines
            .iter()
            .copied()
            .skip(start)
            .map_while(blockquote_text)
            .collect();
        if dequoted.is_empty() {
            return (body_title, h1_idx, BodySummaryKind::None);
        }
        let end = start + dequoted.len() - 1;
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
    let para: Vec<&str> = lines
        .iter()
        .copied()
        .skip(start)
        .take_while(|line| !line.trim().is_empty())
        .collect();
    let joined = para.join("\n");
    if joined.is_empty() || !line_looks_like_tldr_lede(&joined) {
        return (body_title, h1_idx, BodySummaryKind::None);
    }
    let end = start + para.len() - 1;
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
#[must_use]
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

#[must_use]
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
#[must_use]
pub fn render_markdown_fragment_to_html(markdown: &str) -> String {
    render_markdown_to_html(markdown, "")
}

/// If `html` is a single top-level `<p>…</p>` from comrak, returns the inner HTML for embedding.
#[must_use]
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
#[must_use]
pub fn markdown_fragment_to_plain_text(markdown: &str) -> String {
    if markdown.trim().is_empty() {
        return String::new();
    }
    let html = render_markdown_fragment_to_html(markdown);
    strip_html_tags_collapse_ws(&html)
}

pub(crate) fn strip_html_tags_collapse_ws(html: &str) -> String {
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

#[must_use]
pub fn article_description_html_fragment(summary_markdown: &str) -> Option<String> {
    let t = summary_markdown.trim();
    if t.is_empty() {
        return None;
    }
    let html = render_markdown_fragment_to_html(t);
    let inner = unwrap_single_paragraph_html(&html);
    if inner.is_empty() { None } else { Some(inner) }
}

#[must_use]
pub fn render_markdown_to_html(markdown: &str, frontmatter_delimiter: &str) -> String {
    let mut options = comrak::Options::default();
    options.extension.footnotes = true;
    options.extension.strikethrough = true;
    options.extension.table = true;
    options.extension.tasklist = true;

    options.extension.header_id_prefix = Some(String::new());
    options.extension.autolink = true;

    options.extension.math_dollars = true;
    options.extension.math_code = true;

    options.extension.alerts = true;

    options.extension.description_lists = true;

    options.extension.front_matter_delimiter = frontmatter_delimiter_option(frontmatter_delimiter);

    // Posts are written by the blog's owner, so inline HTML is trusted.
    options.render.r#unsafe = true;

    markdown_extensions::render(markdown, &options)
}

#[cfg(test)]
#[path = "markdown_tests.rs"]
mod tests;
