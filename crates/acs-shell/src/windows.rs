//! PowerShell and cmd.exe: a tokenizer and a handful of heuristics for the commands that
//! delete, move or write files. Anything not understood is unknown.

use acs_core::gate::{CommandClass, TargetOp};

use crate::args::{Chdir, Outcome};
use crate::ctx::Ctx;
use crate::word::{Kind, Redirect, Word, basename, dirname};

/// One statement: a command with its arguments, redirections and script blocks.
#[derive(Default)]
struct Statement {
    words: Vec<Word>,
    redirects: Vec<Redirect>,
    /// Script blocks (`{ ... }`) found among the arguments.
    blocks: Vec<String>,
    /// Whether the statement receives pipeline input.
    piped: bool,
    /// Whether the first word was a quoted string (a value, not a command).
    first_quoted: bool,
    /// Whether the statement began with the call operator `&`.
    call_op: bool,
}

/// Splits a command line into statements.
struct Lexer {
    chars: Vec<char>,
    at: usize,
    powershell: bool,
    /// Cleared when quotes or brackets are unbalanced.
    ok: bool,
    /// Whether the word read last began with a quote.
    last_quoted: bool,
}

impl Lexer {
    fn new(src: &str, powershell: bool) -> Self {
        Self {
            chars: src.chars().collect(),
            at: 0,
            powershell,
            ok: true,
            last_quoted: false,
        }
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.at).copied()
    }

    fn peek_at(&self, offset: usize) -> Option<char> {
        self.chars.get(self.at + offset).copied()
    }

    fn statements(mut self) -> (Vec<Statement>, bool) {
        let mut statements = Vec::new();
        let mut current = Statement::default();
        // `Some(op)` after a redirection operator: the next word is its file.
        let mut redirect: Option<TargetOp> = None;
        while let Some(c) = self.peek() {
            match c {
                ' ' | '\t' | '\r' => self.at += 1,
                '`' if self.powershell => self.at += 2,
                '\n' | ';' => {
                    self.at += 1;
                    Self::finish(&mut statements, &mut current, false);
                }
                '|' => {
                    self.at += 1;
                    let double = self.peek() == Some('|');
                    if double {
                        self.at += 1;
                    }
                    Self::finish(&mut statements, &mut current, !double);
                }
                '&' => {
                    self.at += 1;
                    if self.peek() == Some('&') {
                        self.at += 1;
                        Self::finish(&mut statements, &mut current, false);
                    } else if !self.powershell {
                        Self::finish(&mut statements, &mut current, false);
                    }
                    // In PowerShell a lone `&` is the call operator: the next word is the command.
                    if self.powershell {
                        current.call_op = true;
                    }
                }
                '#' if self.powershell && self.at_word_start() => {
                    while self.peek().is_some_and(|c| c != '\n') {
                        self.at += 1;
                    }
                }
                '{' if self.powershell => match self.balanced('{', '}') {
                    Some(block) => {
                        current.blocks.push(block);
                        current.words.push(Word::opaque("{...}"));
                    }
                    None => self.ok = false,
                },
                '(' | ')' if !self.powershell => self.at += 1,
                _ => {
                    if let Some(op) = self.redirect_operator() {
                        redirect = op;
                        continue;
                    }
                    let Some(word) = self.word() else {
                        continue;
                    };
                    match redirect.take() {
                        Some(_) if self.discards(&word) => {}
                        Some(op) => current.redirects.push(Redirect { op, word }),
                        None => {
                            if current.words.is_empty() {
                                current.first_quoted = self.last_quoted;
                            }
                            current.words.push(word);
                        }
                    }
                }
            }
        }
        Self::finish(&mut statements, &mut current, false);
        (statements, self.ok)
    }

    /// Whether a redirection target throws the output away (`$null`, `nul`).
    fn discards(&self, word: &Word) -> bool {
        let text = word.text.to_ascii_lowercase();
        (self.powershell && text == "$null") || (!self.powershell && text == "nul")
    }

    fn at_word_start(&self) -> bool {
        self.at == 0
            || self
                .chars
                .get(self.at - 1)
                .is_none_or(|c| c.is_whitespace() || *c == ';')
    }

    /// Consumes a redirection operator (`>`, `>>`, `2>`, `*>`, `<`, `2>&1`). The outer option is
    /// `None` when there is no operator here; the inner one is `None` when the operator names a
    /// descriptor instead of a file.
    fn redirect_operator(&mut self) -> Option<Option<TargetOp>> {
        let mut look = 0;
        if self.peek().is_some_and(|c| c.is_ascii_digit() || c == '*') {
            look = 1;
        }
        let op = self.peek_at(look)?;
        if op != '>' && op != '<' {
            return None;
        }
        self.at += look + 1;
        if op == '<' {
            return Some(Some(TargetOp::Read));
        }
        if self.peek() == Some('>') {
            self.at += 1;
        }
        if self.peek() == Some('&')
            && self
                .peek_at(1)
                .is_some_and(|c| c.is_ascii_digit() || c == '-')
        {
            self.at += 2;
            return Some(None);
        }
        Some(Some(TargetOp::Write))
    }

    fn finish(out: &mut Vec<Statement>, current: &mut Statement, piped_next: bool) {
        let statement = std::mem::take(current);
        if !statement.words.is_empty()
            || !statement.redirects.is_empty()
            || !statement.blocks.is_empty()
        {
            out.push(statement);
        }
        current.piped = piped_next;
    }

    /// Reads text up to the matching close bracket and returns what is inside.
    fn balanced(&mut self, open: char, close: char) -> Option<String> {
        let mut depth = 0;
        let start = self.at + 1;
        while let Some(c) = self.peek() {
            self.at += 1;
            if c == open {
                depth += 1;
            } else if c == close {
                depth -= 1;
                if depth == 0 {
                    return Some(self.chars[start..self.at - 1].iter().collect());
                }
            } else if c == '\'' || c == '"' {
                while let Some(q) = self.peek() {
                    self.at += 1;
                    if q == c {
                        break;
                    }
                }
            }
        }
        None
    }

    /// Reads one word with quotes removed.
    fn word(&mut self) -> Option<Word> {
        let mut text = String::new();
        let mut partial = false;
        let mut glob = false;
        let mut started = false;
        self.last_quoted = matches!(self.peek(), Some('\'' | '"'));
        while let Some(c) = self.peek() {
            match c {
                ' ' | '\t' | '\r' | '\n' | ';' | '|' | '&' | '>' | '<' => break,
                '\'' if self.powershell => {
                    started = true;
                    self.at += 1;
                    loop {
                        match self.peek() {
                            Some('\'') => {
                                self.at += 1;
                                if self.peek() == Some('\'') {
                                    text.push('\'');
                                    self.at += 1;
                                } else {
                                    break;
                                }
                            }
                            Some(q) => {
                                text.push(q);
                                self.at += 1;
                            }
                            None => {
                                self.ok = false;
                                break;
                            }
                        }
                    }
                }
                '"' => {
                    started = true;
                    self.at += 1;
                    loop {
                        match self.peek() {
                            Some('"') => {
                                self.at += 1;
                                if self.powershell && self.peek() == Some('"') {
                                    text.push('"');
                                    self.at += 1;
                                } else {
                                    break;
                                }
                            }
                            Some('`') if self.powershell => {
                                self.at += 1;
                                if let Some(escaped) = self.peek() {
                                    text.push(escaped);
                                    self.at += 1;
                                }
                            }
                            Some(q) => {
                                if (q == '$' && self.powershell) || (q == '%' && !self.powershell) {
                                    partial = true;
                                }
                                text.push(q);
                                self.at += 1;
                            }
                            None => {
                                self.ok = false;
                                break;
                            }
                        }
                    }
                }
                '`' | '^' if (c == '`') == self.powershell => {
                    self.at += 1;
                    if let Some(escaped) = self.peek() {
                        text.push(escaped);
                        self.at += 1;
                    }
                    started = true;
                }
                '(' if self.powershell => {
                    // A sub-expression: the value is unknown.
                    partial = true;
                    let before = self.at;
                    if self.balanced('(', ')').is_none() {
                        self.ok = false;
                        return None;
                    }
                    text.extend(self.chars[before..self.at].iter());
                    started = true;
                }
                '$' if self.powershell => {
                    partial = true;
                    text.push(c);
                    self.at += 1;
                    started = true;
                }
                '%' if !self.powershell => {
                    partial = true;
                    text.push(c);
                    self.at += 1;
                    started = true;
                }
                '*' | '?' | '[' if c != '[' || self.powershell => {
                    glob = true;
                    text.push(c);
                    self.at += 1;
                    started = true;
                }
                '{' | '}' | ')' if self.powershell && !started => {
                    self.at += 1;
                    return None;
                }
                _ => {
                    text.push(c);
                    self.at += 1;
                    started = true;
                }
            }
        }
        if !started {
            return None;
        }
        if !self.powershell && text.matches('!').count() >= 2 {
            partial = true;
        }
        let kind = if partial || (text.starts_with('~') && self.powershell) {
            Kind::Partial
        } else if glob {
            Kind::Pattern
        } else {
            Kind::Exact
        };
        Some(Word { text, kind })
    }
}

// ---------------------------------------------------------------------------------------------
// PowerShell
// ---------------------------------------------------------------------------------------------

/// Analyzes `src` as PowerShell, appending to `ctx`.
pub(crate) fn powershell(src: &str, ctx: &mut Ctx) {
    let (statements, ok) = Lexer::new(src, true).statements();
    if !ok {
        ctx.complete = false;
        if statements.is_empty() {
            ctx.unparsed(src);
        }
    }
    for statement in statements {
        ps_statement(statement, ctx);
    }
}

/// A parsed cmdlet parameter list.
struct Params {
    named: Vec<(String, Option<Word>)>,
    positional: Vec<Word>,
}

impl Params {
    /// Parses `-Name value`, `-Name:value` and switches. `valued` lists the parameters that take
    /// a value; abbreviations match by prefix.
    fn new(args: &[Word], valued: &[&str]) -> Self {
        let mut named = Vec::new();
        let mut positional = Vec::new();
        let mut i = 0;
        while i < args.len() {
            let arg = &args[i];
            i += 1;
            let is_param = arg.kind != Kind::Opaque
                && arg.text.len() > 1
                && arg.text.starts_with('-')
                && arg.text[1..].starts_with(|c: char| c.is_ascii_alphabetic());
            if !is_param {
                positional.push(arg.clone());
                continue;
            }
            let body = &arg.text[1..];
            if let Some((name, value)) = body.split_once(':') {
                if matches!(value.to_ascii_lowercase().as_str(), "$false" | "0") {
                    // `-Recurse:$false` turns a switch off.
                    continue;
                }
                let kind = if value.contains('$') {
                    Kind::Partial
                } else {
                    arg.kind
                };
                named.push((
                    name.to_ascii_lowercase(),
                    Some(Word {
                        text: value.to_string(),
                        kind,
                    }),
                ));
                continue;
            }
            let name = body.to_ascii_lowercase();
            let takes_value = valued.iter().any(|v| v.starts_with(name.as_str()));
            if takes_value && i < args.len() {
                named.push((name, Some(args[i].clone())));
                i += 1;
            } else {
                named.push((name, None));
            }
        }
        Self { named, positional }
    }

    /// Whether the (possibly abbreviated) parameter was given.
    fn has(&self, full: &str) -> bool {
        self.named
            .iter()
            .any(|(name, _)| !name.is_empty() && full.starts_with(name.as_str()))
    }

    fn value(&self, full: &str) -> Option<&Word> {
        self.named
            .iter()
            .rev()
            .find(|(name, _)| !name.is_empty() && full.starts_with(name.as_str()))
            .and_then(|(_, value)| value.as_ref())
    }
}

/// Splits `a,b,c` (a PowerShell array literal) into words.
fn split_list(word: &Word) -> Vec<Word> {
    if word.kind == Kind::Opaque || !word.text.contains(',') {
        return vec![word.clone()];
    }
    word.text
        .split(',')
        .map(|part| Word {
            text: part.trim().to_string(),
            kind: word.kind,
        })
        .collect()
}

fn ps_paths(params: &Params, names: &[&str], positional_from: usize, count: usize) -> Vec<Word> {
    for name in names {
        if let Some(value) = params.value(name) {
            return split_list(value);
        }
    }
    params
        .positional
        .iter()
        .skip(positional_from)
        .take(count)
        .flat_map(split_list)
        .collect()
}

const PS_VALUED: &[&str] = &[
    "path",
    "literalpath",
    "pspath",
    "include",
    "exclude",
    "filter",
    "erroraction",
    "credential",
    "stream",
    "destination",
    "name",
    "value",
    "newname",
    "itemtype",
    "filepath",
    "encoding",
    "width",
    "inputobject",
    "uri",
    "outfile",
    "method",
    "body",
    "headers",
    "contenttype",
    "tail",
    "totalcount",
    "first",
    "last",
    "depth",
    "pattern",
    "workingdirectory",
    "argumentlist",
];

/// Cmdlets whose script blocks are expressions, not commands.
const EXPRESSION_BLOCKS: &[&str] = &[
    "where-object",
    "?",
    "sort-object",
    "select-object",
    "group-object",
    "measure-object",
];

/// Keywords that introduce blocks but are not commands themselves.
const PS_KEYWORDS: &[&str] = &[
    "if", "elseif", "else", "foreach", "for", "while", "do", "until", "switch", "try", "catch",
    "finally", "function", "filter", "param", "return", "throw", "begin", "process", "end", "trap",
    "class", "using",
];

fn ps_statement(statement: Statement, ctx: &mut Ctx) {
    let Statement {
        mut words,
        redirects,
        blocks,
        piped,
        first_quoted,
        call_op,
    } = statement;
    // `$x = <command>`: the command is what matters.
    if words.first().is_some_and(|w| w.text.starts_with('$'))
        && words
            .get(1)
            .is_some_and(|w| matches!(w.text.as_str(), "=" | "+=" | "-="))
    {
        words.drain(..2);
    }
    let first_lower = words.first().map(|w| w.text.to_ascii_lowercase());
    let analyze_blocks = !first_lower
        .as_deref()
        .is_some_and(|f| EXPRESSION_BLOCKS.contains(&f));
    let value = !call_op
        && (first_quoted
            || words.first().is_some_and(|w| {
                w.text.starts_with(['$', '-']) || w.text.starts_with(|c: char| c.is_ascii_digit())
            }));
    // `(Get-Item x).Delete()`, `[IO.File]::Delete(...)`: method calls we cannot follow.
    let method_call = !call_op
        && words
            .first()
            .is_some_and(|w| w.text.starts_with(['(', '[']));
    let keyword = first_lower
        .as_deref()
        .is_some_and(|f| PS_KEYWORDS.contains(&f));
    if method_call {
        ctx.push(&words, Outcome::unknown(), redirects);
    } else if words.is_empty() || value || keyword {
        // A value, a control keyword or only blocks: nothing runs here but the blocks.
        if !redirects.is_empty() {
            ctx.command(Vec::new(), redirects, true);
        }
    } else {
        ps_command(words, redirects, piped, ctx);
    }
    if analyze_blocks {
        for block in &blocks {
            ctx.nested(crate::Dialect::PowerShell, block);
        }
    }
}

fn ps_command(words: Vec<Word>, redirects: Vec<Redirect>, piped: bool, ctx: &mut Ctx) {
    let Some(first) = words.first() else {
        return;
    };
    if !first.is_exact() && first.text != "?" {
        ctx.push(&words, Outcome::unknown(), redirects);
        return;
    }
    let name = first.text.to_ascii_lowercase();
    let params = Params::new(&words[1..], PS_VALUED);
    let mut out = Outcome::default();
    match name.as_str() {
        "remove-item" | "ri" | "rm" | "del" | "erase" | "rd" | "rmdir" => {
            if params.has("whatif") {
                out.class(CommandClass::Read);
            } else {
                out.class(CommandClass::Delete);
                let paths = ps_paths(&params, &["path", "literalpath", "pspath"], 0, usize::MAX);
                if paths.is_empty() {
                    out.unknown_targets();
                }
                let recursive = params.has("recurse");
                out.targets(paths.iter(), TargetOp::Delete, recursive);
                if piped && paths.is_empty() {
                    out.unknown_targets();
                }
            }
        }
        "move-item" | "mi" | "move" | "mv" => {
            out.class(CommandClass::Move);
            let src = ps_paths(&params, &["path", "literalpath"], 0, 1);
            let dest = params.value("destination").cloned().or_else(|| {
                params
                    .positional
                    .get(if params.value("path").is_some() { 0 } else { 1 })
                    .cloned()
            });
            if src.is_empty() {
                out.unknown_targets();
            }
            out.targets(src.iter(), TargetOp::Delete, true);
            match dest {
                Some(dest) => out.target(&dest, TargetOp::Write, true),
                None => out.unknown_targets(),
            };
        }
        "copy-item" | "cpi" | "cp" | "copy" => {
            out.class(CommandClass::Write);
            let src = ps_paths(&params, &["path", "literalpath"], 0, 1);
            let dest = params.value("destination").cloned().or_else(|| {
                params
                    .positional
                    .get(if params.value("path").is_some() { 0 } else { 1 })
                    .cloned()
            });
            out.targets(src.iter(), TargetOp::Read, params.has("recurse"));
            match dest {
                Some(dest) => out.target(&dest, TargetOp::Write, true),
                None => out.unknown_targets(),
            };
        }
        "set-content" | "sc" | "add-content" | "ac" | "out-file" | "clear-content" | "clc"
        | "tee-object" | "export-csv" => {
            out.class(CommandClass::Write);
            let paths = ps_paths(&params, &["filepath", "path", "literalpath"], 0, 1);
            if paths.is_empty() {
                out.unknown_targets();
            }
            out.targets(paths.iter(), TargetOp::Write, false);
        }
        "new-item" | "ni" | "mkdir" | "md" => {
            out.class(CommandClass::Write);
            let paths = ps_paths(&params, &["path", "literalpath"], 0, usize::MAX);
            match (params.value("name"), paths.first()) {
                (Some(name), Some(base)) if base.is_exact() && name.is_exact() => {
                    out.path(
                        &format!("{}/{}", base.text.trim_end_matches(['/', '\\']), name.text),
                        Kind::Exact,
                        TargetOp::Write,
                        false,
                    );
                }
                (Some(name), None) => {
                    out.target(name, TargetOp::Write, false);
                }
                _ => {
                    if paths.is_empty() {
                        out.unknown_targets();
                    }
                    out.targets(paths.iter(), TargetOp::Write, false);
                }
            }
        }
        "rename-item" | "rni" | "ren" => {
            out.class(CommandClass::Move);
            let src = ps_paths(&params, &["path", "literalpath"], 0, 1);
            let new_name = params.value("newname").cloned().or_else(|| {
                params
                    .positional
                    .get(usize::from(params.value("path").is_none()))
                    .cloned()
            });
            out.targets(src.iter(), TargetOp::Delete, true);
            match (src.first(), new_name) {
                (Some(src), Some(new)) if src.is_exact() && new.is_exact() => {
                    let dir = dirname(&src.text);
                    let dest = if dir == "." {
                        new.text.clone()
                    } else {
                        format!("{dir}/{}", new.text)
                    };
                    out.path(&dest, Kind::Exact, TargetOp::Write, true);
                }
                _ => {
                    out.unknown_targets();
                }
            }
        }
        "invoke-webrequest" | "iwr" | "invoke-restmethod" | "irm" | "curl" | "wget" => {
            out.class(CommandClass::Network);
            if let Some(file) = params.value("outfile") {
                out.class(CommandClass::Write);
                out.target(file, TargetOp::Write, false);
            }
        }
        "set-location" | "sl" | "cd" | "chdir" => {
            out.class(CommandClass::Read);
            let dir = params
                .value("path")
                .or_else(|| params.value("literalpath"))
                .or(params.positional.first());
            out.chdir = Some(Chdir::To(dir.cloned()));
        }
        "push-location" | "pushd" | "pop-location" | "popd" => {
            out.class(CommandClass::Read);
            out.chdir = Some(Chdir::To(None));
        }
        "invoke-expression" | "iex" | "start-process" | "saps" | "start" | "invoke-command"
        | "icm" | "invoke-item" | "ii" => {
            out.and_unknown();
        }
        "echo" | "write-host" | "write-output" | "write" | "write-error" | "write-warning"
        | "write-verbose" | "out-null" | "out-host" | "out-string" | "cls" | "clear"
        | "clear-host" | "foreach-object" | "%" | "where-object" | "?" | "foreach"
        | "start-sleep" | "sleep" | "pwd" | "hostname" | "whoami" => {
            out.class(CommandClass::Read);
        }
        _ if name.starts_with("get-")
            || name.starts_with("select-")
            || name.starts_with("format-")
            || name.starts_with("test-")
            || name.starts_with("measure-")
            || name.starts_with("sort-")
            || name.starts_with("group-")
            || name.starts_with("convertto-")
            || name.starts_with("convertfrom-")
            || name.starts_with("compare-")
            || name.starts_with("resolve-")
            || name.starts_with("import-module") =>
        {
            out.class(CommandClass::Read);
            if matches!(
                name.as_str(),
                "get-content" | "get-childitem" | "get-item" | "select-string"
            ) {
                let paths = ps_paths(
                    &params,
                    &["path", "literalpath"],
                    usize::from(name == "select-string"),
                    usize::MAX,
                );
                out.targets(paths.iter(), TargetOp::Read, params.has("recurse"));
            }
        }
        "cat" | "type" | "gc" | "ls" | "dir" | "gci" | "gi" | "sls" => {
            out.class(CommandClass::Read);
            let paths = ps_paths(
                &params,
                &["path", "literalpath"],
                usize::from(name == "sls"),
                usize::MAX,
            );
            out.targets(paths.iter(), TargetOp::Read, params.has("recurse"));
        }
        _ if name.contains('-')
            && !name.contains('.')
            && !name.contains('\\')
            && !name.contains('/') =>
        {
            // An unknown cmdlet (`Verb-Noun`).
            out.and_unknown();
        }
        _ => {
            ctx.command(words, redirects, true);
            return;
        }
    }
    ctx.push(&words, out, redirects);
}

// ---------------------------------------------------------------------------------------------
// cmd.exe
// ---------------------------------------------------------------------------------------------

/// Analyzes `src` as a cmd.exe command line, appending to `ctx`.
pub(crate) fn cmd(src: &str, ctx: &mut Ctx) {
    let (statements, ok) = Lexer::new(src, false).statements();
    if !ok {
        ctx.complete = false;
        if statements.is_empty() {
            ctx.unparsed(src);
        }
    }
    for statement in statements {
        cmd_statement(statement, ctx);
    }
}

/// Splits switches (`/s`, `/q`) from paths, lowercasing the switches.
fn cmd_args(args: &[Word]) -> (Vec<String>, Vec<Word>) {
    let mut switches = Vec::new();
    let mut paths = Vec::new();
    for arg in args {
        if arg.is_exact()
            && arg.text.len() > 1
            && arg.text.starts_with('/')
            && !arg.text[1..].contains(['/', '\\'])
        {
            switches.push(arg.text[1..].to_ascii_lowercase());
        } else {
            paths.push(arg.clone());
        }
    }
    (switches, paths)
}

fn cmd_statement(statement: Statement, ctx: &mut Ctx) {
    let Statement {
        words, redirects, ..
    } = statement;
    cmd_words(words, redirects, ctx);
}

/// Handles the prefixes of a cmd statement (`@`, `call`, `if ...`, `for ... do`, `else`), splits
/// it where a parenthesized group closes, and classifies what is left.
fn cmd_words(mut words: Vec<Word>, redirects: Vec<Redirect>, ctx: &mut Ctx) {
    if let Some(first) = words.first_mut() {
        if let Some(stripped) = first.text.strip_prefix('@') {
            first.text = stripped.to_string();
        }
        if first.text.starts_with("::") {
            return;
        }
    }
    loop {
        let lower: Vec<String> = words.iter().map(|w| w.text.to_ascii_lowercase()).collect();
        match lower.first().map(String::as_str) {
            Some("call" | "else") => {
                words.remove(0);
            }
            Some("if") => {
                let at = |i: usize| lower.get(i).map(String::as_str);
                let mut skip = 1;
                if at(skip) == Some("/i") {
                    skip += 1;
                }
                if at(skip) == Some("not") {
                    skip += 1;
                }
                match at(skip) {
                    Some("exist" | "defined" | "errorlevel" | "cmdextversion") => skip += 2,
                    Some(token) if token.contains("==") => skip += 1,
                    Some(_)
                        if matches!(
                            at(skip + 1),
                            Some("==" | "equ" | "neq" | "lss" | "leq" | "gtr" | "geq")
                        ) =>
                    {
                        skip += 3;
                    }
                    _ => skip += 1,
                }
                words.drain(..skip.min(words.len()));
            }
            Some("for") => match lower.iter().position(|w| w == "do") {
                Some(at) => {
                    words.drain(..=at);
                }
                None => break,
            },
            _ => break,
        }
    }
    // A closing parenthesis ends the command before it: `if exist a (del a) else (echo no)`.
    if let Some(at) = words
        .iter()
        .position(|w| w.text.ends_with(')') && w.kind != Kind::Opaque)
    {
        let rest = words.split_off(at + 1);
        if let Some(last) = words.last_mut() {
            last.text = last.text.trim_end_matches(')').to_string();
            if last.text.is_empty() {
                words.pop();
            }
        }
        cmd_words(words, redirects, ctx);
        cmd_words(rest, Vec::new(), ctx);
        return;
    }
    let Some(first) = words.first() else {
        if !redirects.is_empty() {
            ctx.command(Vec::new(), redirects, true);
        }
        return;
    };
    if !first.is_exact() {
        ctx.push(&words, Outcome::unknown(), redirects);
        return;
    }
    let name = first.text.to_ascii_lowercase();
    let (switches, paths) = cmd_args(&words[1..]);
    let has = |s: &str| {
        switches
            .iter()
            .any(|x| x == s || x.starts_with(&format!("{s}:")))
    };
    let mut out = Outcome::default();
    match name.as_str() {
        "del" | "erase" => {
            out.class(CommandClass::Delete);
            out.targets(paths.iter(), TargetOp::Delete, has("s"));
        }
        "rd" | "rmdir" => {
            out.class(CommandClass::Delete);
            out.targets(paths.iter(), TargetOp::Delete, has("s"));
        }
        "move" => {
            out.class(CommandClass::Move);
            if let [src, dest, ..] = paths.as_slice() {
                out.target(src, TargetOp::Delete, true);
                out.target(dest, TargetOp::Write, true);
            } else {
                out.unknown_targets();
            }
        }
        "copy" | "xcopy" => {
            out.class(CommandClass::Write);
            if let [src, rest @ ..] = paths.as_slice() {
                out.target(src, TargetOp::Read, has("s") || has("e"));
                match rest.first() {
                    Some(dest) => out.target(dest, TargetOp::Write, true),
                    None => out.unknown_targets(),
                };
            } else {
                out.unknown_targets();
            }
        }
        "robocopy" => {
            out.class(CommandClass::Write);
            if let [src, dest, ..] = paths.as_slice() {
                out.target(src, TargetOp::Read, true);
                out.target(dest, TargetOp::Write, true);
                if has("mir") || has("purge") {
                    out.class(CommandClass::Delete);
                    out.target(dest, TargetOp::Delete, true);
                }
                if has("mov") || has("move") {
                    out.class(CommandClass::Delete);
                    out.target(src, TargetOp::Delete, true);
                }
            } else {
                out.unknown_targets();
            }
        }
        "ren" | "rename" => {
            out.class(CommandClass::Move);
            match paths.as_slice() {
                [src, new, ..] if src.is_exact() && new.is_exact() => {
                    out.target(src, TargetOp::Delete, true);
                    let dir = dirname(&src.text);
                    let dest = if dir == "." {
                        new.text.clone()
                    } else {
                        format!("{dir}/{}", basename(&new.text))
                    };
                    out.path(&dest, Kind::Exact, TargetOp::Write, true);
                }
                _ => {
                    out.unknown_targets();
                }
            }
        }
        "mkdir" | "md" => {
            out.class(CommandClass::Write);
            out.targets(paths.iter(), TargetOp::Write, false);
        }
        "cd" | "chdir" => {
            out.class(CommandClass::Read);
            out.chdir = Some(Chdir::To(paths.first().cloned()));
        }
        "pushd" | "popd" => {
            out.class(CommandClass::Read);
            out.chdir = Some(Chdir::To(None));
        }
        "type" | "more" | "findstr" | "find" | "dir" | "fc" => {
            out.class(CommandClass::Read);
            out.targets(
                paths
                    .iter()
                    .skip(usize::from(matches!(name.as_str(), "findstr" | "find"))),
                TargetOp::Read,
                false,
            );
        }
        _ if name.starts_with("echo.") => {
            out.class(CommandClass::Read);
        }
        "echo" | "rem" | "set" | "setlocal" | "endlocal" | "title" | "cls" | "pause" | "exit"
        | "ver" | "where" | "tasklist" | "whoami" | "hostname" | "ipconfig" | "color"
        | "prompt" | "path" | "timeout" | "chcp" | "mode" | "vol" | "date" | "time" => {
            out.class(CommandClass::Read);
        }
        "start" => {
            out.and_unknown();
        }
        _ => {
            ctx.command(words, redirects, true);
            return;
        }
    }
    ctx.push(&words, out, redirects);
}
