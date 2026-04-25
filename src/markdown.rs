use comrak::{Options, markdown_to_html};

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

pub fn render_markdown_to_html(markdown: &str) -> String {
    let mut options = Options::default();
    options.extension.footnotes = true;
    options.extension.strikethrough = true;
    options.extension.table = true;
    options.extension.tasklist = true;
    options.extension.alerts = true;

    markdown_to_html(markdown, &options)
}

#[cfg(test)]
mod tests {
    use super::{parse_title_and_summary, render_markdown_to_html};

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
        let html = render_markdown_to_html(md);
        assert!(html.contains("<table>"));
        assert!(html.contains("type=\"checkbox\""));
    }
}
