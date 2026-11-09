//! Python-Markdown syntax that comrak does not support: abbreviations, attribute lists, Markdown
//! inside HTML blocks and the `[TOC]` marker.
//!
//! Anything that must survive comrak's rendering travels through it as an HTML comment placeholder,
//! which comrak leaves out of heading slugs.

use std::collections::HashMap;
use std::sync::LazyLock;

use comrak::nodes::{AstNode, NodeValue};
use comrak::{Arena, Options, format_html, parse_document};
use regex::Regex;

use crate::markdown::{
    render_markdown_to_html, strip_html_tags_collapse_ws, unwrap_single_paragraph_html,
};

const TOC_MARKER: &str = "<p>[TOC]</p>";
const TOC_TITLE: &str = "Table of contents";
const ATTR_PLACEHOLDER: &str = "<!--gitblog-attr-";
const ATTR_BODY: &str = r"\{(?::[ ]*([^{}\n]+?)|[ ]*([#.][^{}\n]*?))[ ]*\}";
const BLOCK_TAGS: &[&str] = &["p", "h1", "h2", "h3", "h4", "h5", "h6", "li", "dd", "dt"];
const INLINE_TAGS: &[&str] = &["em", "strong", "code", "del", "a"];
/// HTML elements whose `markdown="1"` content is inline rather than block-level.
const SPAN_LEVEL_TAGS: &[&str] = &[
    "address", "dd", "dt", "h1", "h2", "h3", "h4", "h5", "h6", "legend", "li", "p", "summary",
    "td", "th",
];

static ABBREVIATION_LINE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^ {0,3}\*\[([^\]\n]+)\]:[ \t]*(.*?)[ \t]*$").expect("abbr regex")
});
static ATTR_WHOLE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(&format!(r"^[ \t]*{ATTR_BODY}[ \t]*$")).expect("attr regex"));
static ATTR_TRAILING: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(&format!(r"[ \t]+{ATTR_BODY}[ \t]*$")).expect("attr regex"));
static ATTR_LEADING: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(&format!("^{ATTR_BODY}")).expect("attr regex"));
static MARKDOWN_HTML_OPEN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?i)^ {0,3}<([a-z][a-z0-9-]*)([^<>]*?)\s+markdown\s*=\s*(?:"(1|block|span)"|'(1|block|span)'|(1|block|span))([^<>]*)>"#,
    )
    .expect("markdown html regex")
});
static HEADING_WITH_ANCHOR: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r##"(?s)<h([1-6])\b[^>]*><a href="#[^"]*"[^>]*\bid="([^"]*)"[^>]*></a>(.*?)</h[1-6]>"##,
    )
    .expect("heading regex")
});

/// Renders `markdown` to HTML with comrak plus the extensions of this module.
pub(crate) fn render(markdown: &str, options: &Options) -> String {
    let (markdown, abbreviations) = extract_abbreviations(markdown);
    let (markdown, html_blocks) = extract_markdown_html_blocks(&markdown);

    let arena = Arena::new();
    let root = parse_document(&arena, &markdown, options);
    let attr_lists = mark_attr_lists(&arena, root);
    if let Some(abbreviations) = &abbreviations {
        apply_abbreviations(&arena, root, abbreviations);
    }

    let mut html = String::new();
    format_html(root, options, &mut html).expect("writing to a String cannot fail");

    let html = apply_attr_lists(html, &attr_lists);
    let html = restore_markdown_html_blocks(html, &html_blocks);
    insert_table_of_contents(html)
}

/// Whether a source line only carries markup of this module, with no content of its own.
pub(crate) fn is_markup_only_line(line: &str) -> bool {
    line == "[TOC]" || ABBREVIATION_LINE.is_match(line) || ATTR_WHOLE.is_match(line)
}

/// `line` without a trailing attribute list.
pub(crate) fn strip_trailing_attr_list(line: &str) -> &str {
    ATTR_TRAILING
        .find(line)
        .map_or(line, |m| &line[..m.start()])
}

fn escape_html(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Tells which lines of a document belong to fenced code blocks.
#[derive(Default)]
struct FenceTracker {
    open: Option<(char, usize)>,
}

impl FenceTracker {
    fn in_fence(&mut self, line: &str) -> bool {
        let trimmed = line.trim_start();
        let indent = line.len() - trimmed.len();
        let marker = trimmed.chars().next().filter(|c| matches!(c, '`' | '~'));
        let run = marker.map_or(0, |m| trimmed.chars().take_while(|c| *c == m).count());
        if let Some((open_marker, open_run)) = self.open {
            if marker == Some(open_marker) && run >= open_run && trimmed[run..].trim().is_empty() {
                self.open = None;
            }
            return true;
        }
        if let Some(marker) = marker.filter(|_| indent <= 3 && run >= 3) {
            self.open = Some((marker, run));
            return true;
        }
        false
    }
}

struct Abbreviations {
    matcher: Regex,
    titles: HashMap<String, String>,
}

fn extract_abbreviations(markdown: &str) -> (String, Option<Abbreviations>) {
    let mut kept = String::with_capacity(markdown.len());
    let mut titles = HashMap::new();
    let mut fences = FenceTracker::default();
    for line in markdown.split_inclusive('\n') {
        let definition = (!fences.in_fence(line))
            .then(|| ABBREVIATION_LINE.captures(line.trim_end_matches(['\r', '\n'])))
            .flatten();
        match definition {
            Some(captures) => {
                titles.insert(captures[1].to_string(), captures[2].to_string());
            }
            None => kept.push_str(line),
        }
    }
    if titles.is_empty() {
        return (kept, None);
    }
    let mut terms: Vec<_> = titles.keys().map(|term| regex::escape(term)).collect();
    terms.sort_by_key(|term| std::cmp::Reverse(term.len()));
    let matcher = Regex::new(&format!(r"\b(?:{})\b", terms.join("|"))).expect("abbreviation regex");
    (kept, Some(Abbreviations { matcher, titles }))
}

fn new_node<'a>(arena: &'a Arena<'a>, value: NodeValue) -> &'a AstNode<'a> {
    arena.alloc(value.into())
}

fn node_text(node: &AstNode<'_>) -> Option<String> {
    let data = node.data.borrow();
    if let NodeValue::Text(text) = &data.value {
        Some(text.to_string())
    } else {
        None
    }
}

fn set_text_or_detach(node: &AstNode<'_>, text: String) {
    if text.is_empty() {
        node.detach();
    } else {
        node.data.borrow_mut().value = NodeValue::Text(text.into());
    }
}

fn apply_abbreviations<'a>(
    arena: &'a Arena<'a>,
    root: &'a AstNode<'a>,
    abbreviations: &Abbreviations,
) {
    let in_image = |node: &'a AstNode<'a>| {
        node.ancestors()
            .any(|a| matches!(a.data.borrow().value, NodeValue::Image(_)))
    };
    let text_nodes: Vec<_> = root.descendants().filter(|n| !in_image(n)).collect();
    for node in text_nodes {
        let Some(text) = node_text(node) else {
            continue;
        };
        let mut last = 0;
        for found in abbreviations.matcher.find_iter(&text) {
            let title = abbreviations
                .titles
                .get(found.as_str())
                .map_or_else(String::new, |title| escape_html(title));
            let before = text[last..found.start()].to_string();
            for value in [
                NodeValue::Text(before.into()),
                NodeValue::HtmlInline(format!("<abbr title=\"{title}\">")),
                NodeValue::Text(found.as_str().to_string().into()),
                NodeValue::HtmlInline("</abbr>".to_string()),
            ] {
                node.insert_before(new_node(arena, value));
            }
            last = found.end();
        }
        if last > 0 {
            set_text_or_detach(node, text[last..].to_string());
        }
    }
}

/// `name`/`value` pairs from an attribute list, in source order.
type Attributes = Vec<(String, String)>;

fn parse_attributes(source: &str) -> Attributes {
    let mut attributes: Attributes = Vec::new();
    let mut classes: Vec<String> = Vec::new();
    let mut rest = source.trim();
    while !rest.is_empty() {
        let (token, value, remaining) = next_attribute_token(rest);
        rest = remaining.trim_start();
        match (token.strip_prefix('#'), token.strip_prefix('.'), value) {
            (Some(id), _, None) => attributes.push(("id".to_string(), id.to_string())),
            (_, Some(class), None) => classes.push(class.to_string()),
            (_, _, Some(value)) if token == "class" => classes.push(value),
            (_, _, Some(value)) => attributes.push((token.to_string(), value)),
            (_, _, None) => attributes.push((token.to_string(), token.to_string())),
        }
    }
    if !classes.is_empty() {
        attributes.push(("class".to_string(), classes.join(" ")));
    }
    attributes
}

/// Splits the first `word` or `name=value` token off `source`, honouring quoted values.
fn next_attribute_token(source: &str) -> (&str, Option<String>, &str) {
    let name_end = source
        .find(|c: char| c == '=' || c.is_whitespace())
        .unwrap_or(source.len());
    let (name, rest) = source.split_at(name_end);
    let Some(value_source) = rest.strip_prefix('=') else {
        return (name, None, rest);
    };
    if let Some(quote) = value_source
        .chars()
        .next()
        .filter(|c| matches!(c, '"' | '\''))
    {
        let quoted = &value_source[1..];
        let end = quoted.find(quote).unwrap_or(quoted.len());
        let remaining = quoted.get(end + 1..).unwrap_or("");
        return (name, Some(quoted[..end].to_string()), remaining);
    }
    let end = value_source
        .find(char::is_whitespace)
        .unwrap_or(value_source.len());
    (
        name,
        Some(value_source[..end].to_string()),
        &value_source[end..],
    )
}

fn captured_attributes(captures: &regex::Captures<'_>) -> Attributes {
    let source = captures.get(1).or_else(|| captures.get(2));
    parse_attributes(source.map_or("", |m| m.as_str()))
}

/// Replaces attribute lists in the tree with placeholders and returns what each one holds.
fn mark_attr_lists<'a>(arena: &'a Arena<'a>, root: &'a AstNode<'a>) -> Vec<Attributes> {
    let mut lists = Vec::new();
    let nodes: Vec<_> = root.descendants().collect();
    for node in nodes {
        let is_block = {
            let value = &node.data.borrow().value;
            matches!(value, NodeValue::Paragraph | NodeValue::Heading(_))
        };
        if is_block {
            mark_block_attr_list(arena, node, &mut lists);
        } else {
            mark_inline_attr_list(arena, node, &mut lists);
        }
    }
    lists
}

fn placeholder_node<'a>(arena: &'a Arena<'a>, index: usize) -> &'a AstNode<'a> {
    new_node(
        arena,
        NodeValue::HtmlInline(format!("{ATTR_PLACEHOLDER}{index}-->")),
    )
}

fn allows_same_line_attr_list(node: &AstNode<'_>) -> bool {
    if matches!(node.data.borrow().value, NodeValue::Heading(_)) {
        return true;
    }
    node.parent().is_some_and(|parent| {
        matches!(
            parent.data.borrow().value,
            NodeValue::Item(_)
                | NodeValue::TaskItem(_)
                | NodeValue::DescriptionTerm
                | NodeValue::DescriptionDetails
        )
    })
}

fn mark_block_attr_list<'a>(
    arena: &'a Arena<'a>,
    block: &'a AstNode<'a>,
    lists: &mut Vec<Attributes>,
) {
    let Some(last) = block.last_child() else {
        return;
    };
    let Some(text) = node_text(last) else {
        return;
    };
    let previous_is_break = last.previous_sibling().filter(|previous| {
        matches!(
            previous.data.borrow().value,
            NodeValue::SoftBreak | NodeValue::LineBreak
        )
    });
    if let (Some(line_break), Some(captures)) = (previous_is_break, ATTR_WHOLE.captures(&text)) {
        lists.push(captured_attributes(&captures));
        last.insert_before(placeholder_node(arena, lists.len() - 1));
        line_break.detach();
        last.detach();
    } else if allows_same_line_attr_list(block) {
        if let Some(captures) = ATTR_TRAILING.captures(&text) {
            let start = captures.get(0).map_or(text.len(), |m| m.start());
            lists.push(captured_attributes(&captures));
            last.insert_after(placeholder_node(arena, lists.len() - 1));
            set_text_or_detach(last, text[..start].to_string());
        }
    }
}

fn mark_inline_attr_list<'a>(
    arena: &'a Arena<'a>,
    node: &'a AstNode<'a>,
    lists: &mut Vec<Attributes>,
) {
    let follows_inline_element = node.previous_sibling().is_some_and(|previous| {
        matches!(
            previous.data.borrow().value,
            NodeValue::Emph
                | NodeValue::Strong
                | NodeValue::Strikethrough
                | NodeValue::Code(_)
                | NodeValue::Link(_)
                | NodeValue::Image(_)
        )
    });
    if !follows_inline_element {
        return;
    }
    let Some(text) = node_text(node) else {
        return;
    };
    let Some(captures) = ATTR_LEADING.captures(&text) else {
        return;
    };
    let end = captures.get(0).map_or(0, |m| m.end());
    lists.push(captured_attributes(&captures));
    node.insert_before(placeholder_node(arena, lists.len() - 1));
    set_text_or_detach(node, text[end..].to_string());
}

/// Byte range of the `<tag …>` opening nearest the end of `html`, excluding its final `>` (or `/>`).
fn last_open_tag(html: &str, tag: &str) -> Option<(usize, usize)> {
    let needle = format!("<{tag}");
    let mut search_end = html.len();
    while let Some(start) = html[..search_end].rfind(&needle) {
        let after = html[start + needle.len()..].chars().next();
        if matches!(after, Some('>' | ' ' | '/' | '\n')) {
            let close = start + html[start..].find('>')?;
            let end = if html[..close].ends_with('/') {
                close - 1
            } else {
                close
            };
            return Some((start, end.min(html[..end].trim_end().len())));
        }
        search_end = start;
    }
    None
}

/// The element that the attribute list located between `before` and `after` applies to.
fn attr_list_target(before: &str, after: &str) -> Option<(usize, usize, bool)> {
    let following = after.trim_start();
    for tag in BLOCK_TAGS {
        if following.starts_with(&format!("</{tag}>")) {
            let (start, end) = last_open_tag(before, tag)?;
            return Some((start, end, tag.starts_with('h')));
        }
    }
    for tag in INLINE_TAGS {
        if before.ends_with(&format!("</{tag}>")) {
            let (start, end) = last_open_tag(before, tag)?;
            return Some((start, end, false));
        }
    }
    let (start, end) = last_open_tag(before, "img")?;
    before[start..]
        .find('>')
        .is_some_and(|close| start + close + 1 == before.len())
        .then_some((start, end, false))
}

fn apply_attr_lists(mut html: String, lists: &[Attributes]) -> String {
    while let Some(start) = html.rfind(ATTR_PLACEHOLDER) {
        let digits_start = start + ATTR_PLACEHOLDER.len();
        let Some(length) = html[digits_start..].find("-->") else {
            break;
        };
        let end = digits_start + length + 3;
        let list = html[digits_start..digits_start + length]
            .parse::<usize>()
            .ok()
            .and_then(|index| lists.get(index));
        let target = attr_list_target(&html[..start], &html[end..]);
        html.replace_range(start..end, "");
        if let (Some((tag_start, tag_end, is_heading)), Some(list)) = (target, list) {
            add_attributes(&mut html, tag_start, tag_end, is_heading, list);
        }
    }
    html
}

fn add_attributes(
    html: &mut String,
    tag_start: usize,
    tag_end: usize,
    is_heading: bool,
    attributes: &Attributes,
) {
    const CLASS_PREFIX: &str = " class=\"";
    let mut opening = html[tag_start..tag_end].to_string();
    for (name, value) in attributes {
        if is_heading && name == "id" {
            set_heading_anchor(html, tag_end, value);
            continue;
        }
        let value = escape_html(value);
        match opening.find(CLASS_PREFIX).filter(|_| name == "class") {
            Some(position) => {
                opening.insert_str(position + CLASS_PREFIX.len(), &format!("{value} "));
            }
            None => opening.push_str(&format!(" {name}=\"{value}\"")),
        }
    }
    html.replace_range(tag_start..tag_end, &opening);
}

/// Comrak puts a heading's id on its anchor, so a custom id has to replace it there.
fn set_heading_anchor(html: &mut String, heading_tag_end: usize, id: &str) {
    let Some(anchor_start) = html[heading_tag_end..]
        .find("<a href=\"#")
        .map(|i| heading_tag_end + i)
    else {
        return;
    };
    let Some(anchor_end) = html[anchor_start..].find("</a>").map(|i| anchor_start + i) else {
        return;
    };
    let id = escape_html(id);
    html.replace_range(
        anchor_start..anchor_end,
        &format!("<a href=\"#{id}\" aria-hidden=\"true\" class=\"anchor\" id=\"{id}\">"),
    );
}

fn markdown_html_placeholder(index: usize) -> String {
    format!("<!--gitblog-html-{index}-->")
}

/// Pulls out HTML elements carrying a `markdown` attribute, with their content already rendered.
fn extract_markdown_html_blocks(markdown: &str) -> (String, Vec<String>) {
    let mut kept = String::with_capacity(markdown.len());
    let mut blocks = Vec::new();
    let mut fences = FenceTracker::default();
    let mut position = 0;
    while position < markdown.len() {
        let line_end = markdown[position..]
            .find('\n')
            .map_or(markdown.len(), |i| position + i + 1);
        let line = &markdown[position..line_end];
        if !fences.in_fence(line) {
            if let Some((block, next)) = render_markdown_html_block(markdown, position, line) {
                kept.push_str(&format!("\n{}\n", markdown_html_placeholder(blocks.len())));
                blocks.push(block);
                position = next;
                continue;
            }
        }
        kept.push_str(line);
        position = line_end;
    }
    (kept, blocks)
}

/// The rendered element opening on `line`, and the offset just past its closing tag.
fn render_markdown_html_block(
    markdown: &str,
    line_start: usize,
    line: &str,
) -> Option<(String, usize)> {
    let captures = MARKDOWN_HTML_OPEN.captures(line)?;
    let tag = captures.get(1)?.as_str();
    let mode = captures
        .get(3)
        .or_else(|| captures.get(4))
        .or_else(|| captures.get(5))?
        .as_str();
    let content_start = line_start + captures.get(0)?.end();
    let (content_end, next) = find_closing_tag(&markdown[content_start..], tag)?;
    let content = &markdown[content_start..content_start + content_end];

    let is_span = mode == "span"
        || (mode == "1" && SPAN_LEVEL_TAGS.iter().any(|t| t.eq_ignore_ascii_case(tag)));
    let rendered = render_markdown_to_html(content.trim(), "");
    let rendered = if is_span {
        unwrap_single_paragraph_html(&rendered)
    } else {
        format!("\n{rendered}")
    };
    let before = captures.get(2).map_or("", |m| m.as_str());
    let after = captures.get(6).map_or("", |m| m.as_str()).trim_end();
    Some((
        format!("<{tag}{before}{after}>{rendered}</{tag}>"),
        content_start + next,
    ))
}

/// Offsets of the `</tag>` matching an element whose content starts at the beginning of `source`:
/// where that closing tag starts, and where it ends.
fn find_closing_tag(source: &str, tag: &str) -> Option<(usize, usize)> {
    let pattern = Regex::new(&format!(r"(?i)<(/?){}\b[^>]*>", regex::escape(tag))).ok()?;
    let mut depth = 1usize;
    for found in pattern.captures_iter(source) {
        let whole = found.get(0)?;
        if found.get(1).is_some_and(|slash| !slash.as_str().is_empty()) {
            depth -= 1;
            if depth == 0 {
                return Some((whole.start(), whole.end()));
            }
        } else if !whole.as_str().ends_with("/>") {
            depth += 1;
        }
    }
    None
}

fn restore_markdown_html_blocks(mut html: String, blocks: &[String]) -> String {
    for (index, block) in blocks.iter().enumerate() {
        html = html.replace(&markdown_html_placeholder(index), block);
    }
    html
}

fn insert_table_of_contents(html: String) -> String {
    if !html.contains(TOC_MARKER) {
        return html;
    }
    let mut list = String::new();
    let mut open_levels: Vec<u8> = Vec::new();
    for heading in HEADING_WITH_ANCHOR.captures_iter(&html) {
        let level = heading[1].parse::<u8>().unwrap_or(1);
        while open_levels.last().is_some_and(|open| *open > level) {
            list.push_str("</li></ul>");
            open_levels.pop();
        }
        if open_levels.last() == Some(&level) {
            list.push_str("</li>");
        } else {
            list.push_str("<ul>");
            open_levels.push(level);
        }
        list.push_str(&format!(
            "<li><a href=\"#{}\">{}</a>",
            &heading[2],
            strip_html_tags_collapse_ws(&heading[3])
        ));
    }
    list.push_str(&"</li></ul>".repeat(open_levels.len()));
    let table =
        format!("<div class=\"toc\"><span class=\"toctitle\">{TOC_TITLE}</span>{list}</div>");
    html.replace(TOC_MARKER, &table)
}

#[cfg(test)]
#[path = "markdown_extensions_tests.rs"]
mod tests;
