use std::path::Path;

use crate::blog_post::BlogPost;

pub fn markdown_file_to_gemtext(markdown_path: &Path, title: &str) -> std::io::Result<()> {
    let md_content = std::fs::read_to_string(markdown_path)?;
    let mut gemtext = String::new();
    if !title.trim().is_empty() {
        gemtext.push_str("# ");
        gemtext.push_str(title.trim());
        gemtext.push_str("\n\n");
    }
    gemtext.push_str(&markdown_to_gemtext(&md_content));

    let mut output_path = markdown_path.to_path_buf();
    output_path.set_extension("gmi");
    std::fs::write(output_path, gemtext)
}

pub fn markdown_to_gemtext(markdown: &str) -> String {
    let mut output = String::new();
    let mut in_code_block = false;

    for line in markdown.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("```") {
            output.push_str("```");
            output.push('\n');
            in_code_block = !in_code_block;
            continue;
        }

        if in_code_block {
            output.push_str(line);
            output.push('\n');
            continue;
        }

        if trimmed.is_empty() {
            output.push('\n');
            continue;
        }

        if let Some((url, label)) = parse_standalone_link(trimmed) {
            output.push_str("=> ");
            output.push_str(url);
            if !label.is_empty() {
                output.push(' ');
                output.push_str(label);
            }
            output.push('\n');
            continue;
        }

        if trimmed.starts_with("# ") || trimmed.starts_with("## ") || trimmed.starts_with("### ") {
            output.push_str(trimmed);
            output.push('\n');
            continue;
        }

        if let Some(text) = trimmed.strip_prefix("#### ") {
            output.push_str("### ");
            output.push_str(text);
            output.push('\n');
            continue;
        }

        if let Some(text) = trimmed.strip_prefix("##### ") {
            output.push_str("### ");
            output.push_str(text);
            output.push('\n');
            continue;
        }

        if let Some(text) = trimmed.strip_prefix("###### ") {
            output.push_str("### ");
            output.push_str(text);
            output.push('\n');
            continue;
        }

        if let Some(text) = trimmed.strip_prefix("> ") {
            output.push_str("> ");
            output.push_str(&replace_inline_markdown_links(text));
            output.push('\n');
            continue;
        }

        if let Some(text) = parse_unordered_list_item(trimmed) {
            output.push_str("* ");
            output.push_str(&replace_inline_markdown_links(text));
            output.push('\n');
            continue;
        }

        if let Some(text) = parse_ordered_list_item(trimmed) {
            output.push_str("* ");
            output.push_str(&replace_inline_markdown_links(text));
            output.push('\n');
            continue;
        }

        output.push_str(&replace_inline_markdown_links(trimmed));
        output.push('\n');
    }

    output
}

pub fn write_index_gemtext(dest: &Path, blog_posts: &[BlogPost]) -> std::io::Result<()> {
    let mut posts = blog_posts.to_vec();
    posts.sort_by(|a, b| b.last_updated.cmp(&a.last_updated));

    let mut gemtext = String::from("# Blog\n\n");
    for post in posts {
        let relative_path = post
            .path
            .with_extension("gmi")
            .to_string_lossy()
            .to_string();
        let date = post.last_updated.format("%Y-%m-%d");
        gemtext.push_str(&format!("=> /{relative_path} {} ({date})\n", post.title));
    }

    std::fs::write(dest.join("index.gmi"), gemtext)
}

fn parse_unordered_list_item(line: &str) -> Option<&str> {
    for marker in ["- ", "* ", "+ "] {
        if let Some(text) = line.strip_prefix(marker) {
            return Some(text);
        }
    }
    None
}

fn parse_ordered_list_item(line: &str) -> Option<&str> {
    let mut digit_count = 0usize;
    for c in line.chars() {
        if c.is_ascii_digit() {
            digit_count += 1;
            continue;
        }
        break;
    }
    if digit_count == 0 {
        return None;
    }
    let suffix = &line[digit_count..];
    let text = suffix.strip_prefix(". ")?;
    Some(text)
}

fn parse_standalone_link(line: &str) -> Option<(&str, &str)> {
    let label_end = line.find("](")?;
    if !line.starts_with('[') || !line.ends_with(')') {
        return None;
    }
    let label = &line[1..label_end];
    let url_start = label_end + 2;
    let url = &line[url_start..line.len() - 1];
    if url.is_empty() {
        return None;
    }
    Some((url, label))
}

fn replace_inline_markdown_links(line: &str) -> String {
    let mut out = String::new();
    let mut idx = 0usize;
    while idx < line.len() {
        let rem = &line[idx..];
        let Some(open) = rem.find('[') else {
            out.push_str(rem);
            break;
        };
        let open_idx = idx + open;
        out.push_str(&line[idx..open_idx]);
        let after_open = &line[open_idx + 1..];
        let Some(close_bracket_rel) = after_open.find(']') else {
            out.push_str(&line[open_idx..]);
            break;
        };
        let close_bracket_idx = open_idx + 1 + close_bracket_rel;
        let after_bracket = &line[close_bracket_idx + 1..];
        if !after_bracket.starts_with('(') {
            out.push('[');
            idx = open_idx + 1;
            continue;
        }
        let Some(close_paren_rel) = after_bracket.find(')') else {
            out.push_str(&line[open_idx..]);
            break;
        };
        let label = &line[open_idx + 1..close_bracket_idx];
        let url_start = close_bracket_idx + 2;
        let url_end = close_bracket_idx + 1 + close_paren_rel;
        let url = &line[url_start..url_end];
        if !url.is_empty() {
            out.push_str(label);
            out.push_str(" (");
            out.push_str(url);
            out.push(')');
        } else {
            out.push_str(label);
        }
        idx = url_end + 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use std::{io::Write, path::PathBuf};

    use chrono::{DateTime, FixedOffset};
    use tempfile::{NamedTempFile, tempdir};

    use super::{markdown_file_to_gemtext, markdown_to_gemtext, write_index_gemtext};
    use crate::blog_post::BlogPost;

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
        writeln!(f, "## Hello\n\nBody").expect("write markdown");

        markdown_file_to_gemtext(f.path(), "Custom Title").expect("generate gemtext file");

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
}
