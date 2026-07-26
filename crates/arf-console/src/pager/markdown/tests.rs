use super::*;
use ratatui::style::Color;

/// Helper: render markdown and collect line text (without styling).
fn render_plain(input: &str) -> Vec<String> {
    render_markdown(input, None, None)
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>()
        })
        .collect()
}

#[test]
fn plain_text() {
    let lines = render_plain("Hello world");
    assert_eq!(lines, vec!["Hello world"]);
}

#[test]
fn heading_prefix() {
    let lines = render_plain("# Title\n\nBody");
    assert_eq!(lines, vec!["# Title", "", "Body"]);
}

#[test]
fn heading_levels() {
    let lines = render_plain("## Sub\n\n### Third");
    assert_eq!(lines, vec!["## Sub", "", "### Third"]);
}

#[test]
fn emphasis_and_strong() {
    let lines = render_markdown("*em* **strong**", None, None);
    assert_eq!(lines.len(), 1);
    // Check that there are separate spans with appropriate styles
    let spans = &lines[0].spans;
    assert!(spans.len() >= 2);
}

#[test]
fn inline_code() {
    let lines = render_markdown("Use `print()`", None, None);
    assert_eq!(lines.len(), 1);
    let text: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
    assert_eq!(text, "Use print()");
}

#[test]
fn code_block() {
    let input = "```r\nx <- 1\ny <- 2\n```";
    let lines = render_plain(input);
    assert!(lines.contains(&"x <- 1".to_string()));
    assert!(lines.contains(&"y <- 2".to_string()));
}

#[test]
fn unordered_list() {
    let input = "- one\n- two\n- three";
    let lines = render_plain(input);
    assert_eq!(lines, vec!["- one", "- two", "- three"]);
}

#[test]
fn ordered_list() {
    let input = "1. first\n2. second\n3. third";
    let lines = render_plain(input);
    assert_eq!(lines, vec!["1. first", "2. second", "3. third"]);
}

#[test]
fn nested_list() {
    let input = "- outer\n  - inner";
    let lines = render_plain(input);
    assert_eq!(lines, vec!["- outer", "  - inner"]);
}

#[test]
fn blockquote() {
    let input = "> quoted text";
    let lines = render_plain(input);
    assert_eq!(lines, vec!["> quoted text"]);
}

#[test]
fn simple_table() {
    let input = "| A | B |\n|---|---|\n| 1 | 2 |";
    let lines = render_plain(input);
    // Should have header, separator, data row
    assert_eq!(lines.len(), 3);
    assert!(lines[0].contains("A"));
    assert!(lines[0].contains("B"));
    assert!(lines[1].contains("-"));
    assert!(lines[2].contains("1"));
    assert!(lines[2].contains("2"));
}

#[test]
fn link_rendering() {
    let input = "[click here](https://example.com)";
    let lines = render_plain(input);
    assert_eq!(lines.len(), 1);
    assert!(lines[0].contains("click here"));
    assert!(lines[0].contains("https://example.com"));
}

#[test]
fn horizontal_rule() {
    let input = "before\n\n---\n\nafter";
    let lines = render_plain(input);
    assert!(lines.contains(&"———".to_string()));
}

#[test]
fn table_with_br_tags() {
    let input = "| Arg | Desc |\n|---|---|\n| x | first<br>second |";
    let lines = render_plain(input);
    // The cell with <br> should produce two visual rows
    assert!(lines.len() >= 4); // header + separator + 2 data lines
    // First data row has "first"
    assert!(lines[2].contains("first"));
    // Second data row has "second"
    assert!(lines[3].contains("second"));
}

#[test]
fn inline_html_br_outside_table() {
    let input = "line one<br>line two";
    let lines = render_plain(input);
    assert_eq!(lines, vec!["line one", "line two"]);
}

#[test]
fn r_code_block_syntax_highlight() {
    let input = "```r\nx <- 42\n```";
    let lines = render_markdown(input, None, None);
    // Should produce one line: "x <- 42"
    let text: String = lines
        .iter()
        .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
        .collect();
    assert!(text.contains("x <- 42"));

    // Find the line containing "x <- 42"
    let code_line = lines.iter().find(|l| {
        let t: String = l.spans.iter().map(|s| s.content.as_ref()).collect();
        t.contains("<-")
    });
    assert!(code_line.is_some(), "Should have a line with <-");
    let code_line = code_line.unwrap();

    // With syntax highlighting, should have multiple spans (not a single dim span)
    assert!(
        code_line.spans.len() >= 3,
        "R code should be tokenized into multiple spans, got {}",
        code_line.spans.len()
    );

    // The operator "<-" should have Yellow foreground
    let op_span = code_line
        .spans
        .iter()
        .find(|s| s.content.as_ref().contains("<-"));
    assert!(op_span.is_some(), "Should have an <- operator span");
    assert_eq!(
        op_span.unwrap().style.fg,
        Some(Color::Yellow),
        "Operator <- should be Yellow"
    );

    // The number "42" should have LightMagenta foreground
    let num_span = code_line.spans.iter().find(|s| s.content.as_ref() == "42");
    assert!(num_span.is_some(), "Should have a 42 number span");
    assert_eq!(
        num_span.unwrap().style.fg,
        Some(Color::LightMagenta),
        "Number 42 should be LightMagenta"
    );
}

#[test]
fn non_r_code_block_uses_dim_style() {
    let input = "```python\nprint('hello')\n```";
    let lines = render_markdown(input, None, None);
    // Should produce one line with dim style (not syntax highlighted)
    let code_line = lines.iter().find(|l| {
        let t: String = l.spans.iter().map(|s| s.content.as_ref()).collect();
        t.contains("print")
    });
    assert!(code_line.is_some());
    let code_line = code_line.unwrap();
    // Non-R code blocks get a single dim span per line
    assert_eq!(
        code_line.spans.len(),
        1,
        "Non-R code should be a single span"
    );
    assert!(
        code_line.spans[0]
            .style
            .add_modifier
            .contains(ratatui::style::Modifier::DIM),
        "Non-R code should use DIM style"
    );
}

#[test]
fn r_code_block_multiline() {
    let input = "```r\nif (TRUE) {\n  print(x)\n}\n```";
    let lines = render_markdown(input, None, None);
    let texts: Vec<String> = lines
        .iter()
        .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
        .collect();
    assert!(texts.iter().any(|t| t.contains("if")));
    assert!(texts.iter().any(|t| t.contains("print")));

    // The "if" keyword should be highlighted
    let if_line = lines
        .iter()
        .find(|l| l.spans.iter().any(|s| s.content.as_ref() == "if"));
    assert!(if_line.is_some());
    let if_span = if_line
        .unwrap()
        .spans
        .iter()
        .find(|s| s.content.as_ref() == "if");
    assert_eq!(
        if_span.unwrap().style.fg,
        Some(Color::LightBlue),
        "Keyword 'if' should be LightBlue"
    );
}

#[test]
fn r_code_block_with_comments() {
    let input = "```r\n# A comment\nx <- 1\n```";
    let lines = render_markdown(input, None, None);
    // Comment line should be DarkGray
    let comment_line = lines.iter().find(|l| {
        l.spans
            .iter()
            .any(|s| s.content.as_ref().contains("# A comment"))
    });
    assert!(comment_line.is_some());
    let comment_span = comment_line
        .unwrap()
        .spans
        .iter()
        .find(|s| s.content.as_ref().contains("# A comment"));
    assert_eq!(
        comment_span.unwrap().style.fg,
        Some(Color::DarkGray),
        "Comment should be DarkGray"
    );
}

#[test]
fn untagged_code_block_with_default_r() {
    // Code blocks without a language tag should use the default language
    let input = "```\nx <- 42\n```";
    // Without default: no highlighting (dim style)
    let lines_no_default = render_markdown(input, None, None);
    let code_line = lines_no_default.iter().find(|l| {
        let t: String = l.spans.iter().map(|s| s.content.as_ref()).collect();
        t.contains("<-")
    });
    assert!(code_line.is_some());
    // Should be a single dim span (no tokenization)
    assert_eq!(code_line.unwrap().spans.len(), 1);

    // With default "r": should get syntax highlighting
    let lines_r = render_markdown(input, Some("r"), None);
    let code_line = lines_r.iter().find(|l| {
        let t: String = l.spans.iter().map(|s| s.content.as_ref()).collect();
        t.contains("<-")
    });
    assert!(code_line.is_some());
    let code_line = code_line.unwrap();
    // Should be tokenized into multiple spans
    assert!(
        code_line.spans.len() >= 3,
        "Default R should tokenize untagged code blocks, got {} spans",
        code_line.spans.len()
    );
    // Operator should be Yellow
    let op_span = code_line
        .spans
        .iter()
        .find(|s| s.content.as_ref().contains("<-"));
    assert_eq!(op_span.unwrap().style.fg, Some(Color::Yellow));
}

#[test]
fn explicit_lang_overrides_default() {
    // Explicit language tag should take precedence over default
    let input = "```python\nprint('hello')\n```";
    let lines = render_markdown(input, Some("r"), None);
    let code_line = lines.iter().find(|l| {
        let t: String = l.spans.iter().map(|s| s.content.as_ref()).collect();
        t.contains("print")
    });
    assert!(code_line.is_some());
    // Python code block should NOT be R-highlighted, should be dim
    assert_eq!(
        code_line.unwrap().spans.len(),
        1,
        "Explicit python tag should not use R highlighting even with default_code_lang=r"
    );
}

#[test]
fn empty_input() {
    let lines = render_plain("");
    assert!(lines.is_empty());
}

#[test]
fn paragraphs_separated_by_blank_line() {
    let input = "First paragraph.\n\nSecond paragraph.";
    let lines = render_plain(input);
    assert_eq!(lines, vec!["First paragraph.", "", "Second paragraph."]);
}

// ── wrap_width ─────────────────────────────────────────────────────

/// Helper: render with wrapping and collect plain text.
fn render_wrapped(input: &str, width: usize) -> Vec<String> {
    render_markdown(input, None, Some(width))
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>()
        })
        .collect()
}

#[test]
fn wrap_long_paragraph() {
    let input = "Hello world, this is a long paragraph that should wrap.";
    let lines = render_wrapped(input, 20);
    assert!(lines.len() > 1, "Should wrap into multiple lines");
    for line in &lines {
        assert!(
            unicode_width::UnicodeWidthStr::width(line.as_str()) <= 20,
            "Line too wide: {:?} ({})",
            line,
            unicode_width::UnicodeWidthStr::width(line.as_str())
        );
    }
}

#[test]
fn wrap_code_block_not_wrapped() {
    let input = "```\nthis is a very long code line that should not be wrapped at all ever\n```";
    let lines = render_wrapped(input, 20);
    let code_text: String = lines
        .iter()
        .find(|l| l.contains("this is"))
        .cloned()
        .unwrap_or_default();
    assert!(code_text.len() > 20, "Code block should NOT be wrapped");
}

#[test]
fn wrap_table_cells() {
    let input = "| Arg | Description |\n|---|---|\n| x | A very long description that should be wrapped within the cell |";
    let lines = render_wrapped(input, 40);
    // The table should have more rows than the raw 3 (header + sep + 1 data row)
    // because the long description wraps.
    assert!(
        lines.len() > 3,
        "Table cell should wrap: got {} lines: {:?}",
        lines.len(),
        lines
    );
    // All lines should fit within the wrap width
    for line in &lines {
        let w = unicode_width::UnicodeWidthStr::width(line.as_str());
        assert!(w <= 40, "Table line too wide ({} > 40): {:?}", w, line);
    }
}

#[test]
fn wrap_table_narrow_preserves_content() {
    let input = "| Name | Value |\n|---|---|\n| foo | bar |";
    // Wide enough that no wrapping is needed
    let lines_wide = render_wrapped(input, 80);
    // Narrow — may wrap but should still contain all content
    let lines_narrow = render_wrapped(input, 30);
    let all_text_wide: String = lines_wide.join("");
    let all_text_narrow: String = lines_narrow.join("");
    assert!(all_text_narrow.contains("foo"));
    assert!(all_text_narrow.contains("bar"));
    assert!(all_text_wide.contains("foo"));
    assert!(all_text_wide.contains("bar"));
}

#[test]
fn wrap_prose_but_not_code_block() {
    // Integration test: a document with both a paragraph and a code block.
    // The paragraph should wrap; the code block should not.
    let input = "This is a long paragraph that will be wrapped at the given width.\n\n```\ncode_line_that_must_not_be_wrapped_ever()\n```";
    let lines = render_wrapped(input, 30);

    // Paragraph lines should all fit within 30 columns and actually wrap
    let code_start = lines
        .iter()
        .position(|l| l.contains("code_line"))
        .expect("Should contain the code line");
    assert!(
        code_start > 2,
        "Paragraph should have wrapped into multiple lines, got {}",
        code_start
    );
    for (i, line) in lines.iter().enumerate() {
        if i < code_start && !line.is_empty() {
            let w = unicode_width::UnicodeWidthStr::width(line.as_str());
            assert!(
                w <= 30,
                "Paragraph line should wrap (line {}: {} cols): {:?}",
                i,
                w,
                line
            );
        }
    }

    // Code block line should NOT be wrapped (wider than 30)
    let code_line = &lines[code_start];
    assert!(
        code_line.contains("code_line_that_must_not_be_wrapped_ever()"),
        "Code block should remain intact"
    );
}

#[test]
fn distribute_column_widths_empty() {
    let empty: Vec<usize> = vec![];
    assert_eq!(distribute_column_widths(&[], 10), empty);
}

#[test]
fn distribute_column_widths_single_column() {
    // Single wide column gets clamped to available
    assert_eq!(distribute_column_widths(&[20], 10), vec![10]);
}

#[test]
fn distribute_column_widths_all_fit() {
    // All columns fit within their fair share — no redistribution needed
    assert_eq!(distribute_column_widths(&[3, 3, 3], 12), vec![3, 3, 3]);
}

#[test]
fn distribute_column_widths_one_wide() {
    // One wide column, two narrow. Narrow keep natural, wide gets the rest.
    let result = distribute_column_widths(&[2, 20, 3], 15);
    assert_eq!(result[0], 2); // narrow, kept
    assert_eq!(result[2], 3); // narrow, kept
    assert_eq!(result[1], 10); // wide, gets 15 - 2 - 3 = 10
    assert_eq!(result.iter().sum::<usize>(), 15);
}

#[test]
fn distribute_column_widths_available_less_than_n() {
    // available < n_cols: first `available` columns get 1, rest get 0
    let result = distribute_column_widths(&[5, 10, 15], 2);
    assert_eq!(result, vec![1, 1, 0]);
    assert!(result.iter().sum::<usize>() <= 2);
}

#[test]
fn distribute_column_widths_available_equals_n() {
    // available == n_cols: each column gets exactly 1
    let result = distribute_column_widths(&[5, 10, 15], 3);
    assert_eq!(result, vec![1, 1, 1]);
}
