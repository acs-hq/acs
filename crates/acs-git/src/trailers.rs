//! Commit-message trailers (`Key: value` lines in the last paragraph of a message).
//!
//! The rules are git's own (`git interpret-trailers`), re-implemented so that the
//! `prepare-commit-msg` hook can edit the message file in-process, without spawning `git`:
//!
//! - trailers live in the **last paragraph** of the message, after a blank line, and never in the
//!   first paragraph (the subject);
//! - a paragraph is a trailer block when all of its lines are trailers (`Token: value`, with
//!   indented continuation lines), or when at least a quarter of them are and one is a
//!   recognised one (`Signed-off-by: `, `(cherry picked from commit `);
//! - comment lines (starting with the comment character, `#` by default) are ignored, and the
//!   trailing comment block of an editor template stays last: trailers go in before it;
//! - everything after a scissors line (`# ------------------------ >8 ------------------------`)
//!   is not part of the message;
//! - a trailer that is already there (same token ignoring case, same value) is not added again.
//!
//! An empty message (the editor template of a plain `git commit`) gets two blank lines and then
//! the trailers, so the first line stays free for the subject. If the person then saves without
//! typing a subject, git does not treat the message as empty because of the trailer; this is the
//! same behaviour as `git commit --trailer`.

use std::fs;
use std::io;
use std::path::Path;

/// The token of the session trailer (SPEC section 7).
pub const SESSION_TOKEN: &str = "ACS-Session";

/// The comment character git uses unless `core.commentChar` says otherwise.
pub const DEFAULT_COMMENT_CHAR: char = '#';

/// One `token: value` trailer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Trailer {
    /// The token, for example `ACS-Session`.
    pub token: String,
    /// The value, trimmed.
    pub value: String,
}

impl Trailer {
    /// A trailer with the given token and value.
    #[must_use]
    pub fn new(token: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            token: token.into(),
            value: value.into(),
        }
    }

    /// The `ACS-Session: <session_id>` trailer.
    #[must_use]
    pub fn session(session_id: &str) -> Self {
        Self::new(SESSION_TOKEN, session_id)
    }

    fn line(&self) -> String {
        format!("{}: {}", self.token, self.value)
    }

    fn same_as(&self, other: &Self) -> bool {
        self.token.eq_ignore_ascii_case(&other.token) && self.value == other.value
    }
}

/// Where the parts of a message are, as indexes into its lines.
struct Layout {
    /// The message proper: lines before the trailing comments, blank lines and scissors tail.
    body_end: usize,
    /// First line of the last paragraph of the body, when the body has more than one paragraph
    /// and that paragraph is a trailer block.
    block_start: Option<usize>,
}

fn content(line: &str) -> &str {
    line.trim_end_matches(['\n', '\r'])
}

fn is_comment(line: &str, comment: char) -> bool {
    content(line).starts_with(comment)
}

fn is_blank(line: &str) -> bool {
    line.trim().is_empty()
}

fn is_scissors(line: &str, comment: char) -> bool {
    content(line) == format!("{comment} ------------------------ >8 ------------------------")
}

/// The token of a trailer line, when it is one: non-empty, made of letters, digits, `-` and `_`,
/// then optional spaces, then `:`.
fn split_trailer(line: &str) -> Option<(&str, &str)> {
    let line = content(line);
    let colon = line.find(':')?;
    let token = line[..colon].trim_end();
    let valid = !token.is_empty()
        && token
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    valid.then(|| (token, line[colon + 1..].trim()))
}

fn is_recognised_prefix(line: &str) -> bool {
    line.starts_with("Signed-off-by: ") || line.starts_with("(cherry picked from commit ")
}

/// Whether `paragraph` is a trailer block by git's rule.
fn is_trailer_block(paragraph: &[&str], comment: char) -> bool {
    let (mut trailers, mut others, mut recognised) = (0usize, 0usize, false);
    let mut previous_was_trailer = false;
    for line in paragraph {
        if is_comment(line, comment) {
            continue;
        }
        if line.starts_with([' ', '\t']) {
            if !previous_was_trailer {
                others += 1;
            }
        } else if split_trailer(line).is_some() || is_recognised_prefix(line) {
            trailers += 1;
            recognised |= is_recognised_prefix(line);
            previous_was_trailer = true;
        } else {
            others += 1;
            previous_was_trailer = false;
        }
    }
    (trailers > 0 && others == 0) || (recognised && trailers * 3 >= others)
}

fn layout(lines: &[&str], comment: char) -> (usize, Layout) {
    let cut = lines
        .iter()
        .position(|l| is_scissors(l, comment))
        .unwrap_or(lines.len());
    let body_end = lines[..cut]
        .iter()
        .rposition(|l| !is_blank(l) && !is_comment(l, comment))
        .map_or(0, |i| i + 1);
    let body = &lines[..body_end];
    let block_start = body.iter().rposition(|l| is_blank(l)).and_then(|blank| {
        let start = blank + 1;
        (start < body.len() && is_trailer_block(&body[start..], comment)).then_some(start)
    });
    (
        cut,
        Layout {
            body_end,
            block_start,
        },
    )
}

/// The trailers in the message's trailer block, in order. Empty when there is none.
#[must_use]
pub fn parse_trailers(message: &str, comment: char) -> Vec<Trailer> {
    let lines: Vec<&str> = message.split_inclusive('\n').collect();
    let (_, found) = layout(&lines, comment);
    let Some(start) = found.block_start else {
        return Vec::new();
    };
    lines[start..found.body_end]
        .iter()
        .filter(|l| !is_comment(l, comment))
        .filter_map(|l| split_trailer(l).map(|(t, v)| Trailer::new(t, v)))
        .collect()
}

/// The values of the `ACS-Session` trailers in the message's trailer block.
#[must_use]
pub fn session_ids(message: &str, comment: char) -> Vec<String> {
    parse_trailers(message, comment)
        .into_iter()
        .filter(|t| t.token.eq_ignore_ascii_case(SESSION_TOKEN))
        .map(|t| t.value)
        .collect()
}

/// Adds `trailers` to `message` and returns the new text, or `None` when every one of them is
/// already there (nothing to write). Trailers given twice are added once.
#[must_use]
pub fn add_trailers(message: &str, trailers: &[Trailer], comment: char) -> Option<String> {
    let lines: Vec<&str> = message.split_inclusive('\n').collect();
    let eol = if lines.first().is_some_and(|l| l.ends_with("\r\n")) {
        "\r\n"
    } else {
        "\n"
    };
    let (_, found) = layout(&lines, comment);
    let body = &lines[..found.body_end];

    // What is already in the message: the trailer block, and any exact line in the body (a
    // squash message carries the trailers of the commits it squashes outside the last block).
    let mut present: Vec<Trailer> = parse_trailers(message, comment);
    present.extend(
        body.iter()
            .filter_map(|l| split_trailer(l).map(|(t, v)| Trailer::new(t, v)))
            .filter(|t| t.token.eq_ignore_ascii_case(SESSION_TOKEN)),
    );
    let mut fresh: Vec<&Trailer> = Vec::new();
    for trailer in trailers {
        if !present.iter().any(|p| p.same_as(trailer)) && !fresh.iter().any(|f| f.same_as(trailer))
        {
            fresh.push(trailer);
        }
    }
    if fresh.is_empty() {
        return None;
    }

    let mut out = String::with_capacity(message.len() + 64);
    for line in body {
        out.push_str(line);
    }
    if !body.is_empty() {
        if !out.ends_with('\n') {
            out.push_str(eol);
        }
        if found.block_start.is_none() {
            out.push_str(eol);
        }
    } else {
        out.push_str(eol);
        out.push_str(eol);
    }
    for trailer in fresh {
        out.push_str(&trailer.line());
        out.push_str(eol);
    }
    for line in &lines[found.body_end..] {
        out.push_str(line);
    }
    Some(out)
}

/// Adds `trailers` to the message file at `path` (git's `COMMIT_EDITMSG`). Returns whether the
/// file changed. A file that is not UTF-8 is left alone.
///
/// # Errors
///
/// The I/O error of reading or writing the file.
pub fn add_to_file(path: &Path, trailers: &[Trailer], comment: char) -> io::Result<bool> {
    let Ok(text) = String::from_utf8(fs::read(path)?) else {
        return Ok(false);
    };
    match add_trailers(&text, trailers, comment) {
        Some(new) => {
            fs::write(path, new)?;
            Ok(true)
        }
        None => Ok(false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn add(message: &str) -> String {
        add_trailers(message, &[Trailer::session("ses_1")], '#').unwrap_or_default()
    }

    #[test]
    fn subject_only_gets_a_new_block() {
        assert_eq!(add("fix\n"), "fix\n\nACS-Session: ses_1\n");
        assert_eq!(add("fix"), "fix\n\nACS-Session: ses_1\n");
    }

    #[test]
    fn body_without_trailers_gets_a_new_block() {
        assert_eq!(
            add("fix\n\nwhy it broke\n"),
            "fix\n\nwhy it broke\n\nACS-Session: ses_1\n"
        );
    }

    #[test]
    fn an_existing_block_is_extended() {
        assert_eq!(
            add("fix\n\nbody\n\nSigned-off-by: A <a@x>\n"),
            "fix\n\nbody\n\nSigned-off-by: A <a@x>\nACS-Session: ses_1\n"
        );
    }

    #[test]
    fn the_subject_is_not_a_trailer_block() {
        assert_eq!(add("fix: it\n"), "fix: it\n\nACS-Session: ses_1\n");
        assert_eq!(
            add("fix: it\n\nFixes: #3\n"),
            "fix: it\n\nFixes: #3\nACS-Session: ses_1\n"
        );
    }

    #[test]
    fn a_duplicate_is_not_added() {
        assert_eq!(
            add_trailers(
                "x\n\nACS-Session: ses_1\n",
                &[Trailer::session("ses_1")],
                '#'
            ),
            None
        );
        assert_eq!(
            add_trailers(
                "x\n\nacs-session: ses_1\n",
                &[Trailer::session("ses_1")],
                '#'
            ),
            None
        );
        let two = add_trailers(
            "x\n\nACS-Session: ses_1\n",
            &[
                Trailer::session("ses_1"),
                Trailer::session("ses_2"),
                Trailer::session("ses_2"),
            ],
            '#',
        );
        assert_eq!(
            two.as_deref(),
            Some("x\n\nACS-Session: ses_1\nACS-Session: ses_2\n")
        );
    }

    #[test]
    fn editor_comments_stay_last() {
        let template = "fix\n\nbody\n\n# Please enter the commit message\n# more\n";
        assert_eq!(
            add(template),
            "fix\n\nbody\n\nACS-Session: ses_1\n\n# Please enter the commit message\n# more\n"
        );
        let with_block = "fix\n\nbody\n\nFixes: #1\n\n# comment\n";
        assert_eq!(
            add(with_block),
            "fix\n\nbody\n\nFixes: #1\nACS-Session: ses_1\n\n# comment\n"
        );
    }

    #[test]
    fn an_empty_message_keeps_the_subject_line_free() {
        assert_eq!(
            add("\n# Please enter\n"),
            "\n\nACS-Session: ses_1\n\n# Please enter\n"
        );
        assert_eq!(add(""), "\n\nACS-Session: ses_1\n");
    }

    #[test]
    fn nothing_after_a_scissors_line_is_touched() {
        let message = "fix\n\n# ------------------------ >8 ------------------------\n# Do not modify\ndiff --git a b\n";
        assert_eq!(
            add(message),
            "fix\n\nACS-Session: ses_1\n\n# ------------------------ >8 ------------------------\n# Do not modify\ndiff --git a b\n"
        );
    }

    #[test]
    fn continuation_lines_belong_to_their_trailer() {
        let message = "x\n\nFixes: a\n  continued\n";
        assert_eq!(
            add(message),
            "x\n\nFixes: a\n  continued\nACS-Session: ses_1\n"
        );
    }

    #[test]
    fn a_mixed_paragraph_is_not_a_block_unless_recognised() {
        assert_eq!(
            add("x\n\nsome prose\nFixes: 1\n"),
            "x\n\nsome prose\nFixes: 1\n\nACS-Session: ses_1\n"
        );
        assert_eq!(
            add("x\n\nsome prose\nSigned-off-by: A <a@x>\n"),
            "x\n\nsome prose\nSigned-off-by: A <a@x>\nACS-Session: ses_1\n"
        );
    }

    #[test]
    fn crlf_messages_keep_their_line_endings() {
        assert_eq!(add("fix\r\n"), "fix\r\n\r\nACS-Session: ses_1\r\n");
    }

    #[test]
    fn a_custom_comment_character_is_respected() {
        let out = add_trailers("fix\n\n; note\n", &[Trailer::session("ses_1")], ';');
        assert_eq!(
            out.as_deref(),
            Some("fix\n\nACS-Session: ses_1\n\n; note\n")
        );
    }

    #[test]
    fn parse_reads_the_last_block_only() {
        let message = "x\n\nACS-Session: ses_a\n\nbody\n\nFixes: 1\nACS-Session: ses_b\n";
        assert_eq!(session_ids(message, '#'), vec!["ses_b"]);
        assert!(session_ids("just a subject\n", '#').is_empty());
    }

    #[test]
    fn file_helper_writes_only_on_change() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("MSG");
        fs::write(&path, "fix\n").unwrap();
        assert!(add_to_file(&path, &[Trailer::session("ses_1")], '#').unwrap());
        assert!(!add_to_file(&path, &[Trailer::session("ses_1")], '#').unwrap());
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "fix\n\nACS-Session: ses_1\n"
        );
        fs::write(&path, [0xff, 0xfe]).unwrap();
        assert!(!add_to_file(&path, &[Trailer::session("ses_1")], '#').unwrap());
    }
}
