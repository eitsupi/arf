//! Convert Pandoc-generated R vignette HTML to Markdown.
//!
//! R vignettes are typically authored in R Markdown or Quarto and compiled
//! to HTML by Pandoc. This crate converts that HTML back to Markdown,
//! handling Pandoc-specific patterns that generic HTML-to-Markdown converters
//! do not handle.
//!
//! # Features
//!
//! - Detects code block languages from Pandoc's `sourceCode` class pattern
//! - Removes Pandoc-generated line-number anchors from code blocks
//! - Strips `<style>` and `<script>` tags
//!
//! # Example
//!
//! ```
//! let html = r#"<pre class="sourceCode r"><code class="sourceCode r">print("hello")</code></pre>"#;
//! let md = r_vignette_to_md::convert(html).unwrap();
//! assert!(md.contains("```r"));
//! ```

/// Convert Pandoc-generated R vignette HTML to Markdown.
///
/// This applies the following transformations:
/// 1. Normalizes Pandoc's `class="sourceCode r"` to add `language-r` for
///    fenced code block info strings
/// 2. Converts HTML to Markdown via htmd
/// 3. Strips Pandoc-generated code block line-number anchors
pub fn convert(html: &str) -> Result<String, std::io::Error> {
    let preprocessed = add_pandoc_language_class(html);

    let converter = htmd::HtmlToMarkdown::builder()
        .skip_tags(vec!["style", "script"])
        .build();

    let markdown = converter.convert(&preprocessed)?;

    Ok(strip_pandoc_code_anchors(&markdown))
}

/// Preprocess Pandoc HTML to add `language-` prefixed classes for code blocks.
///
/// Pandoc generates `<code class="sourceCode r">` but htmd's language detection
/// expects `class="language-r"`. This inserts a `language-` prefix so htmd
/// can detect the language for fenced code block info strings.
///
/// Only matches `class="sourceCode X"` where there is a token after `sourceCode`.
/// Attributes like `class="sourceCode"` (without a language) are left untouched.
fn add_pandoc_language_class(html: &str) -> String {
    const PATTERN: &str = r#"class="sourceCode "#;

    let mut result = String::with_capacity(html.len() + 256);
    let mut remaining = html;

    while let Some(start) = remaining.find(PATTERN) {
        result.push_str(&remaining[..start + PATTERN.len()]);
        remaining = &remaining[start + PATTERN.len()..];

        // Don't double-prefix if already normalized
        if !remaining.starts_with("language-") {
            result.push_str("language-");
        }
    }

    result.push_str(remaining);
    result
}

/// Strip Pandoc-generated code block anchors from Markdown.
///
/// Pandoc inserts line-number anchors like `<a href="#cb1-1"></a>` in code
/// blocks, which HTML-to-Markdown converters turn into `[](#cb1-1)`.
/// This removes only Pandoc code block anchors (matching `[](#cb` prefix),
/// preserving other empty anchor patterns like footnote backrefs.
fn strip_pandoc_code_anchors(text: &str) -> String {
    const PATTERN: &str = "[](#cb";

    let mut result = String::with_capacity(text.len());
    let mut remaining = text;

    while let Some(start) = remaining.find(PATTERN) {
        result.push_str(&remaining[..start]);
        remaining = &remaining[start + PATTERN.len()..];

        if let Some(end) = remaining.find(')') {
            remaining = &remaining[end + 1..];
        } else {
            // No closing paren found; keep the pattern as-is
            result.push_str(PATTERN);
        }
    }

    result.push_str(remaining);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- add_pandoc_language_class ---

    #[test]
    fn add_language_class_basic() {
        let input = r#"<code class="sourceCode r">x <- 1</code>"#;
        let result = add_pandoc_language_class(input);
        assert!(result.contains(r#"class="sourceCode language-r"#));
    }

    #[test]
    fn add_language_class_no_language() {
        // class="sourceCode" (no trailing space) should not be modified
        let input = r#"<div class="sourceCode" id="cb1">"#;
        let result = add_pandoc_language_class(input);
        assert_eq!(result, input);
    }

    #[test]
    fn add_language_class_idempotent() {
        let input = r#"<code class="sourceCode language-r">x <- 1</code>"#;
        let result = add_pandoc_language_class(input);
        assert_eq!(result, input);
    }

    #[test]
    fn add_language_class_python() {
        let input = r#"<code class="sourceCode python">print("hello")</code>"#;
        let result = add_pandoc_language_class(input);
        assert!(result.contains(r#"class="sourceCode language-python"#));
    }

    #[test]
    fn add_language_class_multiple_elements() {
        let input = r#"<pre class="sourceCode r"><code class="sourceCode r">x</code></pre>"#;
        let result = add_pandoc_language_class(input);
        assert_eq!(
            result,
            r#"<pre class="sourceCode language-r"><code class="sourceCode language-r">x</code></pre>"#
        );
    }

    // --- strip_pandoc_code_anchors ---

    #[test]
    fn strip_cb_anchors_basic() {
        let input = "some code[](#cb1-1) more code";
        assert_eq!(strip_pandoc_code_anchors(input), "some code more code");
    }

    #[test]
    fn strip_cb_anchors_multiple() {
        let input = "[](#cb1-1)line1\n[](#cb1-2)line2\n[](#cb1-3)line3";
        assert_eq!(strip_pandoc_code_anchors(input), "line1\nline2\nline3");
    }

    #[test]
    fn preserve_non_cb_anchors() {
        // Footnote backrefs like [](#fn1) should be preserved
        let input = "text [](#fn1) more text";
        assert_eq!(strip_pandoc_code_anchors(input), input);
    }

    #[test]
    fn preserve_normal_links() {
        let input = "see [docs](https://example.com) for details";
        assert_eq!(strip_pandoc_code_anchors(input), input);
    }

    #[test]
    fn strip_empty_string() {
        assert_eq!(strip_pandoc_code_anchors(""), "");
    }

    #[test]
    fn strip_unclosed_anchor() {
        let input = "text [](#cb-broken";
        assert_eq!(strip_pandoc_code_anchors(input), "text [](#cb-broken");
    }

    // --- convert (integration) ---

    #[test]
    fn convert_code_block_with_language() {
        let html = r#"<pre class="sourceCode r"><code class="sourceCode r">x &lt;- 1
print(x)</code></pre>"#;
        let result = convert(html).unwrap();
        assert!(result.contains("```r"), "Expected ```r in:\n{result}");
        assert!(result.contains("x <- 1"));
    }

    #[test]
    fn convert_strips_style_and_script() {
        let html = "<style>body{}</style><script>alert(1)</script><p>Hello</p>";
        let result = convert(html).unwrap();
        assert!(!result.contains("body{}"));
        assert!(!result.contains("alert"));
        assert!(result.contains("Hello"));
    }

    #[test]
    fn convert_strips_code_anchors() {
        let html = r##"<pre class="sourceCode r"><code class="sourceCode r"><a href="#cb1-1"></a>x &lt;- 1</code></pre>"##;
        let result = convert(html).unwrap();
        assert!(
            !result.contains("[](#cb1-1)"),
            "Anchor not stripped in:\n{result}"
        );
    }

    #[test]
    fn convert_preserves_non_code_anchors() {
        let html = r##"<p>See footnote <a href="#fn1"></a> for details</p>"##;
        let result = convert(html).unwrap();
        assert!(
            result.contains("[](#fn1)"),
            "Non-code anchor was stripped in:\n{result}"
        );
    }

    #[test]
    fn convert_python_code_block() {
        let html = r#"<pre class="sourceCode python"><code class="sourceCode python">print("hello")</code></pre>"#;
        let result = convert(html).unwrap();
        assert!(
            result.contains("```python"),
            "Expected ```python in:\n{result}"
        );
    }

    #[test]
    fn convert_bash_code_block() {
        let html =
            r#"<pre class="sourceCode bash"><code class="sourceCode bash">echo "hi"</code></pre>"#;
        let result = convert(html).unwrap();
        assert!(result.contains("```bash"), "Expected ```bash in:\n{result}");
    }

    #[test]
    fn convert_mixed_language_blocks() {
        let html = r#"<p>R code:</p>
<pre class="sourceCode r"><code class="sourceCode r">x &lt;- 1</code></pre>
<p>Python code:</p>
<pre class="sourceCode python"><code class="sourceCode python">x = 1</code></pre>"#;
        let result = convert(html).unwrap();
        assert!(result.contains("```r"), "Expected ```r in:\n{result}");
        assert!(
            result.contains("```python"),
            "Expected ```python in:\n{result}"
        );
    }

    #[test]
    fn convert_code_block_without_language() {
        let html = r#"<pre class="sourceCode"><code class="sourceCode">plain code</code></pre>"#;
        let result = convert(html).unwrap();
        assert!(
            result.contains("plain code"),
            "Code content missing in:\n{result}"
        );
    }
}
