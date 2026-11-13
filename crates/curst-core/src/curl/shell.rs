//! splits a command line into words the way a POSIX shell does, for the
//! quoting curl commands are copied with: `'…'`, `"…"`, `$'…'` and a
//! backslash before a newline to continue the line
//!
//! nothing is expanded or run: `$HOME` stays the text `$HOME`

use std::iter::Peekable;
use std::str::Chars;

/// why a command line could not be split
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ShellError {
    /// a quote was opened and never closed
    #[error("a {0} quote is never closed")]
    Unclosed(&'static str),
}

/// the words of `text`
///
/// # Errors
/// when a quote is not closed
pub fn split(text: &str) -> Result<Vec<String>, ShellError> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut in_word = false;
    let mut chars = text.chars().peekable();

    while let Some(c) = chars.next() {
        match c {
            c if c.is_whitespace() => {
                if in_word {
                    words.push(std::mem::take(&mut word));
                    in_word = false;
                }
            }
            '\\' => match chars.next() {
                // a line continued on the next one, or a last backslash
                Some('\n') | None => {}
                Some('\r') => {
                    chars.next_if_eq(&'\n');
                }
                Some(next) => {
                    word.push(next);
                    in_word = true;
                }
            },
            '\'' => {
                single(&mut chars, &mut word)?;
                in_word = true;
            }
            '"' => {
                double(&mut chars, &mut word)?;
                in_word = true;
            }
            '$' if chars.peek() == Some(&'\'') => {
                chars.next();
                ansi_c(&mut chars, &mut word)?;
                in_word = true;
            }
            c => {
                word.push(c);
                in_word = true;
            }
        }
    }

    if in_word {
        words.push(word);
    }

    Ok(words)
}

/// `'…'`: everything up to the next quote, as written
fn single(chars: &mut Peekable<Chars>, word: &mut String) -> Result<(), ShellError> {
    for c in chars.by_ref() {
        if c == '\'' {
            return Ok(());
        }

        word.push(c);
    }

    Err(ShellError::Unclosed("single"))
}

/// `"…"`: a backslash only escapes `"`, `\`, `$`, `` ` `` and a newline
fn double(chars: &mut Peekable<Chars>, word: &mut String) -> Result<(), ShellError> {
    while let Some(c) = chars.next() {
        match c {
            '"' => return Ok(()),
            '\\' => match chars.next() {
                Some('\n') => {}
                Some(next @ ('"' | '\\' | '$' | '`')) => word.push(next),
                Some(next) => {
                    word.push('\\');
                    word.push(next);
                }
                None => break,
            },
            c => word.push(c),
        }
    }

    Err(ShellError::Unclosed("double"))
}

/// `$'…'`: backslash escapes as in C, such as `\n` and `\x41`
fn ansi_c(chars: &mut Peekable<Chars>, word: &mut String) -> Result<(), ShellError> {
    while let Some(c) = chars.next() {
        match c {
            '\'' => return Ok(()),
            '\\' => {
                let Some(next) = chars.next() else {
                    break;
                };

                match next {
                    'n' => word.push('\n'),
                    't' => word.push('\t'),
                    'r' => word.push('\r'),
                    'e' | 'E' => word.push('\u{1b}'),
                    '0' => word.push('\0'),
                    'x' => push_code(chars, word, 2),
                    'u' => push_code(chars, word, 4),
                    'U' => push_code(chars, word, 8),
                    other => word.push(other),
                }
            }
            c => word.push(c),
        }
    }

    Err(ShellError::Unclosed("$'"))
}

/// up to `most` hex digits, as the code of one character
fn push_code(chars: &mut Peekable<Chars>, word: &mut String, most: usize) {
    let mut digits = String::new();

    while digits.len() < most {
        match chars.next_if(char::is_ascii_hexdigit) {
            Some(d) => digits.push(d),
            None => break,
        }
    }

    let code = u32::from_str_radix(&digits, 16)
        .ok()
        .and_then(char::from_u32);

    match code {
        Some(c) => word.push(c),
        None => word.push_str(&digits),
    }
}

/// `word` quoted for a POSIX shell, so [`split`] gives it back as it is
#[must_use]
pub fn quote(word: &str) -> String {
    let plain = !word.is_empty()
        && word
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_./:=@%+,".contains(&b));

    if plain {
        return word.to_owned();
    }

    format!("'{}'", word.replace('\'', r"'\''"))
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    #[test]
    fn quotes_and_continuations_are_understood() {
        let text = "curl -H 'A: b c' \\\n  -d \"x \\\"y\\\" $z\" $'line\\none\\x41' plain\\ word";

        assert_eq!(
            split(text).unwrap(),
            [
                "curl",
                "-H",
                "A: b c",
                "-d",
                "x \"y\" $z",
                "line\noneA",
                "plain word"
            ]
        );
        assert_eq!(split("a''b \"\"").unwrap(), ["ab", ""]);
        assert_eq!(split("a \\\r\n b").unwrap(), ["a", "b"]);
        assert_eq!(split("'open"), Err(ShellError::Unclosed("single")));
        assert_eq!(split("\"open"), Err(ShellError::Unclosed("double")));
        assert_eq!(split("$'open"), Err(ShellError::Unclosed("$'")));
    }

    proptest! {
        #[test]
        fn a_quoted_word_comes_back_whole(word in "\\PC*") {
            prop_assert_eq!(split(&quote(&word)).unwrap(), vec![word]);
        }
    }
}
