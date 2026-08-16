//! Style conversion: rewrite a standalone "//" comment run as one "/* */"
//! block, so the reflow pass that follows can lay it out as a block.
//!
//! This is the one thing the tool does that its own contract otherwise
//! forbids: comment style is preserved, always. So it is OFF by default and
//! reachable only through "--to-blocks", and it is a separate pass rather than
//! a rule inside reflow. Everything downstream still sees a source file and
//! obeys the usual invariants; it just sees one where some "//" runs are now
//! blocks.
//!
//! Rules:
//!   - A run of two or more consecutive standalone "//" lines becomes one
//!     "/* */" block. A lone standalone "//" line converts too.
//!   - Trailing "//" (code on the same line) is never part of a standalone
//!     run, so it stays "//".
//!   - Doc comments ("///", "//!", and their "<" member-doc variants) are left
//!     alone: converting them would strip their meaning to rustdoc/Doxygen.
//!   - The file-start SPDX run is refused; see "holds_file_start_spdx".
//!   - Existing "/* */" comments are untouched here; the reflow pass treats
//!     them exactly as it always does. That has a visible consequence: a
//!     converted run directly under an existing block comment at the same
//!     column is now two adjacent blocks, and merging those is what reflow has
//!     always done, so the two comments come out as one. Converting a run next
//!     to a block means accepting that merge.
//!
//! It reuses the tree-sitter parse, so a "//" inside a string literal or a URL
//! is never claimed.

use anyhow::Result;

use crate::parse::{self, Comment, Language, ParserPool};
use crate::rewrite;

/// The replacements that turn every convertible "//" comment in "source" into a
/// "/* */" block. Empty when the language has no "//" comment form or nothing
/// qualifies. Shares the caller's "ParserPool", so "--to-blocks" costs no extra
/// parser construction on top of the reflow pass that follows.
///
/// Returns replacements rather than a rewritten string so the caller can report
/// how much this pass would change, and so the whole run has exactly one place
/// that applies bytes.
pub fn line_run_replacements(
    source: &str,
    lang: Language,
    pool: &mut ParserPool,
) -> Result<Vec<rewrite::Replacement>> {
    // Shell has "#" and assembly claims only "/* */", so neither can have a
    // "//" run to convert. Skip the parse entirely rather than walk a comment
    // list that cannot match.
    if matches!(lang, Language::Shell | Language::Asm) {
        return Ok(Vec::new());
    }

    let comments = parse::extract_comments_with(source, lang, pool)?;
    let reps: Vec<rewrite::Replacement> = comments
        .iter()
        .filter(|c| is_plain_line(c))
        .filter_map(build_block)
        .collect();

    // Backstop: the spans come from comment nodes, but validate anyway so a
    // future change can never silently rewrite a non-comment byte range.
    rewrite::validate(&reps, source, &comments)?;
    Ok(reps)
}

/// True for a plain "//" comment eligible for conversion: not trailing, not a
/// doc-generator marker ("///" / "//!", which also covers the "<" member-doc
/// variants), not flagged for passthrough, and not the file's SPDX header.
fn is_plain_line(c: &Comment) -> bool {
    !c.is_trailing
        && !c.force_passthrough
        && !holds_file_start_spdx(c)
        && c.text.starts_with("//")
        && !c.text.starts_with("///")
        && !c.text.starts_with("//!")
}

/// True for a file-start run whose first line carries the SPDX tag.
///
/// The tag has to stay on line 1, where the kernel checker, "reuse", and
/// scancode look for it. "can_merge" refuses to merge a file-start BLOCK for
/// exactly this reason and exempts "//" runs, reasoning that a run of one-line
/// comments keeps the tag on its own line either way. Converting the run is
/// what makes that false: the block opens with a bare "/*" and the tag lands on
/// line 2, which reads as an unlicensed file. Kernel policy also wants the tag
/// in "//" form in a ".c"/".h" file, so the style flip is wrong on its own.
fn holds_file_start_spdx(c: &Comment) -> bool {
    c.at_file_start
        && c.text
            .lines()
            .next()
            .is_some_and(|l| l.contains("SPDX-License-Identifier"))
}

/// Build the "/* */" replacement for one plain-"//" comment node. The parser
/// already coalesces a consecutive standalone "//" run into a single node whose
/// "text" carries the embedded newlines, so this handles a run and a lone line
/// through the same path. Layout (collapse, wrap, bookend strip) is left to the
/// reflow pass.
fn build_block(c: &Comment) -> Option<rewrite::Replacement> {
    let indent = &c.line_indent_bytes;

    let lines: Vec<&str> = c
        .text
        .split('\n')
        .map(|raw| {
            // Each interior line still carries its own source indent up to the
            // "//". Strip indent, the "//" marker, then one optional space.
            let raw = raw.strip_suffix('\r').unwrap_or(raw);
            let raw = raw.trim_start_matches([' ', '\t']);
            let body = raw
                .strip_prefix("//")
                .map_or(raw, |r| r.strip_prefix(' ').unwrap_or(r));
            body.trim_end()
        })
        .collect();

    // Drop leading/trailing empty lines: the empty "//" lines a run is often
    // framed with collapse away here.
    let first = lines.iter().position(|l| !l.is_empty());
    let last = lines.iter().rposition(|l| !l.is_empty());
    let (Some(first), Some(last)) = (first, last) else {
        return None; // a run of only empty "//" lines: leave it as-is
    };
    let lines = &lines[first..=last];

    // Refuse rather than corrupt: a "*/" closes the block early, and a "/*"
    // opens a NESTED block in Rust (block comments nest), leaving the outer
    // block unterminated, so it would swallow the rest of the file. In C/C++ a
    // nested "/*" also trips -Wcomment (often fatal under -Werror). Skip both.
    if lines.iter().any(|l| l.contains("*/") || l.contains("/*")) {
        return None;
    }

    // Vote the dominant ending rather than flipping the whole block to CRLF on
    // a single stray "\r\n", matching reflow::join_lines. A one-line run has no
    // interior break to vote on, and the block being built has several, so fall
    // back to the ending that follows the comment: guessing LF there writes LF
    // breaks into a CRLF file the moment reflow keeps the block multi-line. A
    // one-line node carries the CR of its own CRLF (the "\n" sits just past
    // "end_byte", which is why the replacement below gives that CR back), so
    // "fallback_ending" reads the LF alone and cannot be trusted on its own.
    let newlines = c.text.matches('\n').count();
    let nl = if newlines == 0 {
        if c.text.ends_with('\r') || c.fallback_ending == "\r\n" {
            "\r\n"
        } else {
            "\n"
        }
    } else if c.text.matches("\r\n").count() * 2 >= newlines {
        "\r\n"
    } else {
        "\n"
    };

    // The opener starts at start_byte (after the indent, which is outside the
    // span), so it carries no indent; continuation and closer lines do.
    let mut text = String::from("/*");
    for line in lines {
        text.push_str(nl);
        text.push_str(indent);
        text.push_str(" *");
        if !line.is_empty() {
            text.push(' ');
            text.push_str(line);
        }
    }
    text.push_str(nl);
    text.push_str(indent);
    text.push_str(" */");

    Some(rewrite::Replacement {
        start: c.start_byte,
        end: c.end_byte - usize::from(c.text.ends_with('\r')),
        text,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::IndentConfig;

    /// Conversion followed by the standard reflow pipeline, which is what
    /// "--to-blocks" runs. Layout assertions below depend on both.
    fn run(src: &str, lang: Language) -> String {
        let mut pool = ParserPool::new();
        let conversions = line_run_replacements(src, lang, &mut pool).unwrap();
        let converted = rewrite::apply(src, &conversions);
        let reps = crate::plan(&converted, lang, 80, IndentConfig::default(), &mut pool).unwrap();
        rewrite::apply(&converted, &reps)
    }

    #[test]
    fn empty_bookend_run_collapses_to_single_block() {
        let src = "int a;\n//\n// This is comment\n//\nint b;\n";
        let out = run(src, Language::C);
        assert_eq!(out, "int a;\n/* This is comment */\nint b;\n");
    }

    #[test]
    fn multiline_run_becomes_block() {
        let src = "int a;\n// first line of a longer thought\n// second line of the same thought\nint b;\n";
        let out = run(src, Language::C);
        assert!(out.contains("/*"), "must convert, got:\n{out}");
        assert!(!out.contains("//"), "no // should remain, got:\n{out}");
    }

    #[test]
    fn multiline_run_crlf_keeps_crlf() {
        let src = "int a;\r\n// first line\r\n// second line\r\nint b;\r\n";
        let out = run(src, Language::C);
        assert_eq!(out, "int a;\r\n/* first line second line */\r\nint b;\r\n");
    }

    #[test]
    fn standalone_single_line_converts() {
        let src = "int a;\n// standalone single line\nint b;\n";
        let out = run(src, Language::C);
        assert_eq!(out, "int a;\n/* standalone single line */\nint b;\n");
    }

    #[test]
    fn standalone_single_line_crlf_keeps_crlf() {
        let src = "int a;\r\n// standalone single line\r\nint b;\r\n";
        let out = run(src, Language::C);
        assert_eq!(out, "int a;\r\n/* standalone single line */\r\nint b;\r\n");
    }

    #[test]
    fn trailing_comment_stays() {
        let src = "int a; // trailing stays\nint b;\n";
        let out = run(src, Language::C);
        assert_eq!(out, src, "trailing // must not convert, got:\n{out}");
    }

    #[test]
    fn acsl_line_annotation_is_not_converted() {
        let src = "int a;\n//@ assert a >= 0;\nint b;\n";
        let out = run(src, Language::C);
        assert_eq!(out, src, "Frama-C ACSL must stay a line annotation");
    }

    #[test]
    fn rust_open_marker_is_not_converted() {
        // "/*" opens a NESTED block in Rust; wrapping would leave the outer
        // block unterminated. Must stay a // comment.
        let src = "// discuss the /* opener token here\n// and more text in the run\nfn f() {}\n";
        let out = run(src, Language::Rust);

        // Content keeps its "/*", but it must stay a // comment (no block
        // opener/closer synthesized around it).
        assert!(
            out.contains("// discuss"),
            "must stay a // comment, got:\n{out}"
        );
        assert!(
            !out.contains("*/"),
            "must not synthesize a block closer, got:\n{out}"
        );
    }

    #[test]
    fn doc_comments_untouched() {
        let src = "/// doc line one of a run\n/// doc line two of a run\nfn f() {}\n";
        let out = run(src, Language::Rust);
        assert_eq!(out, src, "/// runs must stay doc comments, got:\n{out}");
    }

    #[test]
    fn unsafe_close_marker_is_not_converted() {
        // A "*/" in the body would close the block early. The run must stay a
        // "//" comment (reflow may still repack it), never become "/* */".
        let src = "int a;\n// see foo */ bar\n// and more text here\nint b;\n";
        let out = run(src, Language::C);
        assert!(!out.contains("/*"), "must not create a block, got:\n{out}");
        assert!(out.contains("//"), "must stay a // comment, got:\n{out}");
    }

    #[test]
    fn comment_in_string_is_not_touched() {
        let src = "const char *s = \"http://example.com\";\n// real comment one of two\n// real comment two of two\n";
        let out = run(src, Language::C);
        assert!(
            out.contains("\"http://example.com\""),
            "URL in string preserved"
        );
    }

    #[test]
    fn shell_and_asm_yield_no_replacements() {
        let mut pool = ParserPool::new();
        let src = "# a shell comment\nx=0\n";
        assert!(
            line_run_replacements(src, Language::Shell, &mut pool)
                .unwrap()
                .is_empty()
        );
        let asm = "/* a block */\n	mov r0, r1\n";
        assert!(
            line_run_replacements(asm, Language::Asm, &mut pool)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn file_start_spdx_run_is_not_converted() {
        let src = "// SPDX-License-Identifier: GPL-2.0\n// Copyright (C) 2026 Example Corp.\n//\n// Driver for the example device.\nint f(void) { return 0; }\n";
        let out = run(src, Language::C);
        assert!(
            out.starts_with("// SPDX-License-Identifier: GPL-2.0\n"),
            "the tag must stay on line 1 in // form, got:\n{out}"
        );
    }

    #[test]
    fn single_line_crlf_run_keeps_crlf_when_it_stays_multiline() {
        // No interior newline to vote on, so the ending has to come from the
        // comment's surroundings. A wrapped block must not carry LF breaks into
        // a CRLF file.
        let src = "int a;\r\n// this is a very long standalone single line comment that certainly exceeds the eighty column limit\r\nint b;\r\n";
        let out = run(src, Language::C);
        assert!(out.contains("/*"), "must convert, got:\n{out}");
        assert!(
            !out.replace("\r\n", "").contains('\n'),
            "no bare LF may survive in a CRLF file, got:\n{out:?}"
        );
    }
}
