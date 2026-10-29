//! Bash: walks the tree-sitter-bash syntax tree and hands every simple command to the
//! classifier.

use acs_core::gate::TargetOp;
use tree_sitter::{Node, Parser};

use crate::args::Outcome;
use crate::ctx::Ctx;
use crate::word::{Cwd, Kind, Redirect, Word};

/// How deep syntax nesting may go before the rest is given up on.
const MAX_DEPTH: usize = 200;

/// Analyzes `src` as bash, appending to `ctx`.
pub(crate) fn run(src: &str, ctx: &mut Ctx) {
    let mut parser = Parser::new();
    if parser
        .set_language(&tree_sitter_bash::LANGUAGE.into())
        .is_err()
    {
        ctx.unparsed(src);
        return;
    }
    let Some(tree) = parser.parse(src, None) else {
        ctx.unparsed(src);
        return;
    };
    let root = tree.root_node();
    let found = ctx.out.len();
    let mut walker = Walker {
        ctx,
        src,
        pending: Vec::new(),
        depth: 0,
    };
    walker.walk(root);
    if root.has_error() {
        ctx.complete = false;
        if ctx.out.len() == found {
            ctx.unparsed(src);
        }
    }
}

/// Text accumulated while building one word.
#[derive(Default)]
struct Acc {
    /// The value after quote removal.
    text: String,
    /// The characters that were unquoted and unescaped (where globs can occur).
    unquoted: String,
    partial: bool,
    opaque: bool,
}

impl Acc {
    fn finish(self) -> Word {
        let kind = if self.opaque {
            Kind::Opaque
        } else if self.partial || (self.unquoted.starts_with('~') && self.text.starts_with('~')) {
            Kind::Partial
        } else if has_glob(&self.unquoted) {
            Kind::Pattern
        } else {
            Kind::Exact
        };
        Word {
            text: self.text,
            kind,
        }
    }

    /// Appends text that is neither quoted nor escaped except for backslashes.
    fn unquoted(&mut self, s: &str) {
        let mut chars = s.chars();
        while let Some(c) = chars.next() {
            if c == '\\' {
                match chars.next() {
                    Some('\n') | None => {}
                    Some(escaped) => self.text.push(escaped),
                }
            } else {
                self.text.push(c);
                self.unquoted.push(c);
            }
        }
    }

    /// Appends the content of a double-quoted string (only some escapes are special).
    fn double_quoted(&mut self, s: &str) {
        let mut chars = s.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '\\' {
                match chars.peek() {
                    Some('"' | '\\' | '$' | '`') => {
                        if let Some(escaped) = chars.next() {
                            self.text.push(escaped);
                        }
                    }
                    Some('\n') => {
                        chars.next();
                    }
                    _ => self.text.push('\\'),
                }
            } else {
                self.text.push(c);
            }
        }
    }

    /// Appends the decoded content of `$'...'`.
    fn ansi_c(&mut self, s: &str) {
        let mut chars = s.chars().peekable();
        while let Some(c) = chars.next() {
            if c != '\\' {
                self.text.push(c);
                continue;
            }
            match chars.next() {
                Some('n') => self.text.push('\n'),
                Some('t') => self.text.push('\t'),
                Some('r') => self.text.push('\r'),
                Some('a') => self.text.push('\u{7}'),
                Some('b') => self.text.push('\u{8}'),
                Some('f') => self.text.push('\u{c}'),
                Some('v') => self.text.push('\u{b}'),
                Some('e' | 'E') => self.text.push('\u{1b}'),
                Some('x') => {
                    let mut value = 0u32;
                    for _ in 0..2 {
                        match chars.peek().and_then(|d| d.to_digit(16)) {
                            Some(d) => {
                                value = value * 16 + d;
                                chars.next();
                            }
                            None => break,
                        }
                    }
                    self.text.push(char::from_u32(value).unwrap_or('\u{fffd}'));
                }
                Some(d @ '0'..='7') => {
                    let mut value = d.to_digit(8).unwrap_or(0);
                    for _ in 0..2 {
                        match chars.peek().and_then(|d| d.to_digit(8)) {
                            Some(d) => {
                                value = value * 8 + d;
                                chars.next();
                            }
                            None => break,
                        }
                    }
                    self.text.push(char::from_u32(value).unwrap_or('\u{fffd}'));
                }
                Some(other) => self.text.push(other),
                None => self.text.push('\\'),
            }
        }
    }
}

/// Whether unquoted text contains a glob: `*`, `?`, a bracket expression or a brace expansion.
fn has_glob(unquoted: &str) -> bool {
    if unquoted.contains(['*', '?']) {
        return true;
    }
    if let Some(open) = unquoted.find('[')
        && unquoted[open..].contains(']')
    {
        return true;
    }
    if let Some(open) = unquoted.find('{')
        && let Some(close) = unquoted[open..].find('}')
    {
        let inside = &unquoted[open + 1..open + close];
        return inside.contains(',') || inside.contains("..");
    }
    false
}

struct Walker<'a> {
    ctx: &'a mut Ctx,
    src: &'a str,
    /// Redirections of an enclosing compound statement, to be attached to the next command.
    pending: Vec<Redirect>,
    depth: usize,
}

impl<'a, 't> Walker<'a> {
    fn text(&self, node: Node<'t>) -> &'a str {
        self.src.get(node.byte_range()).unwrap_or("")
    }

    /// Walks a compound statement; a directory change inside it makes later directories
    /// unknown because the change may not have happened.
    fn conditional(&mut self, node: Node<'t>) {
        let before = self.ctx.cwd.clone();
        self.children(node);
        if self.ctx.cwd != before {
            self.ctx.cwd = Cwd::Unknown;
        }
    }

    fn children(&mut self, node: Node<'t>) {
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            self.walk(child);
        }
    }

    fn walk(&mut self, node: Node<'t>) {
        if self.depth >= MAX_DEPTH {
            self.ctx.complete = false;
            return;
        }
        self.depth += 1;
        match node.kind() {
            "comment" | "heredoc_start" | "heredoc_end" | "heredoc_content" => {}
            "command" => self.command(node),
            "declaration_command" | "unset_command" => self.declaration(node),
            "redirected_statement" => self.redirected(node),
            "list" => self.list(node),
            "pipeline" => {
                let saved = self.ctx.cwd.clone();
                let mut cursor = node.walk();
                let operands: Vec<Node<'t>> = node
                    .named_children(&mut cursor)
                    .filter(|n| n.kind() != "comment")
                    .collect();
                self.operands(&operands);
                self.ctx.cwd = saved;
            }
            "subshell"
            | "command_substitution"
            | "process_substitution"
            | "function_definition" => {
                let saved = self.ctx.cwd.clone();
                self.children(node);
                self.ctx.cwd = saved;
            }
            "if_statement"
            | "while_statement"
            | "for_statement"
            | "c_style_for_statement"
            | "case_statement" => self.conditional(node),
            _ => self.children(node),
        }
        self.depth -= 1;
    }

    /// `a && b`, `a || b`: the right side of `||` runs only when the left failed.
    fn list(&mut self, node: Node<'t>) {
        let mut cursor = node.walk();
        let operands: Vec<Node<'t>> = node
            .named_children(&mut cursor)
            .filter(|n| n.kind() != "comment")
            .collect();
        let is_or = {
            let mut c = node.walk();
            node.children(&mut c)
                .any(|child| !child.is_named() && child.kind() == "||")
        };
        match (operands.as_slice(), is_or) {
            ([left, right], true) => {
                let before = self.ctx.cwd.clone();
                let mut held = std::mem::take(&mut self.pending);
                self.walk(*left);
                let after_left = std::mem::replace(&mut self.ctx.cwd, before.clone());
                self.pending.append(&mut held);
                self.walk(*right);
                self.ctx.cwd = if self.ctx.cwd == before {
                    after_left
                } else {
                    Cwd::Unknown
                };
            }
            _ => self.operands(&operands),
        }
    }

    /// Walks the operands of a list or pipeline. A redirection that applies to the whole
    /// statement (`a && b > f`, `a | b > f`) belongs to the last operand.
    fn operands(&mut self, operands: &[Node<'t>]) {
        let mut held = std::mem::take(&mut self.pending);
        for (index, operand) in operands.iter().enumerate() {
            if index + 1 == operands.len() {
                self.pending.append(&mut held);
            }
            self.walk(*operand);
        }
    }

    // -----------------------------------------------------------------------------------------
    // Words
    // -----------------------------------------------------------------------------------------

    fn word(&self, node: Node<'t>, subs: &mut Vec<Node<'t>>) -> Word {
        let mut acc = Acc::default();
        self.add(node, &mut acc, subs);
        acc.finish()
    }

    fn add(&self, node: Node<'t>, acc: &mut Acc, subs: &mut Vec<Node<'t>>) {
        match node.kind() {
            "command_name" => match node.named_child(0) {
                Some(inner) => self.add(inner, acc, subs),
                None => acc.unquoted(self.text(node)),
            },
            "string" | "translated_string" => self.add_string(node, acc, subs),
            "raw_string" => {
                let text = self.text(node);
                let inner = text.strip_prefix('\'').unwrap_or(text);
                acc.text.push_str(inner.strip_suffix('\'').unwrap_or(inner));
            }
            "ansi_c_string" => {
                let text = self.text(node);
                let inner = text.strip_prefix("$'").unwrap_or(text);
                acc.ansi_c(inner.strip_suffix('\'').unwrap_or(inner));
            }
            "concatenation" => {
                let mut cursor = node.walk();
                for child in node.children(&mut cursor) {
                    self.add(child, acc, subs);
                }
            }
            "simple_expansion" | "expansion" | "arithmetic_expansion" | "special_variable_name" => {
                acc.partial = true;
                acc.text.push_str(self.text(node));
            }
            "command_substitution" => {
                acc.partial = true;
                acc.text.push_str(self.text(node));
                subs.push(node);
            }
            "process_substitution" => {
                acc.opaque = true;
                acc.text.push_str(self.text(node));
                subs.push(node);
            }
            _ => acc.unquoted(self.text(node)),
        }
    }

    fn add_string(&self, node: Node<'t>, acc: &mut Acc, subs: &mut Vec<Node<'t>>) {
        let text = self.text(node);
        let content_start = node.start_byte() + text.find('"').map_or(0, |at| at + 1);
        let content_end = if text.ends_with('"') && text.len() > 1 {
            node.end_byte() - 1
        } else {
            node.end_byte()
        };
        let mut cursor_at = content_start;
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if child.kind() == "\"" {
                continue;
            }
            if child.start_byte() > cursor_at {
                acc.double_quoted(self.src.get(cursor_at..child.start_byte()).unwrap_or(""));
            }
            match child.kind() {
                "string_content" => acc.double_quoted(self.text(child)),
                _ => self.add(child, acc, subs),
            }
            cursor_at = cursor_at.max(child.end_byte());
        }
        if cursor_at < content_end {
            acc.double_quoted(self.src.get(cursor_at..content_end).unwrap_or(""));
        }
    }

    /// Finds command and process substitutions below `node` without descending into them.
    fn collect_subs(node: Node<'t>, subs: &mut Vec<Node<'t>>) {
        if matches!(node.kind(), "command_substitution" | "process_substitution") {
            subs.push(node);
            return;
        }
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            Self::collect_subs(child, subs);
        }
    }

    // -----------------------------------------------------------------------------------------
    // Commands
    // -----------------------------------------------------------------------------------------

    fn command(&mut self, node: Node<'t>) {
        let mut words = Vec::new();
        let mut subs = Vec::new();
        let mut redirects = std::mem::take(&mut self.pending);
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            match child.kind() {
                "variable_assignment" | "herestring_redirect" | "heredoc_redirect" => {
                    Self::collect_subs(child, &mut subs);
                }
                "file_redirect" => self.redirect(child, &mut redirects, &mut subs),
                "comment" => {}
                _ if child.is_named() => words.push(self.word(child, &mut subs)),
                _ => {}
            }
        }
        // The parser invents an empty command name for a dangling operator (`rm x &&`).
        let missing = words.first().is_some_and(|w| w.text.is_empty()) && redirects.is_empty();
        if !missing {
            self.ctx.command(words, redirects, false);
        }
        for sub in subs {
            self.walk(sub);
        }
    }

    /// `export A=1`, `local x`, `declare -r y`, `unset z`: no effect on files.
    fn declaration(&mut self, node: Node<'t>) {
        let mut words = Vec::new();
        let mut subs = Vec::new();
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if child.is_named() {
                if child.kind() == "variable_assignment" {
                    Self::collect_subs(child, &mut subs);
                    words.push(Word::opaque(self.text(child)));
                } else {
                    words.push(self.word(child, &mut subs));
                }
            } else if words.is_empty() && child.kind().chars().all(|c| c.is_ascii_alphabetic()) {
                words.push(Word::exact(child.kind()));
            }
        }
        let redirects = std::mem::take(&mut self.pending);
        self.ctx.push(&words, Outcome::read(), redirects);
        for sub in subs {
            self.walk(sub);
        }
    }

    fn redirect(&self, node: Node<'t>, redirects: &mut Vec<Redirect>, subs: &mut Vec<Node<'t>>) {
        let mut op = "";
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if !child.is_named() && child.kind().contains(['<', '>']) {
                op = child.kind();
                break;
            }
        }
        let Some(destination) = node.child_by_field_name("destination") else {
            return;
        };
        let word = self.word(destination, subs);
        let target_op = match op {
            "<" => TargetOp::Read,
            ">&" if destination.kind() == "number" || word.text == "-" => return,
            "<&" => return,
            _ => TargetOp::Write,
        };
        redirects.push(Redirect {
            op: target_op,
            word,
        });
    }

    /// A statement with redirections: they belong to the first command of its body.
    fn redirected(&mut self, node: Node<'t>) {
        let body = node.child_by_field_name("body");
        let mut redirects = Vec::new();
        let mut subs = Vec::new();
        let mut trailing = Vec::new();
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if body.is_some_and(|b| b.id() == child.id()) {
                continue;
            }
            match child.kind() {
                "file_redirect" => self.redirect(child, &mut redirects, &mut subs),
                "herestring_redirect" => Self::collect_subs(child, &mut subs),
                "heredoc_redirect" => {
                    let mut inner = child.walk();
                    for part in child.children(&mut inner) {
                        match part.kind() {
                            "file_redirect" => self.redirect(part, &mut redirects, &mut subs),
                            "heredoc_body" => Self::collect_subs(part, &mut subs),
                            _ if part.is_named() => trailing.push(part),
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
        }
        self.pending.extend(redirects);
        if let Some(body) = body {
            self.walk(body);
        }
        if !self.pending.is_empty() {
            let leftover = std::mem::take(&mut self.pending);
            self.ctx.command(Vec::new(), leftover, false);
        }
        for sub in subs {
            self.walk(sub);
        }
        for part in trailing {
            self.walk(part);
        }
    }
}
