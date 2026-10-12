use super::*;

#[test]
fn link_focus_skips_non_help_targets_wraps_and_scrolls_into_view() {
    let source = format!(
        "[site](https://example.com) [broken](x-r-help:base/)\n\n[first](x-r-help:/mean)\n\n{}\n\n[last](x-r-help:stats/lm)",
        "paragraph\n\n".repeat(12)
    );
    let mut content = HelpContent::new(&source, 30, 6);
    assert_eq!(content.selected_target(), None);
    assert!(content.has_help_links());
    content.select_link(true);
    assert_eq!(content.selected_target().unwrap().topic, "mean");
    assert!(matches!(
        content.select_link(true),
        PagerAction::ScrollTo(_)
    ));
    assert_eq!(content.selected_target().unwrap().topic, "lm");
    content.select_link(true);
    assert_eq!(content.selected_target().unwrap().topic, "mean");
    content.select_link(false);
    assert_eq!(content.selected_target().unwrap().topic, "lm");
    let mut content = HelpContent::new("[site](https://example.com)", 30, 6);
    assert!(!content.has_help_links());
    assert_eq!(content.select_link(true), PagerAction::Redraw);
    assert_eq!(content.selected_target(), None);
}

#[test]
fn wrapped_unicode_link_labels_are_highlighted_and_keep_independent_identities() {
    let mut content = HelpContent::new(
        "[**日本語** with long label](x-r-help:/mean) and [日本語](x-r-help:/mean)",
        12,
        24,
    );
    content.select_link(true);
    let first = content.selected_link;
    assert_eq!(first, Some(0));
    assert!(content.document.links[0].fragments.len() > 1);
    for fragment in &content.document.links[0].fragments {
        let line = content.render_line(fragment.line, 12);
        let highlighted: String = line
            .spans
            .iter()
            .filter(|s| s.style.bg == Some(Color::Cyan))
            .map(|s| s.content.as_ref())
            .collect();
        let plain = content.document.lines[fragment.line].to_string();
        assert!(highlighted.contains(&plain[fragment.byte_range.clone()]));
    }
    content.on_resize(30, 24);
    assert_eq!(content.selected_link, first);
    content.select_link(true);
    assert_eq!(content.selected_link, Some(1));
    assert_eq!(content.selected_target().unwrap().topic, "mean");
    content.select_link(false);
    assert_eq!(content.selected_link, first);
}

#[test]
fn rd_link_targets_survive_rendering_and_resize() {
    let rd = include_str!("../../../tests/fixtures/help_links.Rd");
    let source = arf_harp::help::rd_source_to_markdown(rd).unwrap();
    let expected = [
        (None, "mean"),
        (Some("stats"), "lm"),
        (None, "mean"),
        (None, "["),
        (None, "[["),
        (None, "%/%"),
        (None, "%in%"),
        (None, "/"),
        (Some("base"), "%/%"),
        (Some("base"), "mean"),
    ];
    let mut content = HelpContent::new(&source, 80, 24);
    for width in [8, 24, 80] {
        content.on_resize(width, 24);
        assert_eq!(content.document.links.len(), expected.len() + 1);
        for (link, (package, topic)) in content.document.links.iter().zip(expected) {
            assert_eq!(
                HelpTarget::from_uri(&link.destination),
                Some(HelpTarget {
                    package: package.map(str::to_owned),
                    topic: topic.to_owned(),
                }),
            );
            assert!(!link.text_ranges.is_empty());
            assert!(!link.fragments.is_empty());
            for fragment in &link.fragments {
                let line = &content.document.lines[fragment.line];
                let text: String = line
                    .spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect();
                assert!(!text[fragment.byte_range.clone()].is_empty());
                assert!(fragment.columns.start < fragment.columns.end);
            }
        }
        let label = &content.document.links[2];
        assert_eq!(label.label, "an average with a longer label");
        if width == 8 {
            assert!(label.fragments.len() > 1);
        }
        let website = content.document.links.last().unwrap();
        assert_eq!(website.destination, "https://example.com/docs");
        assert_eq!(HelpTarget::from_uri(&website.destination), None);
        assert!(
            content
                .document
                .text
                .iter()
                .all(|t| !t.text.contains("x-r-help:"))
        );
    }
}

#[test]
fn invalid_help_destinations_remain_readable() {
    let content = HelpContent::new(
        "[broken](x-r-help:base/) and [mean](x-r-help:/mean)",
        80,
        24,
    );
    assert_eq!(
        content.document.text[0].text,
        "broken (x-r-help:base/) and mean"
    );
}

#[test]
fn help_links_keep_metadata_without_exposing_internal_uris() {
    let source = "Read [**mean**](x-r-help:base/mean) and [docs](https://example.com).";
    let mut content = HelpContent::new(source, 80, 24);
    assert_eq!(
        content.document.text[0].text,
        "Read mean and docs (https://example.com)."
    );
    assert_eq!(content.document.links[0].destination, "x-r-help:base/mean");
    assert_eq!(content.document.links[1].destination, "https://example.com");
    for width in [12, 40, 80] {
        content.on_resize(width, 24);
        assert_eq!(content.document.links.len(), 2);
        assert_eq!(content.document.links[0].label, "mean");
        assert!(!content.document.links[0].fragments.is_empty());
        assert!(
            !content
                .document
                .text
                .iter()
                .any(|t| t.text.contains("x-r-help:"))
        );
    }
}

fn key(content: &mut HelpContent, code: KeyCode) -> Option<PagerAction> {
    content.handle_key(code, KeyModifiers::NONE)
}

fn search(content: &mut HelpContent, query: &str) -> Option<PagerAction> {
    key(content, KeyCode::Char('/'));
    for ch in query.chars() {
        key(content, KeyCode::Char(ch));
    }
    key(content, KeyCode::Enter)
}

fn highlighted_text(content: &HelpContent) -> String {
    (0..content.line_count())
        .flat_map(|index| content.render_line(index, content.width).spans)
        .filter(|span| span.style.bg == Some(Color::Yellow))
        .map(|span| span.content.into_owned())
        .collect()
}

#[test]
fn search_moves_forward_backward_and_wraps_in_both_directions() {
    let mut content = HelpContent::new("first needle\n\nsecond needle", 80, 24);
    assert_eq!(search(&mut content, "needle"), Some(PagerAction::Redraw));
    assert_eq!(content.current, Some(0));
    key(&mut content, KeyCode::Char('n'));
    assert_eq!(content.current, Some(1));
    key(&mut content, KeyCode::Char('n'));
    assert_eq!(content.current, Some(0));
    key(&mut content, KeyCode::Char('N'));
    assert_eq!(content.current, Some(1));
    key(&mut content, KeyCode::Char('N'));
    assert_eq!(content.current, Some(0));
}

#[test]
fn search_starts_at_viewport_and_scrolls_only_for_hidden_matches() {
    let mut content = HelpContent::new("needle\n\nother\n\nneedle", 80, 4);
    content.prepare_render(2);
    assert_eq!(
        search(&mut content, "needle"),
        Some(PagerAction::ScrollTo(3))
    );
    assert_eq!(content.current, Some(1));
    content.prepare_render(3);
    assert_eq!(
        key(&mut content, KeyCode::Char('n')),
        Some(PagerAction::ScrollTo(0))
    );
    content.prepare_render(0);
    assert_eq!(
        key(&mut content, KeyCode::Char('N')),
        Some(PagerAction::ScrollTo(3))
    );
}

#[test]
fn no_match_and_empty_search_report_feedback_without_moving() {
    let mut content = HelpContent::new("needle", 80, 24);
    assert_eq!(search(&mut content, ""), Some(PagerAction::Redraw));
    assert_eq!(content.feedback_message(), None);
    assert_eq!(search(&mut content, "absent"), Some(PagerAction::Redraw));
    assert!(content.matches.is_empty());
    assert!(
        content
            .feedback_message()
            .unwrap()
            .contains("No matches for: absent")
    );
    assert_eq!(
        key(&mut content, KeyCode::Char('N')),
        Some(PagerAction::Redraw)
    );
}

#[test]
fn search_uses_rendered_text_across_styles_with_literal_case_sensitive_matching() {
    let mut content = HelpContent::new("**bold**needle Needle a.b axb", 80, 24);
    search(&mut content, "boldneedle");
    assert_eq!(content.matches.len(), 1);
    let line = content.render_line(0, 80);
    assert_eq!(line.to_string(), "boldneedle Needle a.b axb");
    assert_eq!(
        line.spans
            .iter()
            .filter(|s| s.style.bg == Some(Color::Yellow))
            .map(|s| s.content.as_ref())
            .collect::<String>(),
        "boldneedle"
    );
    search(&mut content, "**");
    assert!(content.matches.is_empty());
    search(&mut content, "needle");
    assert_eq!(content.matches.len(), 1);
    search(&mut content, "a.b");
    assert_eq!(content.matches.len(), 1);
}

#[test]
fn unicode_matches_and_highlighting_preserve_text_and_style() {
    let mut content = HelpContent::new("前 **日本**語 後 日本語", 80, 24);
    search(&mut content, "日本語");
    assert_eq!(content.matches.len(), 2);
    let line = content.render_line(0, 80);
    assert_eq!(line.to_string(), "前 日本語 後 日本語");
    assert!(line.spans.iter().any(|s| s.content == "日本"
        && s.style.bg == Some(Color::Yellow)
        && s.style.add_modifier.contains(Modifier::BOLD)));
    assert!(
        line.spans
            .iter()
            .any(|s| s.content.contains("後 日本語") && s.style.bg != Some(Color::Yellow))
    );
}

#[test]
fn logical_matches_survive_resize_and_map_to_wrapped_lines() {
    let mut content = HelpContent::new("alpha beta needle gamma needle", 80, 24);
    search(&mut content, "needle");
    assert_eq!(content.matches.len(), 2);
    let matches = content.matches.clone();
    assert!(content.matches.iter().all(|m| content.match_line(*m) == 0));
    content.on_resize(12, 24);
    assert_eq!(content.matches, matches);
    assert_eq!(content.current, None);
    assert!(content.matches.iter().all(|m| content.match_line(*m) > 0));
    for m in &content.matches {
        for (line, start, end) in content.visual_ranges(*m) {
            assert_eq!(
                &content.document.lines[line].to_string()[start..end],
                "needle"
            );
        }
    }
    search(&mut content, "beta needle");
    assert_eq!(content.matches.len(), 1);
    let matches = content.matches.clone();
    for width in [80, 12, 5, 1, 0] {
        content.on_resize(width, 24);
        assert_eq!(content.matches, matches);
        key(&mut content, KeyCode::Char('n'));
        assert_eq!(highlighted_text(&content).replace(' ', ""), "betaneedle");
    }
}

#[test]
fn wrapped_unicode_phrases_keep_identity_and_highlight_in_lists_and_quotes() {
    for source in ["- 前 **日本**語 needle 後", "> 前 **日本**語 needle 後"] {
        let mut content = HelpContent::new(source, 80, 24);
        search(&mut content, "日本語 needle");
        let matches = content.matches.clone();
        assert_eq!(matches.len(), 1);
        for width in [8, 6, 2, 80] {
            content.on_resize(width, 24);
            assert_eq!(content.matches, matches);
            key(&mut content, KeyCode::Char('n'));
            assert_eq!(highlighted_text(&content).replace(' ', ""), "日本語needle");
        }
    }
}

#[test]
fn table_cell_phrases_survive_wrapping_and_map_past_unicode_columns() {
    let source = "| Left | Right |\n| --- | --- |\n| 日本 | alpha **beta** needle gamma |";
    let mut content = HelpContent::new(source, 80, 24);
    search(&mut content, "beta needle");
    let matches = content.matches.clone();
    assert_eq!(matches.len(), 1);
    for width in [22, 16, 80] {
        content.on_resize(width, 24);
        assert_eq!(content.matches, matches);
        key(&mut content, KeyCode::Char('n'));
        assert_eq!(highlighted_text(&content).replace(' ', ""), "betaneedle");
    }
}

#[test]
fn search_does_not_cross_hard_breaks_paragraphs_code_lines_or_table_cells() {
    for source in [
        "beta  \nneedle",
        "beta\n\nneedle",
        "```\nbeta\nneedle\n```",
        "| Left | Right |\n| --- | --- |\n| beta | needle |",
        "| Text |\n| --- |\n| beta<br>needle |",
    ] {
        let mut content = HelpContent::new(source, 80, 24);
        search(&mut content, "beta needle");
        assert!(content.matches.is_empty());
        content.on_resize(12, 24);
        assert!(content.matches.is_empty());
    }
}

#[test]
fn resize_clears_selection_and_keeps_query_and_count_for_next_navigation() {
    let mut content = HelpContent::new("needle needle", 80, 24);
    search(&mut content, "needle");
    key(&mut content, KeyCode::Char('n'));
    assert_eq!(content.current, Some(1));
    content.on_resize(80, 4);
    assert_eq!(content.current, None);
    assert_eq!(content.query, "needle");
    assert!(content.feedback_message().unwrap().contains("[0/2]"));
    assert_eq!(highlighted_text(&content), "");
    content.on_resize(6, 4);
    content.prepare_render(1);
    key(&mut content, KeyCode::Char('n'));
    assert_eq!(content.current, Some(1));
    assert_eq!(highlighted_text(&content), "needle");
}

#[test]
fn input_consumes_pager_keys_edits_unicode_and_escape_restores_search() {
    let mut content = HelpContent::new("needle", 80, 24);
    search(&mut content, "needle");
    let previous = content.matches.clone();
    key(&mut content, KeyCode::Char('/'));
    for ch in "qjk/日本".chars() {
        assert_eq!(
            key(&mut content, KeyCode::Char(ch)),
            Some(PagerAction::Redraw)
        );
    }
    key(&mut content, KeyCode::Backspace);
    assert_eq!(content.input.as_deref(), Some("qjk/日"));
    for code in [KeyCode::Down, KeyCode::PageDown, KeyCode::Tab] {
        assert_eq!(key(&mut content, code), Some(PagerAction::Redraw));
    }
    assert_eq!(key(&mut content, KeyCode::Esc), Some(PagerAction::Redraw));
    assert_eq!(content.input, None);
    assert_eq!(content.query, "needle");
    assert_eq!(content.matches, previous);
    assert_eq!(content.current, Some(0));
    assert_eq!(key(&mut content, KeyCode::Esc), None);
    assert_eq!(key(&mut content, KeyCode::Enter), None);
}

#[test]
fn enter_outside_input_keeps_the_page_and_search_unchanged() {
    let mut content = HelpContent::new("needle needle", 80, 24);
    assert_eq!(key(&mut content, KeyCode::Enter), None);
    search(&mut content, "needle");
    let current = content.current;
    let status = content.status.clone();
    let highlighted = content.render_line(0, 80);
    assert_eq!(key(&mut content, KeyCode::Enter), None);
    assert_eq!(content.current, current);
    assert_eq!(content.status, status);
    assert_eq!(content.render_line(0, 80), highlighted);
}

#[test]
fn q_clears_committed_search_and_highlight_without_changing_scroll() {
    let mut content = HelpContent::new("needle", 80, 24);
    search(&mut content, "needle");
    content.prepare_render(3);
    assert!(content.feedback_message().unwrap().contains("n/N"));
    assert!(
        content
            .feedback_message()
            .unwrap()
            .contains("q clear search")
    );
    assert_eq!(
        key(&mut content, KeyCode::Char('q')),
        Some(PagerAction::Redraw)
    );
    assert_eq!(content.scroll_offset, 3);
    assert_eq!(content.feedback_message(), None);
    assert!(content.query.is_empty());
    assert!(content.matches.is_empty());
    assert_eq!(content.current, None);
    assert_eq!(content.render_line(0, 80), content.document.lines[0]);
    assert_eq!(key(&mut content, KeyCode::Char('q')), None);
    for ch in ['n', 'N'] {
        assert_eq!(key(&mut content, KeyCode::Char(ch)), None);
        assert_eq!(content.feedback_message(), None);
    }
}

#[test]
fn no_match_search_can_be_cleared_and_does_not_advertise_match_navigation() {
    let mut content = HelpContent::new("needle", 80, 24);
    search(&mut content, "absent");
    assert!(!content.feedback_message().unwrap().contains("n/N"));
    assert_eq!(
        key(&mut content, KeyCode::Char('q')),
        Some(PagerAction::Redraw)
    );
    assert_eq!(content.feedback_message(), None);
    assert_eq!(key(&mut content, KeyCode::Char('q')), None);
}

#[test]
fn empty_query_repeats_search_and_control_exit_keys_fall_through() {
    let mut content = HelpContent::new("needle needle", 80, 24);
    search(&mut content, "needle");
    search(&mut content, "");
    assert_eq!(content.current, Some(1));
    key(&mut content, KeyCode::Char('/'));
    for ch in ['c', 'd'] {
        assert_eq!(
            content.handle_key(KeyCode::Char(ch), KeyModifiers::CONTROL),
            None
        );
    }
    assert_eq!(content.input.as_deref(), Some(""));
}
