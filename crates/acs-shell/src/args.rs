//! Option parsing and the per-command result builder used by every handler.

use acs_core::gate::{CommandClass, Confidence, GitInvocation, ShellTarget, TargetOp};

use crate::Dialect;
use crate::word::{Kind, Word, confidence, is_absolute, is_null_device, join};

/// One option as written (`-r`, `--recursive`, `-n`), with its value when it takes one.
#[derive(Debug, Clone)]
pub(crate) struct Opt {
    pub name: String,
    pub value: Option<Word>,
}

/// Arguments split into options and positionals.
#[derive(Debug, Default)]
pub(crate) struct Parsed<'a> {
    pub opts: Vec<Opt>,
    /// Every positional, including those after `--`.
    pub pos: Vec<&'a Word>,
    /// How many positionals came before a `--` (all of them when there is none).
    pub before_dd: usize,
}

impl<'a> Parsed<'a> {
    /// Splits `args`. `value_opts` lists the options that consume a value (`-n`, `--output`);
    /// short options may be clustered (`-rf`) and take the rest of the cluster or the next word.
    pub fn new(args: &'a [Word], value_opts: &[&str]) -> Self {
        let mut parsed = Self::default();
        let mut dd = false;
        let mut i = 0;
        while i < args.len() {
            let arg = &args[i];
            i += 1;
            if dd || !arg.is_option() {
                parsed.pos.push(arg);
                if !dd {
                    parsed.before_dd += 1;
                }
                continue;
            }
            if arg.text == "--" {
                dd = true;
                continue;
            }
            if let Some(long) = arg.text.strip_prefix("--") {
                if let Some((name, value)) = long.split_once('=') {
                    parsed.opts.push(Opt {
                        name: format!("--{name}"),
                        value: Some(Word {
                            text: value.to_string(),
                            kind: arg.kind,
                        }),
                    });
                } else if value_opts.contains(&arg.text.as_str()) && i < args.len() {
                    parsed.opts.push(Opt {
                        name: arg.text.clone(),
                        value: Some(args[i].clone()),
                    });
                    i += 1;
                } else {
                    parsed.opts.push(Opt {
                        name: arg.text.clone(),
                        value: None,
                    });
                }
                continue;
            }
            // A cluster of short options.
            for (index, ch) in arg.text.char_indices().skip(1) {
                let name = format!("-{ch}");
                if value_opts.contains(&name.as_str()) {
                    let rest = &arg.text[index + ch.len_utf8()..];
                    let value = if !rest.is_empty() {
                        Some(Word {
                            text: rest.to_string(),
                            kind: arg.kind,
                        })
                    } else if i < args.len() {
                        i += 1;
                        Some(args[i - 1].clone())
                    } else {
                        None
                    };
                    parsed.opts.push(Opt { name, value });
                    break;
                }
                parsed.opts.push(Opt { name, value: None });
            }
        }
        parsed
    }

    /// Whether any of the option names was given.
    pub fn has(&self, names: &[&str]) -> bool {
        self.opts.iter().any(|o| names.contains(&o.name.as_str()))
    }

    /// The value of the last occurrence of any of the options.
    pub fn value(&self, names: &[&str]) -> Option<Word> {
        self.opts
            .iter()
            .rev()
            .find(|o| names.contains(&o.name.as_str()))
            .and_then(|o| o.value.clone())
    }

    /// The values of every occurrence of any of the options.
    pub fn values(&self, names: &[&str]) -> Vec<Word> {
        self.opts
            .iter()
            .filter(|o| names.contains(&o.name.as_str()))
            .filter_map(|o| o.value.clone())
            .collect()
    }

    /// The positionals before `--`.
    pub fn positionals(&self) -> &[&'a Word] {
        &self.pos[..self.before_dd]
    }

    /// The positionals after `--`.
    pub fn after_dd(&self) -> &[&'a Word] {
        &self.pos[self.before_dd..]
    }
}

/// A change of working directory made by a command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Chdir {
    /// `cd <dir>` (no directory means home, which is unknown).
    To(Option<Word>),
}

/// What a handler concludes about one simple command.
#[derive(Debug, Default)]
pub(crate) struct Outcome {
    pub classes: Vec<CommandClass>,
    pub targets: Vec<ShellTarget>,
    pub unknown_targets: bool,
    pub git: Option<GitInvocation>,
    /// Scripts the command runs that were found in its arguments (`sh -c '...'`).
    pub nested: Vec<(Dialect, String)>,
    pub chdir: Option<Chdir>,
    /// The program name to show in place of the word as written (`/bin/rm` becomes `rm`).
    pub program: Option<String>,
    /// Set when the program is not one the table knows; its name is left as written.
    pub generic: bool,
}

impl Outcome {
    /// A command that only reads (or does nothing).
    pub fn read() -> Self {
        Self::with(&[CommandClass::Read])
    }

    /// A command with the given classes and no targets yet.
    pub fn with(classes: &[CommandClass]) -> Self {
        Self {
            classes: classes.to_vec(),
            ..Self::default()
        }
    }

    /// A command whose effects cannot be determined.
    pub fn unknown() -> Self {
        Self {
            classes: vec![CommandClass::Unknown],
            unknown_targets: true,
            ..Self::default()
        }
    }

    /// Adds a class once.
    pub fn class(&mut self, class: CommandClass) -> &mut Self {
        if !self.classes.contains(&class) {
            self.classes.push(class);
        }
        self
    }

    /// Marks the command as possibly touching paths that are not listed.
    pub fn unknown_targets(&mut self) -> &mut Self {
        self.unknown_targets = true;
        self
    }

    /// Adds the unknown class and marks the targets incomplete.
    pub fn and_unknown(&mut self) -> &mut Self {
        self.unknown_targets = true;
        self.class(CommandClass::Unknown)
    }

    /// Adds a target for a word; words that are not paths and null devices are skipped.
    pub fn target(&mut self, word: &Word, op: TargetOp, recursive: bool) -> &mut Self {
        if let Some(confidence) = confidence(word.kind)
            && !word.text.is_empty()
            && !is_null_device(&word.text)
        {
            self.targets.push(ShellTarget {
                path: word.text.clone(),
                op,
                recursive,
                confidence,
            });
        }
        self
    }

    /// Adds a target for a pathspec the program expands itself (`git rm "*.log"`, `prettier
    /// "src/**"`): a quoted glob is still a pattern.
    pub fn pathspec(&mut self, word: &Word, op: TargetOp, recursive: bool) -> &mut Self {
        let kind = if word.kind == Kind::Exact && word.text.contains(['*', '?', '[']) {
            Kind::Pattern
        } else {
            word.kind
        };
        self.path(&word.text, kind, op, recursive)
    }

    /// Adds targets for several pathspecs.
    pub fn pathspecs<'w>(
        &mut self,
        words: impl IntoIterator<Item = &'w Word>,
        op: TargetOp,
        recursive: bool,
    ) -> &mut Self {
        for word in words {
            self.pathspec(word, op, recursive);
        }
        self
    }

    /// Adds a target for a path computed by the handler.
    pub fn path(&mut self, path: &str, kind: Kind, op: TargetOp, recursive: bool) -> &mut Self {
        self.target(
            &Word {
                text: path.to_string(),
                kind,
            },
            op,
            recursive,
        )
    }

    /// Adds targets for several words.
    pub fn targets<'w>(
        &mut self,
        words: impl IntoIterator<Item = &'w Word>,
        op: TargetOp,
        recursive: bool,
    ) -> &mut Self {
        for word in words {
            self.target(word, op, recursive);
        }
        self
    }

    /// Prefixes the targets added since `start` with a directory (`git -C dir`, `tar -C dir`).
    /// A directory that is not literal makes them partial.
    pub fn rebase_from(&mut self, start: usize, dir: &Word) {
        for target in &mut self.targets[start..] {
            if is_absolute(&target.path) || target.confidence == Confidence::Partial {
                continue;
            }
            if dir.is_exact() {
                target.path = join(&dir.text, &target.path);
            } else if dir.kind != Kind::Opaque {
                target.confidence = Confidence::Partial;
            }
        }
    }
}
