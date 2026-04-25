use chrono::{DateTime, FixedOffset, NaiveDate, TimeZone};
use comrak::{Arena, Options, markdown_to_html, nodes::NodeValue, parse_document};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frontmatter {
    pub title: Option<String>,
    pub description: Option<String>,
    pub date: Option<DateTime<FixedOffset>>,
}

impl Frontmatter {
    pub fn is_empty(&self) -> bool {
        self.title.is_none() && self.description.is_none() && self.date.is_none()
    }
}

fn frontmatter_delimiter_option(frontmatter_delimiter: &str) -> Option<String> {
    if frontmatter_delimiter.trim().is_empty() {
        None
    } else {
        Some(frontmatter_delimiter.to_string())
    }
}

fn parse_frontmatter(markdown: &str, frontmatter_delimiter: &str) -> Frontmatter {
    let mut options = Options::default();
    options.extension.front_matter_delimiter = frontmatter_delimiter_option(frontmatter_delimiter);

    let arena = Arena::new();
    let root = parse_document(&arena, markdown, &options);
    for node in root.children() {
        if let NodeValue::FrontMatter(raw) = &node.data.borrow().value {
            return parse_frontmatter_fields(raw);
        }
    }

    Frontmatter {
        title: None,
        description: None,
        date: None,
    }
}

fn parse_frontmatter_fields(raw: &str) -> Frontmatter {
    let mut frontmatter = Frontmatter {
        title: None,
        description: None,
        date: None,
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
                FixedOffset::east_opt(0)?.from_local_datetime(&date.and_hms_opt(0, 0, 0)?).single()
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

pub fn parse_content_metadata(
    markdown: &str,
    fallback_title: &str,
    frontmatter_delimiter: &str,
) -> (String, String, Option<DateTime<FixedOffset>>) {
    let frontmatter = parse_frontmatter(markdown, frontmatter_delimiter);
    let (body_title, body_summary) = parse_title_and_summary(markdown, fallback_title);
    let title = frontmatter.title.unwrap_or(body_title);
    let summary = frontmatter.description.unwrap_or(body_summary);
    (title, summary, frontmatter.date)
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

    use super::{parse_content_metadata, parse_title_and_summary, render_markdown_to_html};

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
        let (title, summary, date) = parse_content_metadata(md, "fallback", "---");
        assert_eq!(title, "Frontmatter Title");
        assert_eq!(summary, "Frontmatter Description");
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
        let (_title, _summary, date) = parse_content_metadata(md, "fallback", "---");
        let date = date.expect("date should be parsed");
        assert_eq!(date.year(), 2026);
        assert_eq!(date.month(), 4);
        assert_eq!(date.day(), 24);
        assert_eq!(date.hour(), 10);
        assert_eq!(date.minute(), 15);
    }

    #[test]
    fn falls_back_to_markdown_when_frontmatter_missing_keys() {
        let md = r#"---
author: me
---
# Body Title

> Body summary
"#;
        let (title, summary, date) = parse_content_metadata(md, "fallback", "---");
        assert_eq!(title, "Body Title");
        assert_eq!(summary, "Body summary");
        assert!(date.is_none());
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
        let (title, summary, date) = parse_content_metadata(md, "fallback", "+++");
        assert_eq!(title, "Custom");
        assert_eq!(summary, "Uses custom delimiter");
        assert!(date.is_none());
    }
}
