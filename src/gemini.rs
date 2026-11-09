use std::path::Path;

use crate::blog_post::{BlogPost, fallback_title};
use crate::markdown::article_render_parts;
use crate::markdown_extensions::{is_markup_only_line, strip_trailing_attr_list};

/// Writes the `.gmi` version of the Markdown file next to it.
///
/// # Errors
/// Fails when the Markdown file cannot be read or the output cannot be written.
pub fn markdown_file_to_gemtext(
    markdown_path: &Path,
    frontmatter_delimiter: &str,
) -> std::io::Result<()> {
    let md_content = std::fs::read_to_string(markdown_path)?;
    let fb = fallback_title(markdown_path);
    let parts = article_render_parts(&md_content, &fb, frontmatter_delimiter);
    let mut gemtext = String::new();
    gemtext.push_str("# ");
    gemtext.push_str(parts.title.trim());
    gemtext.push_str("\n\n");
    gemtext.push_str(&markdown_to_gemtext(&parts.body_markdown));

    let mut output_path = markdown_path.to_path_buf();
    output_path.set_extension("gmi");
    std::fs::write(output_path, gemtext)
}

#[must_use]
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

        if is_markup_only_line(trimmed) {
            continue;
        }
        let trimmed = strip_trailing_attr_list(trimmed);

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

/// Writes `index.gmi` listing `blog_posts` under `dest`.
///
/// # Errors
/// Fails when the file cannot be written.
pub fn write_index_gemtext(dest: &Path, blog_posts: &[BlogPost]) -> std::io::Result<()> {
    let mut posts = blog_posts.to_vec();
    posts.sort_by(|a, b| {
        b.effective_publication_date()
            .cmp(&a.effective_publication_date())
    });

    let mut gemtext = String::from("# Blog\n\n");
    for post in posts {
        let relative_path = post
            .path
            .with_extension("gmi")
            .to_string_lossy()
            .to_string();
        let date = post.effective_publication_date().format("%Y-%m-%d");
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
        if url.is_empty() {
            out.push_str(label);
        } else {
            out.push_str(label);
            out.push_str(" (");
            out.push_str(url);
            out.push(')');
        }
        idx = url_end + 1;
    }
    out
}

#[cfg(test)]
#[path = "gemini_tests.rs"]
mod tests;
