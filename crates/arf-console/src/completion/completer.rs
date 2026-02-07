//! R code completion for reedline.

use super::path::{PathCompletionOptions, complete_path};
use crate::external::rig;
use crate::fuzzy::fuzzy_match;
use reedline::{Completer, Span, Suggestion};
use std::cell::RefCell;
use std::time::{Duration, Instant};
use tree_sitter::{Parser, Tree};

// Thread-local tree-sitter parser for R.
thread_local! {
    static R_PARSER: RefCell<Parser> = RefCell::new({
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_r::LANGUAGE.into())
            .expect("Failed to set tree-sitter-r language");
        parser
    });
}

/// Definition of a meta command for completion.
struct MetaCommandDef {
    name: &'static str,
    description: &'static str,
    /// Whether this command takes an argument (e.g., `:switch 4.4`).
    /// If true, a trailing space is appended after completion.
    takes_argument: bool,
}

/// Available meta commands.
const META_COMMANDS: &[MetaCommandDef] = &[
    MetaCommandDef {
        name: "help",
        description: "Search R help",
        takes_argument: false,
    },
    MetaCommandDef {
        name: "h",
        description: "Search R help (alias)",
        takes_argument: false,
    },
    MetaCommandDef {
        name: "info",
        description: "Show session information",
        takes_argument: false,
    },
    MetaCommandDef {
        name: "session",
        description: "Show session information (alias)",
        takes_argument: false,
    },
    MetaCommandDef {
        name: "shell",
        description: "Enter shell mode",
        takes_argument: false,
    },
    MetaCommandDef {
        name: "r",
        description: "Return to R mode",
        takes_argument: false,
    },
    MetaCommandDef {
        name: "system",
        description: "Execute system command",
        takes_argument: true,
    },
    MetaCommandDef {
        name: "reprex",
        description: "Toggle reprex mode",
        takes_argument: false,
    },
    MetaCommandDef {
        name: "autoformat",
        description: "Toggle auto-formatting (requires Air CLI)",
        takes_argument: false,
    },
    MetaCommandDef {
        name: "format",
        description: "Toggle auto-formatting (alias)",
        takes_argument: false,
    },
    MetaCommandDef {
        name: "commands",
        description: "Show available commands",
        takes_argument: false,
    },
    MetaCommandDef {
        name: "cmds",
        description: "Show available commands (alias)",
        takes_argument: false,
    },
    MetaCommandDef {
        name: "restart",
        description: "Restart R session",
        takes_argument: false,
    },
    MetaCommandDef {
        name: "switch",
        description: "Restart with different R version (requires rig)",
        takes_argument: true,
    },
    MetaCommandDef {
        name: "history",
        description: "Manage command history",
        takes_argument: true,
    },
    MetaCommandDef {
        name: "quit",
        description: "Quit arf",
        takes_argument: false,
    },
    MetaCommandDef {
        name: "exit",
        description: "Quit arf",
        takes_argument: false,
    },
];

/// Completer for meta commands (starting with `:`).
pub struct MetaCommandCompleter {
    /// Commands to exclude from completion (e.g., `:r` in R mode, `:shell` in Shell mode).
    excluded_commands: Vec<&'static str>,
}

impl MetaCommandCompleter {
    pub fn new() -> Self {
        MetaCommandCompleter {
            excluded_commands: vec![],
        }
    }

    /// Create a new completer with specified commands excluded from completion.
    pub fn with_exclusions(excluded_commands: Vec<&'static str>) -> Self {
        MetaCommandCompleter { excluded_commands }
    }

    /// Complete meta commands.
    fn complete_commands(&self, line: &str, pos: usize) -> Vec<Suggestion> {
        let trimmed = line.trim_start();
        if !trimmed.starts_with(':') {
            return vec![];
        }

        // Get the part after ':'
        let after_colon = &trimmed[1..];

        // Check if there's a trailing space (user finished typing and wants subcommands)
        let has_trailing_space = after_colon.ends_with(' ') || after_colon.ends_with('\t');
        let parts: Vec<&str> = after_colon.split_whitespace().collect();

        // Calculate the start position for the span
        let leading_whitespace = line.len() - trimmed.len();

        match (parts.len(), has_trailing_space) {
            (0, _) => {
                // Just ":" - show all commands
                let start = leading_whitespace + 1; // after ':'
                let mut suggestions: Vec<Suggestion> = META_COMMANDS
                    .iter()
                    .filter(|cmd| !self.excluded_commands.contains(&cmd.name))
                    .map(|cmd| Suggestion {
                        value: cmd.name.to_string(),
                        display_override: None,
                        description: Some(cmd.description.to_string()),
                        extra: None,
                        span: Span { start, end: pos },
                        append_whitespace: cmd.takes_argument,
                        style: None,
                        match_indices: None,
                    })
                    .collect();
                // Sort by length so shorter aliases (h, r, cmds) appear before longer forms
                suggestions.sort_by_key(|s| s.value.len());
                suggestions
            }
            (1, false) => {
                // Typing command name, e.g., ":rep" or ":rst" (fuzzy)
                let partial = parts[0];
                let start = leading_whitespace + 1; // after ':'
                let mut suggestions: Vec<Suggestion> = META_COMMANDS
                    .iter()
                    .filter(|cmd| !self.excluded_commands.contains(&cmd.name))
                    .filter_map(|cmd| {
                        fuzzy_match(partial, cmd.name).map(|m| Suggestion {
                            value: cmd.name.to_string(),
                            display_override: None,
                            description: Some(cmd.description.to_string()),
                            extra: None,
                            span: Span { start, end: pos },
                            append_whitespace: cmd.takes_argument,
                            style: None,
                            match_indices: if m.indices.is_empty() {
                                None
                            } else {
                                Some(m.indices)
                            },
                        })
                    })
                    .collect();
                // Sort by length so shorter aliases (h, r, cmds) appear before longer forms
                suggestions.sort_by_key(|s| s.value.len());
                suggestions
            }
            (1, true) => {
                // Command complete with trailing space - check for subcommands
                let cmd = parts[0];
                if cmd == "switch" {
                    // Complete with R versions from rig
                    self.complete_switch_versions(line, pos, leading_whitespace, "")
                } else if cmd == "history" {
                    // Complete with history subcommands
                    self.complete_history_subcommands(pos, "")
                } else {
                    vec![]
                }
            }
            (2, false) => {
                // Typing subcommand argument
                let cmd = parts[0];
                let partial = parts[1];
                if cmd == "switch" {
                    self.complete_switch_versions(line, pos, leading_whitespace, partial)
                } else if cmd == "history" {
                    self.complete_history_subcommands(pos, partial)
                } else {
                    vec![]
                }
            }
            (2, true) => {
                // Two parts complete with trailing space - check for third level
                let cmd = parts[0];
                let subcmd = parts[1];
                if cmd == "history" && subcmd == "clear" {
                    // Complete with clear targets (r, shell, all)
                    self.complete_history_clear_targets(pos, "")
                } else if cmd == "history" && subcmd == "browse" {
                    // Complete with browse targets (r, shell)
                    self.complete_history_browse_targets(pos, "")
                } else {
                    vec![]
                }
            }
            (3, false) => {
                // Typing third argument
                let cmd = parts[0];
                let subcmd = parts[1];
                let partial = parts[2];
                if cmd == "history" && subcmd == "clear" {
                    self.complete_history_clear_targets(pos, partial)
                } else if cmd == "history" && subcmd == "browse" {
                    self.complete_history_browse_targets(pos, partial)
                } else {
                    vec![]
                }
            }
            _ => {
                // No more completions
                vec![]
            }
        }
    }

    /// Complete R versions for the :switch command.
    fn complete_switch_versions(
        &self,
        line: &str,
        pos: usize,
        _leading_whitespace: usize,
        partial: &str,
    ) -> Vec<Suggestion> {
        // Check if rig is available
        if !rig::rig_available() {
            return vec![];
        }

        // Get installed R versions from rig
        let versions = match rig::list_versions() {
            Ok(v) => v,
            Err(_) => return vec![],
        };

        // Calculate span start (after ":switch ")
        let start = if partial.is_empty() {
            pos
        } else {
            // Find where the partial version starts
            line.rfind(partial).unwrap_or(pos)
        };

        // Build suggestions from versions
        let match_len = partial.len();
        let mut suggestions: Vec<Suggestion> = versions
            .iter()
            .filter(|v| v.name.starts_with(partial) || v.version.starts_with(partial))
            .map(|v| {
                let description = if v.default {
                    format!("R {} (default)", v.version)
                } else {
                    format!("R {}", v.version)
                };
                let indices = if match_len > 0 {
                    Some((0..match_len).collect())
                } else {
                    None
                };
                Suggestion {
                    value: v.name.clone(),
                    display_override: None,
                    description: Some(description),
                    extra: None,
                    span: Span { start, end: pos },
                    append_whitespace: false,
                    style: None,
                    match_indices: indices,
                }
            })
            .collect();

        // Also add aliases as suggestions
        for v in &versions {
            for alias in &v.aliases {
                if alias.starts_with(partial) {
                    let indices = if match_len > 0 {
                        Some((0..match_len).collect())
                    } else {
                        None
                    };
                    suggestions.push(Suggestion {
                        value: alias.clone(),
                        display_override: None,
                        description: Some(format!("R {} (alias)", v.version)),
                        extra: None,
                        span: Span { start, end: pos },
                        append_whitespace: false,
                        style: None,
                        match_indices: indices,
                    });
                }
            }
        }

        suggestions
    }

    /// Complete history subcommands (browse, clear, schema).
    fn complete_history_subcommands(&self, pos: usize, partial: &str) -> Vec<Suggestion> {
        let subcommands = [
            ("browse", "Browse and manage command history"),
            ("clear", "Clear command history"),
            ("schema", "Display database schema and R examples"),
        ];

        let match_len = partial.len();
        subcommands
            .iter()
            .filter(|(name, _)| name.starts_with(partial))
            .map(|(name, desc)| {
                let indices = if match_len > 0 {
                    Some((0..match_len).collect())
                } else {
                    None
                };
                Suggestion {
                    value: name.to_string(),
                    display_override: None,
                    description: Some(desc.to_string()),
                    extra: None,
                    span: Span {
                        start: pos - match_len,
                        end: pos,
                    },
                    append_whitespace: true,
                    style: None,
                    match_indices: indices,
                }
            })
            .collect()
    }

    /// Complete from a list of (name, description) targets.
    fn complete_targets(
        &self,
        pos: usize,
        partial: &str,
        targets: &[(&str, &str)],
    ) -> Vec<Suggestion> {
        let match_len = partial.len();
        targets
            .iter()
            .filter(|(name, _)| name.starts_with(partial))
            .map(|(name, desc)| {
                let indices = if match_len > 0 {
                    Some((0..match_len).collect())
                } else {
                    None
                };
                Suggestion {
                    value: name.to_string(),
                    display_override: None,
                    description: Some(desc.to_string()),
                    extra: None,
                    span: Span {
                        start: pos - match_len,
                        end: pos,
                    },
                    append_whitespace: false,
                    style: None,
                    match_indices: indices,
                }
            })
            .collect()
    }

    /// Complete history clear targets (r, shell, all).
    fn complete_history_clear_targets(&self, pos: usize, partial: &str) -> Vec<Suggestion> {
        self.complete_targets(
            pos,
            partial,
            &[
                ("r", "Clear R mode history"),
                ("shell", "Clear shell mode history"),
                ("all", "Clear all history"),
            ],
        )
    }

    /// Complete history browse targets (r, shell).
    fn complete_history_browse_targets(&self, pos: usize, partial: &str) -> Vec<Suggestion> {
        self.complete_targets(
            pos,
            partial,
            &[
                ("r", "Browse R mode history"),
                ("shell", "Browse shell mode history"),
            ],
        )
    }
}

impl Default for MetaCommandCompleter {
    fn default() -> Self {
        Self::new()
    }
}

impl Completer for MetaCommandCompleter {
    fn complete(&mut self, line: &str, pos: usize) -> Vec<Suggestion> {
        self.complete_commands(line, pos)
    }
}

/// Combined completer that delegates to the appropriate completer.
pub struct CombinedCompleter {
    r_completer: RCompleter,
    meta_completer: MetaCommandCompleter,
}

impl CombinedCompleter {
    /// Create a new CombinedCompleter with default settings (50ms timeout, 100ms debounce, 50 function check limit).
    pub fn new() -> Self {
        Self::with_settings(50, 100, 50)
    }

    /// Create a new CombinedCompleter with custom settings.
    pub fn with_settings(timeout_ms: u64, debounce_ms: u64, auto_paren_limit: usize) -> Self {
        Self::with_settings_and_rig(timeout_ms, debounce_ms, auto_paren_limit, true)
    }

    /// Create a new CombinedCompleter with custom settings and rig availability.
    ///
    /// When `rig_enabled` is false, the `:switch` command is excluded from completion.
    pub fn with_settings_and_rig(
        timeout_ms: u64,
        debounce_ms: u64,
        auto_paren_limit: usize,
        rig_enabled: bool,
    ) -> Self {
        // Build exclusion list: always exclude `:r` in R mode
        let mut exclusions: Vec<&'static str> = vec!["r"];

        // Exclude `:switch` when rig is not enabled
        if !rig_enabled {
            exclusions.push("switch");
        }

        CombinedCompleter {
            r_completer: RCompleter::with_settings(timeout_ms, debounce_ms, auto_paren_limit),
            meta_completer: MetaCommandCompleter::with_exclusions(exclusions),
        }
    }
}

impl Default for CombinedCompleter {
    fn default() -> Self {
        Self::new()
    }
}

impl Completer for CombinedCompleter {
    fn complete(&mut self, line: &str, pos: usize) -> Vec<Suggestion> {
        if line.trim_start().starts_with(':') {
            self.meta_completer.complete(line, pos)
        } else {
            self.r_completer.complete(line, pos)
        }
    }
}

/// Cache for completion results to avoid repeated R calls.
#[derive(Debug, Clone)]
struct CompletionCache {
    /// The token that was used for the R completion query.
    token: String,
    /// The full completion results from R.
    completions: Vec<String>,
    /// When the completion was fetched.
    timestamp: Instant,
}

/// R code completer using R's built-in completion functions.
pub struct RCompleter {
    /// Timeout in milliseconds for R completion (0 = no timeout).
    timeout_ms: u64,
    /// Debounce delay in milliseconds.
    debounce_ms: u64,
    /// Number of completions to check for function type (0 = disable).
    auto_paren_limit: usize,
    /// Cached completion results.
    cache: Option<CompletionCache>,
}

impl RCompleter {
    /// Create a new RCompleter with default settings (50ms timeout, 100ms debounce, 50 function check limit).
    pub fn new() -> Self {
        RCompleter {
            timeout_ms: 50,
            debounce_ms: 100,
            auto_paren_limit: 50,
            cache: None,
        }
    }

    /// Create a new RCompleter with custom settings.
    pub fn with_settings(timeout_ms: u64, debounce_ms: u64, auto_paren_limit: usize) -> Self {
        RCompleter {
            timeout_ms,
            debounce_ms,
            auto_paren_limit,
            cache: None,
        }
    }

    /// Check if the new token extends the cached token (prefix extension).
    fn is_prefix_extension(&self, new_token: &str) -> bool {
        if let Some(cache) = &self.cache {
            // New token must start with the cached token
            // e.g., "pri" extends "pr", but "po" does not
            new_token.starts_with(&cache.token) && new_token != cache.token
        } else {
            false
        }
    }

    /// Check if we should use cached results (within debounce window, same token).
    fn should_use_cache(&self, token: &str) -> bool {
        if let Some(cache) = &self.cache {
            // Don't use empty cache
            if cache.completions.is_empty() {
                return false;
            }

            // Don't use cache for empty tokens (context likely changed)
            if token.is_empty() || cache.token.is_empty() {
                return false;
            }

            // Don't use cache for package:: completions (always fetch fresh, worth waiting)
            if token.contains("::") || cache.token.contains("::") {
                return false;
            }

            // Use cache if:
            // 1. Same token and within debounce window
            // 2. Token is a prefix extension of cached token
            if cache.token == token {
                cache.timestamp.elapsed() < Duration::from_millis(self.debounce_ms)
            } else {
                self.is_prefix_extension(token)
            }
        } else {
            false
        }
    }

    /// Filter cached completions for a new token (prefix extension).
    fn filter_cached(&self, token: &str) -> Vec<String> {
        if let Some(cache) = &self.cache {
            cache
                .completions
                .iter()
                .filter(|c| c.starts_with(token))
                .cloned()
                .collect()
        } else {
            vec![]
        }
    }

    /// Invalidate the cache.
    fn invalidate_cache(&mut self) {
        self.cache = None;
    }
}

impl Default for RCompleter {
    fn default() -> Self {
        Self::new()
    }
}

impl Completer for RCompleter {
    fn complete(&mut self, line: &str, pos: usize) -> Vec<Suggestion> {
        // Check if we're inside a string - use Rust path completion with fuzzy matching
        // This provides better UX than R's built-in completion:
        // - Fuzzy matching: "rdm" matches "README.md"
        // - Faster: no R evaluation needed
        // - Hidden files shown last
        if let Some(ctx) = detect_string_context(line, pos) {
            return complete_path_in_string(line, pos, &ctx);
        }

        // Get the token being completed (for filtering and span calculation)
        let token = arf_harp::completion::get_token(line, pos).unwrap_or_default();

        // Try to use cache for prefix extensions or debounced requests
        let completions = if self.should_use_cache(&token) {
            // Use cached results, filtering for the current token
            self.filter_cached(&token)
        } else {
            // Fetch fresh completions from R
            let fresh = match arf_harp::completion::get_completions(line, pos, self.timeout_ms) {
                Ok(c) => c,
                Err(_) => {
                    // On error, invalidate cache and return empty
                    self.invalidate_cache();
                    return vec![];
                }
            };

            // Only cache non-empty results with non-empty tokens
            if !fresh.is_empty() && !token.is_empty() {
                self.cache = Some(CompletionCache {
                    token: token.clone(),
                    completions: fresh.clone(),
                    timestamp: Instant::now(),
                });
            }

            fresh
        };

        if completions.is_empty() {
            return vec![];
        }

        // Filter completions: include if starts with token and (different from token OR is a function)
        // This allows "foo" -> "foo(" completion when foo is a function
        let filtered: Vec<String> = completions
            .into_iter()
            .filter(|c| c.starts_with(&token))
            .collect();

        if filtered.is_empty() {
            return vec![];
        }

        // Determine which completions are functions (for parenthesis insertion)
        let is_function = if self.auto_paren_limit > 0 {
            self.check_function_types(&filtered)
        } else {
            vec![false; filtered.len()]
        };

        // Convert to reedline Suggestions
        let match_len = token.len();
        filtered
            .into_iter()
            .zip(is_function)
            .filter(|(c, is_func)| {
                // Include if: different from token, OR is a function (will get "(" added)
                c != &token || (*is_func && !has_special_suffix(c))
            })
            .map(|(c, is_func)| {
                let start = pos - token.len();
                let indices = if match_len > 0 {
                    Some((0..match_len).collect())
                } else {
                    None
                };

                // Add "()" suffix for functions
                // FunctionAwareMenu will move cursor back to place it inside: foo(|)
                let (value, extra_info) = if is_func && !has_special_suffix(&c) {
                    (format!("{}()", c), Some("function".to_string()))
                } else {
                    (c, None)
                };

                Suggestion {
                    value,
                    display_override: None,
                    description: extra_info,
                    extra: None,
                    span: Span { start, end: pos },
                    append_whitespace: false,
                    style: None,
                    match_indices: indices,
                }
            })
            .collect()
    }
}

impl RCompleter {
    /// Check which completions are functions.
    ///
    /// Only checks up to `auto_paren_limit` items for performance.
    fn check_function_types(&self, completions: &[String]) -> Vec<bool> {
        // Only check the first N items
        let check_count = completions.len().min(self.auto_paren_limit);

        if check_count == 0 {
            return vec![false; completions.len()];
        }

        // Collect names to check (exclude items with special suffixes)
        let names_to_check: Vec<&str> = completions[..check_count]
            .iter()
            .map(|s| {
                if has_special_suffix(s) {
                    // Don't check items with special suffixes
                    ""
                } else {
                    s.as_str()
                }
            })
            .collect();

        // Call R to check function types
        let checked = arf_harp::completion::check_if_functions(&names_to_check).unwrap_or_default();

        // Extend with false for remaining items (not checked)
        let mut result = checked;
        result.resize(completions.len(), false);
        result
    }
}

/// Check if a completion has a special suffix that means it shouldn't get parentheses.
fn has_special_suffix(s: &str) -> bool {
    // Package namespace (already has ::)
    s.ends_with("::")
        // Assignment operators
        || s.ends_with("<-")
        || s.ends_with("<<-")
        // Already has parentheses
        || s.ends_with("()")
        || s.ends_with('(')
        // File paths (R completion returns these for strings)
        || s.ends_with('/')
        // Argument names (completion inside function calls)
        || s.ends_with(" = ")
        || s.ends_with("=")
}

/// Result of string context detection.
#[derive(Debug, Clone, PartialEq)]
pub struct StringContext {
    /// The partial path/content being typed inside the string.
    pub content: String,
    /// Start position of the string content (after opening quote).
    pub start: usize,
    /// The quote character used ('"' or '\'').
    pub quote: char,
}

/// Parse R code using tree-sitter.
fn parse_r_code(code: &str) -> Option<Tree> {
    R_PARSER.with(|parser| parser.borrow_mut().parse(code.as_bytes(), None))
}

/// Find the deepest node at or before the given byte position.
fn find_node_at_position<'a>(tree: &'a Tree, pos: usize) -> Option<tree_sitter::Node<'a>> {
    let root = tree.root_node();
    let mut cursor = root.walk();
    let mut best_node = None;

    // Walk down to find the deepest node containing the position
    loop {
        let node = cursor.node();

        // Check if position is within or at the end of this node
        if pos >= node.start_byte() && pos <= node.end_byte() {
            best_node = Some(node);

            // Try to go deeper
            if cursor.goto_first_child() {
                // Find the child that contains the position
                loop {
                    let child = cursor.node();
                    if pos >= child.start_byte() && pos <= child.end_byte() {
                        break; // Found the child, will process it in next iteration
                    }
                    if !cursor.goto_next_sibling() {
                        // No more siblings, go back to parent
                        cursor.goto_parent();
                        return best_node;
                    }
                }
            } else {
                // No children, this is the deepest node
                return best_node;
            }
        } else {
            return best_node;
        }
    }
}

/// Check if a node is a string node or inside a string.
fn find_string_ancestor<'a>(node: tree_sitter::Node<'a>) -> Option<tree_sitter::Node<'a>> {
    let mut current = Some(node);
    while let Some(n) = current {
        if n.kind() == "string" {
            return Some(n);
        }
        current = n.parent();
    }
    None
}

/// Check if a node is an ERROR node that contains an incomplete string.
/// Returns the position of the opening quote if found.
fn find_incomplete_string_in_error<'a>(
    node: tree_sitter::Node<'a>,
    source: &str,
) -> Option<(usize, char)> {
    let mut current = Some(node);
    while let Some(n) = current {
        if n.kind() == "ERROR" {
            // Look for quote characters in the ERROR node's text
            let start = n.start_byte();
            let end = n.end_byte().min(source.len());
            let text = &source[start..end];

            // Find the last opening quote that doesn't have a matching close
            let mut in_double = false;
            let mut in_single = false;
            let mut last_double_pos = None;
            let mut last_single_pos = None;
            let mut skip_next = false;

            for (i, c) in text.char_indices() {
                if skip_next {
                    skip_next = false;
                    continue;
                }
                match c {
                    '\\' => {
                        // Skip next character (escape sequence)
                        skip_next = true;
                    }
                    '"' if !in_single => {
                        if in_double {
                            in_double = false;
                            last_double_pos = None;
                        } else {
                            in_double = true;
                            last_double_pos = Some(start + i);
                        }
                    }
                    '\'' if !in_double => {
                        if in_single {
                            in_single = false;
                            last_single_pos = None;
                        } else {
                            in_single = true;
                            last_single_pos = Some(start + i);
                        }
                    }
                    _ => {}
                }
            }

            // Return the unclosed quote position
            if in_double && let Some(pos) = last_double_pos {
                return Some((pos, '"'));
            }
            if in_single && let Some(pos) = last_single_pos {
                return Some((pos, '\''));
            }
        }
        current = n.parent();
    }
    None
}

/// Detect if cursor is inside a string literal using tree-sitter.
///
/// This uses tree-sitter-r for accurate parsing, correctly handling:
/// - Regular strings: "hello" or 'hello'
/// - Raw strings: r"(hello)" or R"(hello)"
/// - Escape sequences
/// - Comments (not detected as strings)
/// - Incomplete strings (handled via ERROR node analysis)
///
/// Returns `Some(StringContext)` if inside a string, `None` otherwise.
fn detect_string_context(line: &str, cursor_pos: usize) -> Option<StringContext> {
    // Parse the line
    let tree = parse_r_code(line)?;

    // Find the node at cursor position
    let node = find_node_at_position(&tree, cursor_pos)?;

    // First, check if we're inside a complete string
    if let Some(string_node) = find_string_ancestor(node) {
        // Get the string boundaries
        let string_start = string_node.start_byte();
        let string_end = string_node.end_byte();

        // Make sure cursor is actually inside the string (not at the closing quote)
        if cursor_pos < string_start || cursor_pos > string_end {
            return None;
        }

        // Extract the string content from the source
        let string_text = &line[string_start..string_end.min(line.len())];

        // Determine quote type and extract content
        let (quote_char, content_start_offset) = if string_text.starts_with("r\"")
            || string_text.starts_with("R\"")
            || string_text.starts_with("r'")
            || string_text.starts_with("R'")
        {
            // Raw string: r"(...)" - find the opening delimiter
            let quote = if string_text.contains('"') { '"' } else { '\'' };
            // Find position after r"( or similar
            let delim_end = string_text.find('(').map(|p| p + 1).unwrap_or(2);
            (quote, delim_end)
        } else if string_text.starts_with('"') {
            ('"', 1)
        } else if string_text.starts_with('\'') {
            ('\'', 1)
        } else {
            // Unknown string format
            return None;
        };

        // Calculate the absolute position where content starts
        let content_start = string_start + content_start_offset;

        // If cursor is before the content starts (in the opening quote/delimiter)
        if cursor_pos < content_start {
            return Some(StringContext {
                content: String::new(),
                start: content_start,
                quote: quote_char,
            });
        }

        // Extract content from content_start to cursor
        let content = if cursor_pos <= line.len() && content_start <= cursor_pos {
            line[content_start..cursor_pos].to_string()
        } else {
            String::new()
        };

        return Some(StringContext {
            content,
            start: content_start,
            quote: quote_char,
        });
    }

    // Check if we're in an ERROR node with an incomplete string
    if let Some((quote_pos, quote_char)) = find_incomplete_string_in_error(node, line) {
        // The content starts after the quote
        let content_start = quote_pos + 1;

        // Make sure cursor is after the quote
        if cursor_pos <= quote_pos {
            return None;
        }

        // Extract content from content_start to cursor
        let content = if cursor_pos <= line.len() && content_start <= cursor_pos {
            line[content_start..cursor_pos].to_string()
        } else {
            String::new()
        };

        return Some(StringContext {
            content,
            start: content_start,
            quote: quote_char,
        });
    }

    None
}

/// Complete paths using Rust-native path completion.
fn complete_path_in_string(_line: &str, pos: usize, ctx: &StringContext) -> Vec<Suggestion> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let options = PathCompletionOptions::default();

    let completions = complete_path(&ctx.content, &cwd, &options);

    completions
        .into_iter()
        .map(|c| {
            let match_indices = c.match_indices.map(|indices| {
                // Offset indices to account for quote position
                indices.into_iter().collect()
            });

            Suggestion {
                value: c.path,
                display_override: None,
                description: if c.is_dir {
                    Some("directory".to_string())
                } else {
                    None
                },
                extra: None,
                span: Span {
                    start: ctx.start,
                    end: pos,
                },
                append_whitespace: false,
                style: None,
                match_indices,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_meta_command_completer_empty_colon() {
        let mut completer = MetaCommandCompleter::new();
        let suggestions = completer.complete(":", 1);
        assert!(!suggestions.is_empty());
        let values: Vec<&str> = suggestions.iter().map(|s| s.value.as_str()).collect();
        assert!(values.contains(&"shell"));
        assert!(values.contains(&"r"));
        assert!(values.contains(&"system"));
        assert!(values.contains(&"reprex"));
        assert!(values.contains(&"commands"));
        assert!(values.contains(&"cmds"));
        assert!(values.contains(&"restart"));
        assert!(values.contains(&"switch"));
        assert!(values.contains(&"quit"));
        assert!(values.contains(&"exit"));
    }

    #[test]
    fn test_meta_command_completer_partial_command() {
        let mut completer = MetaCommandCompleter::new();
        let suggestions = completer.complete(":rep", 4);
        assert_eq!(suggestions.len(), 1);
        assert_eq!(suggestions[0].value, "reprex");
    }

    #[test]
    fn test_meta_command_completer_no_subcommands() {
        let mut completer = MetaCommandCompleter::new();
        // All commands have no subcommands
        let suggestions = completer.complete(":reprex ", 8);
        assert!(suggestions.is_empty());
        let suggestions = completer.complete(":commands ", 10);
        assert!(suggestions.is_empty());
    }

    #[test]
    fn test_meta_command_completer_not_meta_command() {
        let mut completer = MetaCommandCompleter::new();
        let suggestions = completer.complete("print(x)", 8);
        assert!(suggestions.is_empty());
    }

    #[test]
    fn test_meta_command_completer_has_descriptions() {
        let mut completer = MetaCommandCompleter::new();
        let suggestions = completer.complete(":", 1);
        let reprex = suggestions.iter().find(|s| s.value == "reprex").unwrap();
        assert!(reprex.description.is_some());
        assert!(reprex.description.as_ref().unwrap().contains("reprex"));
    }

    #[test]
    fn test_combined_completer_delegates_to_meta() {
        let mut completer = CombinedCompleter::new();
        let suggestions = completer.complete(":rep", 4);
        assert_eq!(suggestions.len(), 1);
        assert_eq!(suggestions[0].value, "reprex");
    }

    #[test]
    fn test_combined_completer_with_leading_whitespace() {
        let mut completer = CombinedCompleter::new();
        let suggestions = completer.complete("  :rep", 6);
        assert_eq!(suggestions.len(), 1);
        assert_eq!(suggestions[0].value, "reprex");
    }

    #[test]
    fn test_meta_command_completer_excludes_r_command() {
        // In R mode, `:r` should be excluded from completion
        let mut completer = MetaCommandCompleter::with_exclusions(vec!["r"]);
        let suggestions = completer.complete(":", 1);
        let values: Vec<&str> = suggestions.iter().map(|s| s.value.as_str()).collect();
        assert!(!values.contains(&"r"), "`:r` should be excluded in R mode");
        assert!(
            values.contains(&"shell"),
            "`:shell` should still be present"
        );
    }

    #[test]
    fn test_meta_command_completer_excludes_shell_mode_commands() {
        // In Shell mode, R-specific commands should be excluded from completion
        let mut completer = MetaCommandCompleter::with_exclusions(vec![
            "shell",
            "system",
            "autoformat",
            "format",
            "restart",
            "reprex",
            "switch",
            "h",
            "help",
        ]);
        let suggestions = completer.complete(":", 1);
        let values: Vec<&str> = suggestions.iter().map(|s| s.value.as_str()).collect();

        // These should be excluded in Shell mode
        assert!(!values.contains(&"shell"), "`:shell` should be excluded");
        assert!(!values.contains(&"system"), "`:system` should be excluded");
        assert!(
            !values.contains(&"autoformat"),
            "`:autoformat` should be excluded"
        );
        assert!(!values.contains(&"format"), "`:format` should be excluded");
        assert!(
            !values.contains(&"restart"),
            "`:restart` should be excluded"
        );
        assert!(!values.contains(&"reprex"), "`:reprex` should be excluded");
        assert!(!values.contains(&"switch"), "`:switch` should be excluded");
        assert!(!values.contains(&"h"), "`:h` should be excluded");
        assert!(!values.contains(&"help"), "`:help` should be excluded");

        // These should still be present in Shell mode
        assert!(values.contains(&"r"), "`:r` should be present");
        assert!(
            values.contains(&"commands"),
            "`:commands` should be present"
        );
        assert!(values.contains(&"quit"), "`:quit` should be present");
        assert!(values.contains(&"exit"), "`:exit` should be present");
    }

    #[test]
    fn test_meta_command_completer_exclusion_affects_partial_match() {
        // Even with partial match, excluded commands should not appear
        let mut completer = MetaCommandCompleter::with_exclusions(vec!["r"]);
        // Typing ":r" should not match excluded "r" command
        let suggestions = completer.complete(":r", 2);
        let values: Vec<&str> = suggestions.iter().map(|s| s.value.as_str()).collect();
        assert!(
            !values.contains(&"r"),
            "`:r` should not appear even with partial match"
        );
        // But "restart" and "reprex" should still appear
        assert!(values.contains(&"restart"));
        assert!(values.contains(&"reprex"));
    }

    #[test]
    fn test_combined_completer_excludes_r_command() {
        // CombinedCompleter is used in R mode, so `:r` should be excluded
        let mut completer = CombinedCompleter::new();
        let suggestions = completer.complete(":", 1);
        let values: Vec<&str> = suggestions.iter().map(|s| s.value.as_str()).collect();
        assert!(
            !values.contains(&"r"),
            "`:r` should be excluded in CombinedCompleter (R mode)"
        );
        assert!(
            values.contains(&"shell"),
            "`:shell` should be present in R mode"
        );
    }

    #[test]
    fn test_meta_command_append_whitespace_for_commands_with_arguments() {
        // Commands that take arguments should have append_whitespace: true
        let mut completer = MetaCommandCompleter::new();

        // :switch takes an argument (R version)
        let suggestions = completer.complete(":sw", 3);
        let switch = suggestions.iter().find(|s| s.value == "switch").unwrap();
        assert!(
            switch.append_whitespace,
            "`:switch` should append whitespace because it takes an argument"
        );

        // :system takes an argument (shell command)
        let suggestions = completer.complete(":sys", 4);
        let system = suggestions.iter().find(|s| s.value == "system").unwrap();
        assert!(
            system.append_whitespace,
            "`:system` should append whitespace because it takes an argument"
        );
    }

    #[test]
    fn test_meta_command_no_append_whitespace_for_commands_without_arguments() {
        // Commands that don't take arguments should have append_whitespace: false
        let mut completer = MetaCommandCompleter::new();

        let suggestions = completer.complete(":", 1);

        // Commands without arguments
        for cmd_name in &[
            "shell",
            "r",
            "reprex",
            "autoformat",
            "format",
            "commands",
            "cmds",
            "restart",
            "quit",
            "exit",
        ] {
            if let Some(cmd) = suggestions.iter().find(|s| s.value == *cmd_name) {
                assert!(
                    !cmd.append_whitespace,
                    "`:{}` should NOT append whitespace because it takes no argument",
                    cmd_name
                );
            }
        }
    }

    #[test]
    fn test_meta_command_match_indices_for_partial_command() {
        // When typing a partial command, match_indices should highlight the matched prefix
        let mut completer = MetaCommandCompleter::new();

        // Typing ":rep" should match "reprex" and highlight positions 0,1,2
        let suggestions = completer.complete(":rep", 4);
        assert_eq!(suggestions.len(), 1);
        let reprex = &suggestions[0];
        assert_eq!(reprex.value, "reprex");
        assert_eq!(reprex.match_indices, Some(vec![0, 1, 2]));
    }

    #[test]
    fn test_meta_command_match_indices_none_for_empty_input() {
        // When just ":" is typed, no prefix to highlight
        let mut completer = MetaCommandCompleter::new();

        let suggestions = completer.complete(":", 1);
        assert!(!suggestions.is_empty());
        for suggestion in &suggestions {
            assert_eq!(
                suggestion.match_indices, None,
                "`:` with no partial input should have match_indices: None"
            );
        }
    }

    #[test]
    fn test_meta_command_match_indices_single_char() {
        // Single character fuzzy match - 'r' matches at different positions in different commands
        let mut completer = MetaCommandCompleter::new();

        let suggestions = completer.complete(":r", 2);
        // With fuzzy matching, `:r` matches any command containing 'r':
        // - "r" at position 0
        // - "reprex" at position 0
        // - "restart" at position 0
        // - "autoformat" at position 6
        // - "format" at position 2
        let values: Vec<&str> = suggestions.iter().map(|s| s.value.as_str()).collect();
        assert!(values.contains(&"r"));
        assert!(values.contains(&"reprex"));
        assert!(values.contains(&"restart"));
        assert!(values.contains(&"autoformat"));
        assert!(values.contains(&"format"));

        // Verify match_indices are correct for each
        for suggestion in &suggestions {
            let expected_pos = suggestion
                .value
                .find('r')
                .or_else(|| suggestion.value.find('R'));
            assert_eq!(
                suggestion.match_indices,
                expected_pos.map(|p| vec![p]),
                "`:r` should highlight first 'r' position in `{}`",
                suggestion.value
            );
        }
    }

    // --- Fuzzy matching integration tests ---
    // Note: Direct fuzzy_match tests are in fuzzy.rs module

    #[test]
    fn test_meta_command_fuzzy_matching() {
        let mut completer = MetaCommandCompleter::new();

        // ":rst" should fuzzy match "restart"
        let suggestions = completer.complete(":rst", 4);
        let values: Vec<&str> = suggestions.iter().map(|s| s.value.as_str()).collect();
        assert!(
            values.contains(&"restart"),
            "`:rst` should fuzzy match `restart`, got: {:?}",
            values
        );

        // Check match_indices for fuzzy match
        let restart = suggestions.iter().find(|s| s.value == "restart").unwrap();
        assert_eq!(
            restart.match_indices,
            Some(vec![0, 2, 3]),
            "`:rst` should highlight positions 0, 2, 3 in `restart`"
        );
    }

    #[test]
    fn test_meta_command_fuzzy_matching_af_autoformat() {
        let mut completer = MetaCommandCompleter::new();

        // ":af" should fuzzy match "autoformat"
        let suggestions = completer.complete(":af", 3);
        let values: Vec<&str> = suggestions.iter().map(|s| s.value.as_str()).collect();
        assert!(
            values.contains(&"autoformat"),
            "`:af` should fuzzy match `autoformat`, got: {:?}",
            values
        );

        // Check match_indices
        let autoformat = suggestions
            .iter()
            .find(|s| s.value == "autoformat")
            .unwrap();
        assert_eq!(
            autoformat.match_indices,
            Some(vec![0, 4]),
            "`:af` should highlight positions 0, 4 in `autoformat`"
        );
    }

    #[test]
    fn test_meta_command_fuzzy_matching_cms_cmds() {
        let mut completer = MetaCommandCompleter::new();

        // ":cms" should match "cmds" - c=0, m=1, d=2, s=3
        let suggestions = completer.complete(":cms", 4);
        let values: Vec<&str> = suggestions.iter().map(|s| s.value.as_str()).collect();
        assert!(values.contains(&"cmds"), "`:cms` should fuzzy match `cmds`");

        let cmds = suggestions.iter().find(|s| s.value == "cmds").unwrap();
        assert_eq!(
            cmds.match_indices,
            Some(vec![0, 1, 3]),
            "`:cms` should highlight positions 0, 1, 3 in `cmds`"
        );
    }

    #[test]
    fn test_meta_command_fuzzy_no_match() {
        let mut completer = MetaCommandCompleter::new();

        // ":xyz" should not match any command
        let suggestions = completer.complete(":xyz", 4);
        assert!(
            suggestions.is_empty(),
            "`:xyz` should not match any command"
        );
    }

    #[test]
    fn test_meta_command_help_no_append_whitespace() {
        // :help and :h should NOT append whitespace after completion
        // because they open an interactive help browser (no argument needed)
        let mut completer = MetaCommandCompleter::new();

        let suggestions = completer.complete(":hel", 4);
        let help = suggestions.iter().find(|s| s.value == "help").unwrap();
        assert!(
            !help.append_whitespace,
            "`:help` should NOT append whitespace"
        );

        let suggestions = completer.complete(":h", 2);
        let h = suggestions.iter().find(|s| s.value == "h").unwrap();
        assert!(!h.append_whitespace, "`:h` should NOT append whitespace");
    }

    // --- String context detection tests ---

    #[test]
    fn test_detect_string_context_double_quote() {
        // Inside double-quoted string
        let ctx = detect_string_context(r#"read.csv("data/"#, 15);
        assert!(ctx.is_some());
        let ctx = ctx.unwrap();
        assert_eq!(ctx.content, "data/");
        assert_eq!(ctx.quote, '"');
    }

    #[test]
    fn test_detect_string_context_single_quote() {
        // Inside single-quoted string
        let ctx = detect_string_context("source('script", 14);
        assert!(ctx.is_some());
        let ctx = ctx.unwrap();
        assert_eq!(ctx.content, "script");
        assert_eq!(ctx.quote, '\'');
    }

    #[test]
    fn test_detect_string_context_not_in_string() {
        // Not inside a string
        let ctx = detect_string_context("print(x)", 7);
        assert!(ctx.is_none());

        // After closing quote
        let ctx = detect_string_context(r#"read.csv("data.csv")"#, 20);
        assert!(ctx.is_none());
    }

    #[test]
    fn test_detect_string_context_with_escaped_quotes() {
        // Escaped quote should not close the string
        let ctx = detect_string_context(r#"paste("hello \"world"#, 20);
        assert!(ctx.is_some());
        let ctx = ctx.unwrap();
        assert_eq!(ctx.content, r#"hello \"world"#);
    }

    #[test]
    fn test_detect_string_context_empty_string() {
        // Empty string (just opened quote)
        let ctx = detect_string_context(r#"read.csv(""#, 10);
        assert!(ctx.is_some());
        let ctx = ctx.unwrap();
        assert_eq!(ctx.content, "");
        assert_eq!(ctx.start, 10);
    }

    #[test]
    fn test_detect_string_context_tilde_path() {
        // Tilde path
        let ctx = detect_string_context(r#"setwd("~/"#, 9);
        assert!(ctx.is_some());
        let ctx = ctx.unwrap();
        assert_eq!(ctx.content, "~/");
    }

    #[test]
    fn test_detect_string_context_absolute_path() {
        // Absolute path
        let ctx = detect_string_context(r#"source("/usr/local/lib/"#, 23);
        assert!(ctx.is_some());
        let ctx = ctx.unwrap();
        assert_eq!(ctx.content, "/usr/local/lib/");
    }

    #[test]
    fn test_detect_string_context_complete_string_cursor_inside() {
        // Cursor inside a complete string
        let ctx = detect_string_context(r#"read.csv("data.csv")"#, 14);
        assert!(ctx.is_some());
        let ctx = ctx.unwrap();
        assert_eq!(ctx.content, "data");
    }

    #[test]
    fn test_detect_string_context_in_comment() {
        // Quotes inside comments should NOT be detected as strings
        // This is one of the key benefits of using tree-sitter
        let ctx = detect_string_context(r#"# "data/"#, 8);
        assert!(ctx.is_none(), "Should not detect string inside comment");
    }

    #[test]
    fn test_detect_string_context_raw_string() {
        // R 4.0+ raw strings: r"(content)"
        // Inside a complete raw string at position 11 (after "hel")
        // x <- r"(hello)"
        // 0    5 78901234
        let ctx = detect_string_context(r#"x <- r"(hello)""#, 11);
        assert!(ctx.is_some(), "Should detect raw string");
        let ctx = ctx.unwrap();
        assert_eq!(ctx.content, "hel");
        assert_eq!(ctx.start, 8); // Content starts after r"(
    }
}
