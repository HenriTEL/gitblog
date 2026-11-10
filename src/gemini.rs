use std::ops::Range;
use std::path::Path;
use std::sync::LazyLock;

use regex::Regex;

use crate::blog_post::{BlogPost, fallback_title};
use crate::markdown::{alert_title, article_render_parts};
use crate::markdown_extensions::{is_markup_only_line, strip_trailing_attr_list};

/// Spans whose content is literal (escapes, code, link targets, URLs, HTML tags and math), which
/// are matched only to skip them, then the runs of `*` or `_` that may mark emphasis.
static EMPHASIS_TOKEN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(concat!(
        r"(?s)\\.",
        r"|```.+?```|``.+?``|`[^`]+`",
        r"|\]\([^\s)]*",
        r"|[a-zA-Z][a-zA-Z0-9+.-]*://[^\s<>]*[^\s<>*_.,;:!?)]",
        r"|</?[a-zA-Z!][^<>]*>",
        r"|\$\$.+?\$\$|\$[^\s$](?:[^$]*[^\s$])?\$",
        r"|(?P<run>\*+|_+)",
    ))
    .expect("emphasis regex")
});
/// Lines that open a block of their own, even with no blank line before them.
static BLOCK_START: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*(?:#{1,6}\s|[-+*]\s|\d+[.)]\s|>|\|)").expect("block regex"));

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
    gemtext.push_str(&strip_inline_emphasis(parts.title.trim()));
    gemtext.push_str("\n\n");
    gemtext.push_str(&markdown_to_gemtext(&parts.body_markdown));

    let mut output_path = markdown_path.to_path_buf();
    output_path.set_extension("gmi");
    std::fs::write(output_path, gemtext)
}

#[must_use]
pub fn markdown_to_gemtext(markdown: &str) -> String {
    let markdown = strip_emphasis(markdown);
    let mut output = String::new();
    let mut in_code_block = false;
    let mut previous_line = "";

    for line in markdown.lines() {
        let trimmed = line.trim();
        let continues_quote = previous_line.trim_start().starts_with('>');
        previous_line = line;
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

        if let Some(title) = alert_title(trimmed).filter(|_| !continues_quote) {
            output.push_str("> ");
            output.push_str(&title);
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
        let title = strip_inline_emphasis(&post.title);
        gemtext.push_str(&format!("=> /{relative_path} {title} ({date})\n"));
    }

    std::fs::write(dest.join("index.gmi"), gemtext)
}

/// Removes bold and italic markers from `markdown`, leaving code blocks untouched. Emphasis can
/// span the lines of a paragraph, so each block is processed as a whole.
fn strip_emphasis(markdown: &str) -> String {
    fn flush(block: &mut String, output: &mut String) {
        output.push_str(&strip_inline_emphasis(block));
        block.clear();
    }

    let mut output = String::with_capacity(markdown.len());
    let mut block = String::new();
    let mut inside_fence = false;
    let mut in_indented_code = false;

    for line in markdown.lines() {
        let trimmed = line.trim();
        let is_fence = trimmed.starts_with("```");
        inside_fence ^= is_fence;
        in_indented_code = !trimmed.is_empty()
            && (in_indented_code
                || (block.is_empty() && (line.starts_with("    ") || line.starts_with('\t'))));

        let is_verbatim = is_fence
            || inside_fence
            || in_indented_code
            || trimmed.is_empty()
            || is_markup_only_line(trimmed);
        if is_verbatim || BLOCK_START.is_match(line) {
            flush(&mut block, &mut output);
        }
        let target = if is_verbatim { &mut output } else { &mut block };
        target.push_str(line);
        target.push('\n');
        if trimmed.starts_with('#') {
            flush(&mut block, &mut output);
        }
    }
    flush(&mut block, &mut output);
    output
}

/// `text` without the delimiter runs that open and close emphasis, paired as `CommonMark` does.
fn strip_inline_emphasis(text: &str) -> String {
    let mut open_runs: Vec<(char, Range<usize>)> = Vec::new();
    let mut removed: Vec<Range<usize>> = Vec::new();

    for token in EMPHASIS_TOKEN.captures_iter(text) {
        let Some(run) = token.name("run") else {
            continue;
        };
        let marker = if run.as_str().starts_with('*') {
            '*'
        } else {
            '_'
        };
        let before = text[..run.start()].chars().next_back();
        let after = text[run.end()..].chars().next();
        let (can_open, can_close) = run_can_open_and_close(marker, before, after);

        let mut rest = run.range();
        while can_close && !rest.is_empty() {
            let Some(index) = open_runs.iter().rposition(|(m, _)| *m == marker) else {
                break;
            };
            open_runs.truncate(index + 1);
            let Some((_, opener)) = open_runs.last_mut() else {
                break;
            };
            let count = opener.len().min(rest.len());
            removed.push(opener.end - count..opener.end);
            removed.push(rest.start..rest.start + count);
            opener.end -= count;
            rest.start += count;
            if opener.start == opener.end {
                open_runs.pop();
            }
        }
        if can_open && !rest.is_empty() {
            open_runs.push((marker, rest));
        }
    }

    removed.sort_by_key(|range| range.start);
    let mut output = String::with_capacity(text.len());
    let mut copied = 0;
    for range in removed {
        output.push_str(&text[copied..range.start]);
        copied = range.end;
    }
    output.push_str(&text[copied..]);
    output
}

/// `CommonMark`'s flanking rules: whether a delimiter run can open and can close emphasis.
fn run_can_open_and_close(marker: char, before: Option<char>, after: Option<char>) -> (bool, bool) {
    let is_space = |c: Option<char>| c.is_none_or(char::is_whitespace);
    let is_punctuation =
        |c: Option<char>| c.is_some_and(|c| !c.is_alphanumeric() && !c.is_whitespace());

    let left_flanking =
        !is_space(after) && (!is_punctuation(after) || is_space(before) || is_punctuation(before));
    let right_flanking =
        !is_space(before) && (!is_punctuation(before) || is_space(after) || is_punctuation(after));

    if marker == '*' {
        (left_flanking, right_flanking)
    } else {
        (
            left_flanking && (!right_flanking || is_punctuation(before)),
            right_flanking && (!left_flanking || is_punctuation(after)),
        )
    }
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
