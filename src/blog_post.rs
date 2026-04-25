use std::path::{Path, PathBuf};

use chrono::{DateTime, FixedOffset};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlogPost {
    pub last_updated: DateTime<FixedOffset>,
    pub title: String,
    pub summary: String,
    pub path: PathBuf,
}

impl BlogPost {
    pub fn new(
        path: PathBuf,
        last_updated: DateTime<FixedOffset>,
        title: String,
        summary: String,
    ) -> Self {
        Self {
            last_updated,
            title,
            summary,
            path,
        }
    }

    pub fn with_defaults(path: PathBuf, last_updated: DateTime<FixedOffset>) -> Self {
        Self {
            title: fallback_title(&path),
            summary: String::new(),
            path,
            last_updated,
        }
    }

    pub fn update_from_atom(
        &mut self,
        title: String,
        summary: String,
        last_updated: DateTime<FixedOffset>,
        path: PathBuf,
    ) {
        self.path = path;
        self.last_updated = last_updated;
        self.title = title;
        self.summary = summary;
    }

    pub fn from_atom(
        path: PathBuf,
        title: String,
        summary: String,
        last_updated: DateTime<FixedOffset>,
    ) -> Self {
        Self {
            path,
            title,
            summary,
            last_updated,
        }
    }

    pub fn update_from_markdown(&mut self, markdown: &str) {
        let fallback = fallback_title(&self.path);
        let (title, summary) = parse_title_and_summary(markdown, &fallback);
        self.title = title;
        self.summary = summary;
    }
}

pub fn fallback_title(path: &Path) -> String {
    path.file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("post")
        .to_string()
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

#[cfg(test)]
mod tests {
    use super::parse_title_and_summary;

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
}
