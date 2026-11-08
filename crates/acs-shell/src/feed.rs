//! Scripts handed to a shell in ways that hide them from a plain reading of the command line:
//! piped into a shell's standard input (`echo 'rm -rf x' | sh`), given as a here-string or
//! here-document, or base64-encoded (`echo cm0= | base64 -d | sh`,
//! `powershell -EncodedCommand ...`). Each is analyzed as a nested script, so the commands inside
//! are seen like any other.

use acs_core::gate::SimpleCommand;

use crate::Dialect;
use crate::word::basename;

/// The dialect of a shell that reads its script from standard input: a POSIX shell with no
/// `-c` and no script operand (`sh`, `bash -s`, `bash -`), or PowerShell reading `-` (or given
/// nothing at all).
pub(crate) fn stdin_shell(words: &[String]) -> Option<Dialect> {
    let (program, args) = words.split_first()?;
    let name = program_key(program);
    match name.as_str() {
        "sh" | "bash" | "zsh" | "dash" | "ksh" | "ash" => {
            for arg in args {
                if arg == "-" || arg == "--" {
                    return Some(Dialect::Bash);
                }
                if !arg.starts_with('-') || (!arg.starts_with("--") && arg.contains('c')) {
                    return None;
                }
            }
            Some(Dialect::Bash)
        }
        "powershell" | "pwsh" => {
            let reads_stdin = args.is_empty()
                || args.iter().any(|a| a == "-")
                || args.iter().all(|a| {
                    let lower = a.to_ascii_lowercase();
                    lower.starts_with('-') && !lower.starts_with("-c") && !lower.starts_with("-f")
                });
            reads_stdin.then_some(Dialect::PowerShell)
        }
        _ => None,
    }
}

/// For the commands of one pipeline, in order: the script fed to a shell at its end, when the
/// pipeline starts with `echo` or `printf` of literal text (decoded when a `base64 -d` stage is in
/// between).
pub(crate) fn piped_script(commands: &[SimpleCommand]) -> Option<(Dialect, String)> {
    let [first, middle @ .., last] = commands else {
        return None;
    };
    let dialect = stdin_shell(&last.words)?;
    let (program, args) = first.words.split_first()?;
    let text = match program_key(program).as_str() {
        "echo" => args
            .iter()
            .skip_while(|a| matches!(a.as_str(), "-n" | "-e" | "-E" | "-ne" | "-en"))
            .cloned()
            .collect::<Vec<_>>()
            .join(" "),
        "printf" => args.first()?.replace("\\n", "\n"),
        _ => return None,
    };
    let decodes = middle.iter().any(|c| {
        c.words.first().is_some_and(|p| program_key(p) == "base64")
            && c.words
                .iter()
                .any(|w| matches!(w.as_str(), "-d" | "--decode" | "-D"))
    });
    let text = if decodes {
        String::from_utf8(base64_decode(&text)?).ok()?
    } else {
        text
    };
    Some((dialect, text))
}

/// The text of a PowerShell `-EncodedCommand` argument: base64 of UTF-16LE.
pub(crate) fn powershell_encoded(arg: &str) -> Option<String> {
    let bytes = base64_decode(arg)?;
    if bytes.len() % 2 != 0 {
        return None;
    }
    let units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .collect();
    String::from_utf16(&units).ok()
}

/// Decodes standard or URL-safe base64, ignoring whitespace; padding is optional.
pub(crate) fn base64_decode(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(text.len() * 3 / 4);
    let mut buffer: u32 = 0;
    let mut bits = 0;
    for byte in text.bytes() {
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            b'=' => break,
            b' ' | b'\n' | b'\r' | b'\t' => continue,
            _ => return None,
        };
        buffer = (buffer << 6) | u32::from(value);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(u8::try_from((buffer >> bits) & 0xff).ok()?);
        }
    }
    Some(out)
}

/// A program word as a lookup key: its file name, lowercased, without `.exe`.
fn program_key(word: &str) -> String {
    let base = basename(word).to_ascii_lowercase();
    base.strip_suffix(".exe")
        .map_or(base.clone(), str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(line: &str) -> Vec<String> {
        line.split_whitespace().map(str::to_owned).collect()
    }

    #[test]
    fn base64_round_trips_known_values() {
        assert_eq!(base64_decode("YWNzIG9mZg==").unwrap(), b"acs off");
        assert_eq!(base64_decode("YWNzIG9mZg").unwrap(), b"acs off");
        assert!(base64_decode("not base64!").is_none());
        // `acs off` as UTF-16LE.
        assert_eq!(
            powershell_encoded("YQBjAHMAIABvAGYAZgA=").as_deref(),
            Some("acs off")
        );
    }

    #[test]
    fn shells_reading_stdin_are_recognized() {
        assert_eq!(stdin_shell(&words("sh")), Some(Dialect::Bash));
        assert_eq!(stdin_shell(&words("bash -s")), Some(Dialect::Bash));
        assert_eq!(stdin_shell(&words("/bin/bash -")), Some(Dialect::Bash));
        assert_eq!(stdin_shell(&words("bash script.sh")), None);
        assert_eq!(stdin_shell(&words("bash -c x")), None);
        assert_eq!(stdin_shell(&words("pwsh -")), Some(Dialect::PowerShell));
        assert_eq!(stdin_shell(&words("pwsh -File x.ps1")), None);
        assert_eq!(stdin_shell(&words("grep x")), None);
    }
}
