//! The state shared while one command line is analyzed, and the step that turns a handler's
//! [`Outcome`] into a [`SimpleCommand`].

use acs_core::gate::{CommandClass, SimpleCommand, TargetOp};

use crate::Dialect;
use crate::args::{Chdir, Outcome};
use crate::table::classify;
use crate::word::{Cwd, Redirect, Word};

/// How deeply scripts inside scripts (`sh -c 'sh -c ...'`) are followed.
const MAX_NESTING: u8 = 4;

/// Accumulates the commands of one analysis.
pub(crate) struct Ctx {
    /// The directory relative paths are resolved against right now.
    pub cwd: Cwd,
    /// The commands found so far, in source order.
    pub out: Vec<SimpleCommand>,
    /// Cleared when any part of the line did not parse.
    pub complete: bool,
    depth: u8,
}

impl Ctx {
    pub fn new() -> Self {
        Self {
            cwd: Cwd::start(),
            out: Vec::new(),
            complete: true,
            depth: 0,
        }
    }

    /// Classifies and records a command given as words and redirections. `windows` selects
    /// case-insensitive program names.
    pub fn command(&mut self, mut words: Vec<Word>, redirects: Vec<Redirect>, windows: bool) {
        if words.is_empty() {
            // A bare redirection (`> file`) is the null command.
            words.push(Word::exact(":"));
        }
        let (start, outcome) = classify(&words, windows);
        self.push(&words[start..], outcome, redirects);
    }

    /// Records a command whose effects the caller determined.
    pub fn push(&mut self, words: &[Word], mut outcome: Outcome, redirects: Vec<Redirect>) {
        for redirect in redirects {
            let before = outcome.targets.len();
            outcome.target(&redirect.word, redirect.op, false);
            if redirect.op == TargetOp::Write && outcome.targets.len() > before {
                outcome.class(CommandClass::Write);
            }
        }
        for target in &mut outcome.targets {
            self.cwd.apply(target);
        }
        if outcome.classes.len() > 1 {
            outcome.classes.retain(|c| *c != CommandClass::Read);
        }
        if outcome.classes.is_empty() {
            outcome.classes.push(CommandClass::Unknown);
        }
        if outcome.classes.contains(&CommandClass::Unknown) {
            outcome.unknown_targets = true;
        }
        let nested = std::mem::take(&mut outcome.nested);
        if let Some(Chdir::To(dir)) = &outcome.chdir {
            let next = self.cwd.change(dir.as_ref());
            self.record(words, outcome);
            self.cwd = next;
        } else {
            self.record(words, outcome);
        }
        for (dialect, script) in nested {
            self.nested(dialect, &script);
        }
    }

    fn record(&mut self, words: &[Word], outcome: Outcome) {
        let mut texts: Vec<String> = words.iter().map(|w| w.text.clone()).collect();
        if let (Some(program), Some(first)) = (outcome.program, texts.first_mut()) {
            *first = program;
        }
        self.out.push(SimpleCommand {
            words: texts,
            classes: outcome.classes,
            targets: outcome.targets,
            targets_known: !outcome.unknown_targets,
            git: outcome.git,
        });
    }

    /// Records a line (or part of one) that could not be understood at all.
    pub fn unparsed(&mut self, text: &str) {
        self.complete = false;
        let words: Vec<Word> = text.split_whitespace().map(Word::exact).collect();
        let words = if words.is_empty() {
            vec![Word::exact(text)]
        } else {
            words
        };
        self.push(&words, Outcome::unknown(), Vec::new());
    }

    /// Analyzes a script found inside a command (`bash -c '...'`, `eval ...`) in the current
    /// directory; nothing it does to the directory leaks out.
    pub fn nested(&mut self, dialect: Dialect, script: &str) {
        if self.depth >= MAX_NESTING {
            return;
        }
        let saved = self.cwd.clone();
        self.depth += 1;
        match dialect {
            Dialect::Bash => crate::bash::run(script, self),
            Dialect::PowerShell => crate::windows::powershell(script, self),
            Dialect::Cmd => crate::windows::cmd(script, self),
        }
        self.depth -= 1;
        self.cwd = saved;
    }
}
