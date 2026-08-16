//! Argument parsing, hand-rolled.
//!
//! The surface is five flags, two options, and a path list. clap brings
//! seventeen crates for that: a derive macro (proc-macro2, quote, syn), ANSI
//! styling (anstream and four anstyle crates), and edit-distance matching for
//! "did you mean". This project already refused that trade twice, hand-writing
//! the ".clang-format" reader rather than taking a YAML crate and the assembly
//! scanner rather than taking a grammar, so it refuses it here too.
//!
//! Parsing is the boring kind: one pass, "--flag" and "--opt VALUE" and
//! "--opt=VALUE", "--" ends option parsing, and a lone "-" is a path meaning
//! stdin. Anything unrecognized is an error rather than a guess.

use std::ffi::OsString;
use std::fmt;

use crate::parse::Language;

const USAGE: &str = "\
Usage: commentflow [OPTIONS] <PATHS>...
       commentflow [OPTIONS] --files-from <FILE|->

C/C++/Rust/POSIX-shell/assembly comment reflow.

Arguments:
  <PATHS>...  Source file(s) or directory(ies) to reflow, or '-' for stdin.
              Extensions, matched case-insensitively, so '.S' is '.s':
              .c .h .m .cc .cpp .cxx .c++ .hh .hpp .hxx .h++ .mm .rs .sh
              .bash .s

Options:
      --files-from <FILE|->  Read the path list from a file (or '-' for stdin);
                             newline- or NUL-delimited
      --lang <LANG>          Source language for the '-' (stdin) source;
                             required only with '-'
                             [possible values: c, cpp, rust, shell, asm]
      --check                Report whether any changes would be made; exit 1
                             if so. No writes
      --diff                 Print a unified diff to stdout. No writes
      --dry-run              Summarize what would change without writing
      --column-limit <N>     Override .clang-format ColumnLimit discovery
                             (0 disables reflow)
      --to-blocks            Also rewrite standalone '//' comment runs as
                             '/* */' blocks before reflowing. Doc comments
                             ('///', '//!'), trailing '//', and a file-start
                             SPDX header are left alone
  -h, --help                 Print help

Exit codes:
  0  success (write/diff/dry-run completed; --check found no changes)
  1  --check found one or more changes that would be made (write mode does not use this)
  2  I/O or parse error

Use '-' as the path to read source from stdin and write the result to
stdout (requires --lang). '--files-from <file|->' reads a newline- or
NUL-delimited path list instead of listing paths on the command line.
";

const USAGE_SHORT: &str = "\
Usage: commentflow [OPTIONS] <PATHS>...
       commentflow [OPTIONS] --files-from <FILE|->

For more information, try '--help'.
";

/// Explicit source language for the stdin ("-") source, which has no file
/// extension to detect from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    C,
    Cpp,
    Rust,
    Shell,
    Asm,
}

impl Lang {
    pub fn to_language(self) -> Language {
        match self {
            Lang::C => Language::C,
            Lang::Cpp => Language::Cpp,
            Lang::Rust => Language::Rust,
            Lang::Shell => Language::Shell,
            Lang::Asm => Language::Asm,
        }
    }

    fn from_value(v: &str) -> Option<Self> {
        match v {
            "c" => Some(Lang::C),
            "cpp" => Some(Lang::Cpp),
            "rust" => Some(Lang::Rust),
            "shell" => Some(Lang::Shell),
            "asm" => Some(Lang::Asm),
            _ => None,
        }
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Args {
    /// Source file(s) or directory(ies) to reflow, or "-" for stdin.
    pub paths: Vec<String>,
    /// Read the path list from a file (or "-" for stdin); newline- or
    /// NUL-delimited.
    pub files_from: Option<String>,
    /// Source language for the "-" (stdin) source; required only with "-".
    pub lang: Option<Lang>,
    /// Report whether any changes would be made; exit 1 if so. No writes.
    pub check: bool,
    /// Print a unified diff to stdout. No writes.
    pub diff: bool,
    /// Summarize what would change without writing.
    pub dry_run: bool,
    /// Override .clang-format ColumnLimit discovery (0 disables reflow).
    pub column_limit: Option<usize>,
    /// Also rewrite standalone "//" comment runs as "/* */" blocks before
    /// reflowing.
    pub to_blocks: bool,
}

/// Why parsing stopped. "Help" is not a failure: it is the one outcome that
/// prints to stdout and exits 0.
#[derive(Debug, PartialEq, Eq)]
pub enum ParseError {
    Help,
    Message(String),
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ParseError::Help => f.write_str(USAGE),
            ParseError::Message(m) => f.write_str(m),
        }
    }
}

fn err<T>(message: impl Into<String>) -> Result<T, ParseError> {
    Err(ParseError::Message(message.into()))
}

/// A flag takes no value, so "--check=1" is a mistake worth reporting rather
/// than a value to drop on the floor.
fn reject_value(inline: Option<&str>, name: &str) -> Result<(), ParseError> {
    match inline {
        Some(v) => err(format!(
            "unexpected value '{v}' for '{name}' found; no more were expected"
        )),
        None => Ok(()),
    }
}

/// Repeating an option is an error, not last-one-wins: "--column-limit 80
/// --column-limit 100" is far more likely a mistake than an intent.
fn set_once<T>(slot: &mut Option<T>, value: T, name: &str) -> Result<(), ParseError> {
    if slot.is_some() {
        return err(format!(
            "the argument '{name}' cannot be used multiple times"
        ));
    }
    *slot = Some(value);
    Ok(())
}

fn set_flag(slot: &mut bool, name: &str) -> Result<(), ParseError> {
    if *slot {
        return err(format!(
            "the argument '{name}' cannot be used multiple times"
        ));
    }
    *slot = true;
    Ok(())
}

impl Args {
    /// Parse the process arguments, or print and exit the way a CLI should:
    /// help to stdout with status 0, a usage error to stderr with status 2.
    ///
    /// Reads "args_os": "env::args" panics on an argument that is not valid
    /// UTF-8, and a path on Unix need not be. The tool's model is UTF-8 text
    /// end to end (it reads sources with "read_to_string"), so such a path is
    /// rejected rather than supported, but rejecting is a diagnostic and exit
    /// 2, not a panic.
    pub fn parse() -> Args {
        let argv: Vec<String> = match std::env::args_os()
            .skip(1)
            .map(OsString::into_string)
            .collect()
        {
            Ok(argv) => argv,
            Err(bad) => {
                eprintln!(
                    "commentflow: argument is not valid UTF-8: {}",
                    bad.to_string_lossy()
                );
                std::process::exit(2);
            }
        };
        match Args::try_parse_from(argv) {
            Ok(args) => args,
            Err(ParseError::Help) => {
                print!("{USAGE}");
                std::process::exit(0);
            }
            Err(ParseError::Message(m)) => {
                // The short form on purpose: an error is read in a CI log, and
                // dumping 1.8 KB of help over the one line that matters buries
                // it. The full text is what "--help" is for.
                eprintln!("commentflow: {m}");
                eprint!("{USAGE_SHORT}");
                std::process::exit(2);
            }
        }
    }

    /// The parser proper, over an argument iterator that excludes argv[0].
    /// Separate from "parse" so it can be tested without spawning a process.
    ///
    /// The awkward cases are the ones clap got right and a naive pass gets
    /// wrong, so each is handled explicitly: a value glued with "=" is only
    /// legal on an option that takes one, a value that looks like the next
    /// option is a missing value rather than a value, and repeating any option
    /// is an error instead of last-one-wins. A lone "-" is the exception to the
    /// second rule: it is a real value meaning stdin.
    pub fn try_parse_from<I: IntoIterator<Item = String>>(argv: I) -> Result<Args, ParseError> {
        let mut args = Args::default();
        let mut rest = argv.into_iter();
        let mut positional_only = false;

        while let Some(arg) = rest.next() {
            if positional_only {
                args.paths.push(arg);
                continue;
            }

            // Split "--opt=value" so the arms below only decide what to do with
            // a value, not how to find one. "--" is not an option name, so it
            // never splits.
            let (name, inline) = match arg.split_once('=') {
                Some((n, v)) if n.starts_with("--") && n.len() > 2 => {
                    (n.to_string(), Some(v.to_string()))
                }
                _ => (arg.clone(), None),
            };

            // A value came glued to the option, or it is the next argument. A
            // next argument that starts with "-" is the next option: clap
            // rejects it as a missing value rather than swallowing the flag,
            // and so do we. The lone "-" stays a value, since "--files-from -"
            // means "read the list from stdin".
            let mut take_value = |opt: &str| match inline.clone() {
                Some(v) => Ok(v),
                None => match rest.next() {
                    Some(v) if v == "-" || !v.starts_with('-') => Ok(v),
                    _ => err(format!(
                        "a value is required for '{opt}' but none was supplied"
                    )),
                },
            };

            match name.as_str() {
                "--" => {
                    reject_value(inline.as_deref(), "--")?;
                    positional_only = true;
                }
                "-h" | "--help" => {
                    reject_value(inline.as_deref(), &name)?;
                    return Err(ParseError::Help);
                }
                "--check" => {
                    reject_value(inline.as_deref(), &name)?;
                    set_flag(&mut args.check, "--check")?;
                }
                "--diff" => {
                    reject_value(inline.as_deref(), &name)?;
                    set_flag(&mut args.diff, "--diff")?;
                }
                "--dry-run" => {
                    reject_value(inline.as_deref(), &name)?;
                    set_flag(&mut args.dry_run, "--dry-run")?;
                }
                "--to-blocks" => {
                    reject_value(inline.as_deref(), &name)?;
                    set_flag(&mut args.to_blocks, "--to-blocks")?;
                }
                "--files-from" => {
                    let v = take_value("--files-from")?;
                    set_once(&mut args.files_from, v, "--files-from")?;
                }
                "--lang" => {
                    let v = take_value("--lang")?;
                    let Some(lang) = Lang::from_value(&v) else {
                        return err(format!(
                            "invalid value '{v}' for --lang (expected one of: c, cpp, rust, shell, asm)"
                        ));
                    };
                    set_once(&mut args.lang, lang, "--lang")?;
                }
                "--column-limit" => {
                    let v = take_value("--column-limit")?;
                    let Ok(n) = v.parse::<usize>() else {
                        return err(format!(
                            "invalid value '{v}' for --column-limit (expected a number)"
                        ));
                    };
                    set_once(&mut args.column_limit, n, "--column-limit")?;
                }

                // A lone "-" is the stdin path, not an option.
                "-" => args.paths.push(arg),
                _ if arg.starts_with('-') => return err(format!("unrecognized option '{name}'")),
                _ => args.paths.push(arg),
            }
        }

        args.validate()?;
        Ok(args)
    }

    /// The two rules the grammar above cannot express: an input source is
    /// required and comes from exactly one place, and the three no-write modes
    /// are mutually exclusive.
    fn validate(&self) -> Result<(), ParseError> {
        match (self.paths.is_empty(), self.files_from.is_none()) {
            (true, true) => {
                return err("no input: give at least one path, or --files-from <file|->");
            }
            (false, false) => return err("--files-from cannot be combined with path arguments"),
            _ => {}
        }
        let modes = [
            ("--check", self.check),
            ("--diff", self.diff),
            ("--dry-run", self.dry_run),
        ];
        let chosen: Vec<&str> = modes
            .iter()
            .filter(|(_, on)| *on)
            .map(|(n, _)| *n)
            .collect();
        if let [first, rest @ ..] = chosen.as_slice()
            && !rest.is_empty()
        {
            let others = rest.join(", ");
            return err(format!("{first} cannot be used with {others}"));
        }
        Ok(())
    }

    pub fn mode(&self) -> Mode {
        if self.check {
            Mode::Check
        } else if self.diff {
            Mode::Diff
        } else if self.dry_run {
            Mode::DryRun
        } else {
            Mode::Write
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Write,
    Check,
    Diff,
    DryRun,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(argv: &[&str]) -> Result<Args, ParseError> {
        Args::try_parse_from(argv.iter().map(|s| (*s).to_string()))
    }

    fn message(argv: &[&str]) -> String {
        match parse(argv) {
            Err(ParseError::Message(m)) => m,
            other => panic!("expected a usage error, got {other:?}"),
        }
    }

    #[test]
    fn paths_and_flags() {
        let a = parse(&["a.c", "b.c", "--check"]).unwrap();
        assert_eq!(a.paths, ["a.c", "b.c"]);
        assert!(a.check);
        assert_eq!(a.mode(), Mode::Check);
        assert_eq!(parse(&["a.c"]).unwrap().mode(), Mode::Write);
    }

    #[test]
    fn option_values_both_spellings() {
        assert_eq!(
            parse(&["a.c", "--column-limit", "72"])
                .unwrap()
                .column_limit,
            Some(72)
        );
        assert_eq!(
            parse(&["a.c", "--column-limit=72"]).unwrap().column_limit,
            Some(72)
        );
        assert_eq!(parse(&["-", "--lang=cpp"]).unwrap().lang, Some(Lang::Cpp));
        assert_eq!(
            parse(&["--files-from=list"]).unwrap().files_from.as_deref(),
            Some("list")
        );
        // Zero is a real value: it disables reflow.
        assert_eq!(
            parse(&["a.c", "--column-limit", "0"]).unwrap().column_limit,
            Some(0)
        );
    }

    #[test]
    fn a_lone_dash_is_a_path_not_an_option() {
        let a = parse(&["-", "--lang", "c"]).unwrap();
        assert_eq!(a.paths, ["-"]);
        assert_eq!(a.lang, Some(Lang::C));
    }

    #[test]
    fn double_dash_ends_option_parsing() {
        let a = parse(&["--check", "--", "--weird-name.c"]).unwrap();
        assert_eq!(a.paths, ["--weird-name.c"]);
        assert!(a.check);
    }

    #[test]
    fn input_source_is_required_and_singular() {
        assert!(message(&["--column-limit", "40"]).contains("no input"));
        assert!(message(&["a.c", "--files-from", "list"]).contains("--files-from"));
    }

    #[test]
    fn no_write_modes_are_exclusive() {
        assert!(message(&["a.c", "--check", "--diff"]).contains("cannot be used with"));
        assert!(message(&["a.c", "--diff", "--dry-run"]).contains("cannot be used with"));
        assert_eq!(
            message(&["a.c", "--check", "--diff", "--dry-run"]),
            "--check cannot be used with --diff, --dry-run"
        );
    }

    #[test]
    fn bad_values_and_unknown_options_are_errors() {
        assert!(message(&["-", "--lang", "python"]).contains("--lang"));
        assert!(message(&["a.c", "--column-limit", "wide"]).contains("--column-limit"));
        assert!(message(&["a.c", "--column-limit"]).contains("a value is required"));
        assert!(message(&["a.c", "--reflow-everything"]).contains("unrecognized"));
    }

    #[test]
    fn a_flag_takes_no_value() {
        // clap rejects these rather than dropping the value silently.
        assert!(message(&["a.c", "--check=1"]).contains("unexpected value '1'"));
        assert!(message(&["a.c", "--to-blocks=x"]).contains("--to-blocks"));
        assert!(message(&["a.c", "--=x"]).contains("unrecognized"));
    }

    #[test]
    fn a_value_that_looks_like_an_option_is_a_missing_value() {
        // Swallowing the next flag as a value loses the flag AND misreads the
        // option, which is the worst of both.
        assert!(message(&["--files-from", "--check"]).contains("a value is required"));
        assert!(message(&["a.c", "--column-limit", "--check"]).contains("a value is required"));
        assert!(message(&["-", "--lang", "--diff"]).contains("a value is required"));

        // A lone "-" is a real value: "--files-from -" reads the list from
        // stdin.
        assert_eq!(
            parse(&["--files-from", "-"]).unwrap().files_from.as_deref(),
            Some("-")
        );
    }

    #[test]
    fn options_cannot_repeat() {
        assert!(message(&["a.c", "--check", "--check"]).contains("multiple times"));
        assert!(
            message(&["a.c", "--column-limit", "80", "--column-limit", "100"])
                .contains("multiple times")
        );
        assert!(message(&["--files-from", "a", "--files-from", "b"]).contains("multiple times"));
        assert!(message(&["-", "--lang", "c", "--lang", "cpp"]).contains("multiple times"));
    }

    #[test]
    fn help_is_its_own_outcome() {
        assert_eq!(parse(&["--help"]), Err(ParseError::Help));
        assert_eq!(parse(&["-h"]), Err(ParseError::Help));
        // Help is a flag like any other: a glued value is still a mistake.
        assert!(message(&["--help=x"]).contains("unexpected value 'x'"));
        // Help wins even when the rest of the line would not validate.
        assert_eq!(
            parse(&["--check", "--diff", "--help"]),
            Err(ParseError::Help)
        );
    }
}
