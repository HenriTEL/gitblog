use crate::markdown::render_markdown_to_html;

fn render(markdown: &str) -> String {
    render_markdown_to_html(markdown, "")
}

#[test]
fn inline_html_is_kept() {
    let html = render("Press <kbd>Ctrl</kbd> and <mark>look</mark>.\n");
    assert!(html.contains("<kbd>Ctrl</kbd>"), "{html}");
    assert!(html.contains("<mark>look</mark>"), "{html}");
}

#[test]
fn definition_lists_render() {
    let html = render("Apple\n:   A fruit\n    that grows on trees.\n");
    assert!(html.contains("<dl>"), "{html}");
    assert!(html.contains("<dt>Apple</dt>"), "{html}");
    assert!(html.contains("<dd>"), "{html}");
}

#[test]
fn abbreviations_wrap_whole_words_and_definitions_disappear() {
    let html = render("The HTML spec, not XHTMLS.\n\n*[HTML]: Hyper Text Markup Language\n");
    assert!(
        html.contains(r#"<abbr title="Hyper Text Markup Language">HTML</abbr> spec"#),
        "{html}"
    );
    assert!(html.contains("XHTMLS"), "{html}");
    assert!(!html.contains("*[HTML]"), "{html}");
}

#[test]
fn abbreviations_skip_code_and_image_alt_text() {
    let html = render(
        "`HTML` ![HTML](a.png)\n\n```\n*[HTML]: stays in code\nHTML\n```\n\nHTML\n\n*[HTML]: Markup\n",
    );
    assert!(html.contains("<code>HTML</code>"), "{html}");
    assert!(html.contains(r#"alt="HTML""#), "{html}");
    assert!(html.contains("*[HTML]: stays in code"), "{html}");
    assert_eq!(html.matches("<abbr").count(), 1, "{html}");
}

#[test]
fn toc_lists_nested_headings_with_their_anchors() {
    let html = render("[TOC]\n\n## One\n\n### One *a*\n\n## Two\n");
    assert!(!html.contains("[TOC]"), "{html}");
    assert!(
        html.contains(r#"<span class="toctitle">Table of contents</span>"#),
        "{html}"
    );
    assert!(
        html.contains(r##"<ul><li><a href="#one">One</a><ul><li><a href="#one-a">One a</a></li></ul></li><li><a href="#two">Two</a></li></ul>"##),
        "{html}"
    );
}

#[test]
fn toc_marker_in_code_is_left_alone() {
    let html = render("`[TOC]`\n\n## One\n");
    assert!(html.contains("<code>[TOC]</code>"), "{html}");
    assert!(!html.contains("toctitle"), "{html}");
}

#[test]
fn toc_token_accepts_case_and_whitespace_variations() {
    for token in [
        "[TOC]",
        "[toc]",
        "[Toc]",
        "[tOc]",
        "[ TOC ]",
        "[  toc]",
        "[toc  ]",
        "[\ttoc\t]",
        "   [TOC]",
        "[TOC]   ",
        "  [ ToC ]  ",
    ] {
        let html = render(&format!("{token}\n\n## One\n"));
        assert!(html.contains("toctitle"), "{token:?}: {html}");
        assert!(
            html.contains(r##"<a href="#one">One</a>"##),
            "{token:?}: {html}"
        );
        assert!(!html.to_lowercase().contains("toc]"), "{token:?}: {html}");
        assert!(!html.contains("<p>"), "{token:?}: {html}");
    }
}

#[test]
fn toc_token_on_its_own_line_splits_the_paragraph_around_it() {
    let html = render("Before\n[ toc ]\nAfter\n\n## One\n");
    let before = html.find("<p>Before</p>").expect(&html);
    let table = html.find("toctitle").expect(&html);
    let after = html.find("<p>After</p>").expect(&html);
    assert!(before < table && table < after, "{html}");
}

#[test]
fn toc_token_at_a_paragraph_edge_leaves_no_empty_paragraph() {
    for markdown in ["[TOC]\nAfter\n\n## One\n", "Before\n[TOC]\n\n## One\n"] {
        let html = render(markdown);
        assert!(html.contains("toctitle"), "{html}");
        assert!(!html.contains("<p></p>"), "{html}");
    }
}

#[test]
fn every_toc_token_is_replaced_even_after_a_hard_line_break() {
    let html = render("[TOC]  \nBetween\n[toc]\n\n## One\n");
    assert_eq!(html.matches("toctitle").count(), 2, "{html}");
    assert!(html.contains("<p>Between</p>"), "{html}");
}

#[test]
fn toc_token_works_in_lists_and_quotes() {
    for markdown in ["- [toc]\n\n## One\n", "> [ TOC ]\n\n## One\n"] {
        let html = render(markdown);
        assert!(html.contains("toctitle"), "{html}");
        assert!(!html.to_lowercase().contains("toc]"), "{html}");
    }
}

#[test]
fn toc_token_is_found_when_it_is_also_a_defined_abbreviation() {
    let html = render("[TOC]\n\n*[TOC]: Table of contents\n\n## One\n");
    assert!(html.contains("toctitle"), "{html}");
}

#[test]
fn text_that_is_not_a_lone_toc_token_is_left_alone() {
    for markdown in [
        "See [TOC] for details\n\n## One\n",
        "[TOC].\n\n## One\n",
        "[TOCS]\n\n## One\n",
        "[T O C]\n\n## One\n",
        "    [TOC]\n\n## One\n",
        "```\n[toc]\n```\n\n## One\n",
        "`[ toc ]`\n\n## One\n",
        "## [TOC]\n",
        "[toc]\n\n[toc]: https://example.com\n\n## One\n",
    ] {
        let html = render(markdown);
        assert!(!html.contains("toctitle"), "{markdown:?}: {html}");
    }
}

#[test]
fn heading_attr_list_sets_id_used_by_toc_and_class() {
    let html = render("[TOC]\n\n## Title {: #custom .big data-x=\"1 2\" }\n");
    assert!(html.contains(r#"<h2 data-x="1 2" class="big">"#), "{html}");
    assert!(html.contains(r#"id="custom""#), "{html}");
    assert!(html.contains(r##"<a href="#custom">Title</a>"##), "{html}");
    assert!(!html.contains("{:"), "{html}");
}

#[test]
fn paragraph_attr_list_must_be_on_its_own_line() {
    let html = render("A paragraph\n{: .lead #intro }\n\nInline {: .kept } text.\n");
    assert!(
        html.contains(r#"<p id="intro" class="lead">A paragraph</p>"#),
        "{html}"
    );
    assert!(html.contains("Inline {: .kept } text."), "{html}");
}

#[test]
fn inline_attr_lists_apply_to_the_preceding_element() {
    let html = render(
        "*em*{: .a } **b**{: .c } [l](/u){: rel=\"me\" } ![i](/p.png){: .wide } `c`{: .d } tail\n",
    );
    assert!(html.contains(r#"<em class="a">em</em>"#), "{html}");
    assert!(html.contains(r#"<strong class="c">b</strong>"#), "{html}");
    assert!(html.contains(r#"<a href="/u" rel="me">l</a>"#), "{html}");
    assert!(
        html.contains(r#"<img src="/p.png" alt="i" class="wide" />"#),
        "{html}"
    );
    assert!(html.contains(r#"<code class="d">c</code> tail"#), "{html}");
}

#[test]
fn list_item_attr_list_applies_to_the_item() {
    let html = render("- one {: .x }\n- two\n");
    assert!(html.contains(r#"<li class="x">one</li>"#), "{html}");
}

#[test]
fn attr_lists_in_code_are_left_alone() {
    let html = render("```\n## T {: #x }\n```\n\n`a {: .b }`\n");
    assert!(html.contains("## T {: #x }"), "{html}");
    assert!(html.contains("<code>a {: .b }</code>"), "{html}");
}

#[test]
fn braces_without_attr_syntax_are_plain_text() {
    let html = render("## Using {name} in templates\n");
    assert!(html.contains("Using {name} in templates"), "{html}");
}

#[test]
fn markdown_inside_html_blocks_is_rendered() {
    let html = render("<div class=\"note\" markdown=\"1\">\n*hello*\n\n- item\n</div>\n\nAfter\n");
    assert!(html.contains("<div class=\"note\">"), "{html}");
    assert!(html.contains("<em>hello</em>"), "{html}");
    assert!(html.contains("<li>item</li>"), "{html}");
    assert!(!html.contains("markdown="), "{html}");
    assert!(html.contains("<p>After</p>"), "{html}");
}

#[test]
fn markdown_inside_span_level_html_stays_inline() {
    let html = render("<p markdown=\"1\">*hello*</p>\n");
    assert!(html.contains("<p><em>hello</em></p>"), "{html}");
}

#[test]
fn markdown_html_blocks_handle_nesting() {
    let html = render("<div markdown=\"block\">\n<div>inner</div>\n\n*outer*\n</div>\n");
    assert!(html.contains("<div>inner</div>"), "{html}");
    assert!(html.contains("<em>outer</em>"), "{html}");
    assert!(html.trim_end().ends_with("</div>"), "{html}");
}

#[test]
fn markdown_html_without_closing_tag_is_left_alone() {
    let html = render("<div markdown=\"1\">\nnever closed\n");
    assert!(html.contains("markdown=\"1\""), "{html}");
}

#[test]
fn html_without_markdown_attribute_stays_raw() {
    let html = render("<div>\n*not emphasis*\n</div>\n");
    assert!(html.contains("*not emphasis*"), "{html}");
}
