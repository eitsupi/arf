//! Parsing of the `pkg::name(` call shape handled by static completion.

use super::StaticFormalsRequest;

pub(super) fn parse_request(line: &str, cursor_pos: usize) -> Option<StaticFormalsRequest> {
    let before_cursor = line.get(..cursor_pos.min(line.len()))?;
    let operator = before_cursor.rfind("::")?;
    // `rfind("::")` also finds the final pair in `:::`. Reject both triple
    // colon forms and a colon immediately before the selected operator.
    if before_cursor[..operator].ends_with(':') {
        return None;
    }

    let package_end = operator;
    let package_start = before_cursor[..package_end]
        .char_indices()
        .rev()
        .find_map(|(index, character)| {
            (!is_identifier_character(character)).then_some(index + character.len_utf8())
        })
        .unwrap_or(0);
    // The provider intentionally accepts only a top-level qualified call. This
    // avoids guessing through a path, nested call, or other expression.
    if !before_cursor[..package_start].trim().is_empty() {
        return None;
    }
    let package = &before_cursor[package_start..package_end];
    if !is_valid_identifier(package) {
        return None;
    }

    let function_start = operator + 2;
    let function_end = before_cursor[function_start..]
        .find('(')
        .map(|offset| function_start + offset)?;
    let function = &before_cursor[function_start..function_end];
    if !is_valid_identifier(function) {
        return None;
    }

    let arguments = &before_cursor[function_end + 1..];
    // Splitting on commas is only safe without nested expressions. Keep even
    // balanced brackets and braces on the R path rather than tracking depth.
    if arguments.chars().any(|character| {
        matches!(
            character,
            '(' | ')' | '[' | ']' | '{' | '}' | '"' | '\'' | '#'
        )
    }) {
        return None;
    }
    let (completed_arguments, current_argument) = arguments
        .rsplit_once(',')
        .map_or(("", arguments), |(completed, current)| (completed, current));
    let partial = current_argument.trim();
    if !partial.is_empty() && !is_valid_identifier(partial) {
        return None;
    }
    let used_named = completed_arguments
        .split(',')
        .filter_map(named_argument_name)
        .collect();

    Some(StaticFormalsRequest {
        package: package.to_owned(),
        exported_name: function.to_owned(),
        partial: partial.to_owned(),
        used_named,
    })
}

fn is_identifier_character(character: char) -> bool {
    character.is_ascii_alphanumeric() || matches!(character, '.' | '_')
}

pub(super) fn is_valid_identifier(identifier: &str) -> bool {
    let mut characters = identifier.chars();
    let Some(first) = characters.next() else {
        return false;
    };
    if !first.is_ascii_alphabetic() && first != '.' {
        return false;
    }
    if first == '.'
        && characters
            .clone()
            .next()
            .is_some_and(|character| character.is_ascii_digit())
    {
        return false;
    }
    characters.all(is_identifier_character) && !is_reserved_identifier(identifier)
}

fn is_reserved_identifier(identifier: &str) -> bool {
    if identifier == "..."
        || identifier
            .strip_prefix("..")
            .is_some_and(|suffix| !suffix.is_empty() && suffix.chars().all(|c| c.is_ascii_digit()))
    {
        return true;
    }
    matches!(
        identifier,
        "if" | "else"
            | "repeat"
            | "while"
            | "function"
            | "for"
            | "in"
            | "next"
            | "break"
            | "TRUE"
            | "FALSE"
            | "NULL"
            | "Inf"
            | "NaN"
            | "NA"
            | "NA_integer_"
            | "NA_real_"
            | "NA_complex_"
            | "NA_character_"
    )
}

/// Return a named argument's name when the segment contains a top-level,
/// standalone `=` separator. Comparison operators are expressions, not named
/// arguments, and must not affect the set of already-used formals.
pub(super) fn named_argument_name(argument: &str) -> Option<String> {
    let mut depth = 0usize;
    for (index, character) in argument.char_indices() {
        match character {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth = depth.saturating_sub(1),
            '=' if depth == 0 => {
                let previous = argument[..index].chars().next_back();
                let next = argument[index + character.len_utf8()..].chars().next();
                if previous.is_some_and(|value| matches!(value, '=' | '!' | '<' | '>'))
                    || next.is_some_and(|value| matches!(value, '=' | '<' | '>'))
                {
                    continue;
                }
                let name = argument[..index].trim();
                if is_valid_identifier(name) {
                    return Some(name.to_owned());
                }
            }
            _ => {}
        }
    }
    None
}
