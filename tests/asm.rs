// Assembly (.s / .S) support. The scanner claims "/* */" and nothing else,
// because every other comment character in GAS is load-bearing syntax on some
// other target and the file does not say which target it is. These tests pin
// the two things that matter: the block comments do reflow, and not one
// instruction byte moves.
//
// The instruction-byte checks assert the exact expected output rather than
// diffing "everything the scanner did not claim". A helper built on "scan"
// would delete the same bytes from both sides and pass even if "scan" wrongly
// claimed an instruction.

use std::path::PathBuf;

use commentflow::config::IndentConfig;
use commentflow::parse::{self, Language, ParserPool};
use commentflow::{plan, rewrite};

fn run(source: &str, column_limit: usize) -> String {
    let mut pool = ParserPool::new();
    let reps = plan(
        source,
        Language::Asm,
        column_limit,
        IndentConfig::default(),
        &mut pool,
    )
    .unwrap();
    rewrite::apply(source, &reps)
}

#[test]
fn detects_both_extensions() {
    for p in ["entry.s", "entry.S", "arch/arm/kernel/head.S"] {
        assert_eq!(
            parse::detect_language(&PathBuf::from(p)).unwrap(),
            Language::Asm,
            "{p}"
        );
    }
}

#[test]
fn reflows_a_prematurely_wrapped_header_block() {
    let src = "\
/*
 * Save the caller-saved registers before
 * entering the C runtime, because the
 * trampoline may clobber them.
 */
\tpushq %rbp
";
    let out = run(src, 80);
    assert_eq!(
        out,
        "\
/*
 * Save the caller-saved registers before entering the C runtime, because the
 * trampoline may clobber them.
 */
\tpushq %rbp
"
    );
    assert_eq!(run(&out, 80), out, "not idempotent");
}

#[test]
fn x86_source_keeps_every_instruction_byte() {
    // "#" line comment, "@progbits" section marker, a "/*" inside a string, and
    // a "$'\n'" character constant: every one of them is a scanner trap.
    let src = "\
#include <asm/linkage.h>
\t.section .note.GNU-stack,\"\",@progbits
\t.globl entry
entry:
\t/* a comment that is long enough to need repacking when the column limit is short */
\tmovq $'\\n', %rax\t# x86 line comment mentioning /* nothing */
\t.ascii \"/* not a comment */\"
\tret
";
    let out = run(src, 50);
    assert_eq!(
        out,
        "\
#include <asm/linkage.h>
\t.section .note.GNU-stack,\"\",@progbits
\t.globl entry
entry:
\t/* a comment that is long enough to need
\t * repacking when the column limit is
\t * short
\t */
\tmovq $'\\n', %rax\t# x86 line comment mentioning /* nothing */
\t.ascii \"/* not a comment */\"
\tret
"
    );
    assert_eq!(run(&out, 50), out, "not idempotent");
}

#[test]
fn arm_immediates_and_at_comments_are_untouched() {
    let src = "\
\t.arch armv7-a
\tmov r0, #1 /* this trailing block shares a line with a # immediate */
\tldr r1, [r2, #4]!
@ an ARM line comment that is quite long and mentions /* a block */ inline
\tbx lr
";
    // The "#" and "@" lines are unscannable, so nothing on them is claimed.
    assert!(commentflow::asm::scan(src).is_empty());
    assert_eq!(run(src, 40), src);
}

#[test]
fn blank_line_lands_above_a_multiline_comment() {
    let src = "\
\tcall setup
\t/* The stack pointer is deliberately left misaligned here so the callee's own prologue can fix it up. */
\tret
";
    let out = run(src, 60);
    assert_eq!(
        out,
        "\
\tcall setup

\t/* The stack pointer is deliberately left misaligned
\t * here so the callee's own prologue can fix it up.
\t */
\tret
"
    );
    assert_eq!(run(&out, 60), out, "not idempotent");
}

#[test]
fn comments_inside_a_macro_continuation_are_untouched() {
    // A ".S" file is cpp-preprocessed, so it is the language most likely to
    // carry "#define X \" macros. Inside a continuation chain a newline is a
    // terminator, not whitespace: reflowing the comment or prepending a blank
    // line above it truncates the macro body and silently changes the code the
    // assembler emits.
    let src = "\
#define SAVE_ALL \\
\t/* Save every callee-saved register before we clobber them in the trampoline path. */ \\
\tmov x19, x0; \\
\tmov x20, x1
\t.text
foo:
\tSAVE_ALL
\tret
";
    assert_eq!(run(src, 60), src);
    assert_eq!(run(src, 200), src);
}

#[test]
fn a_line_comment_above_is_not_fractured_by_the_blank_line_rule() {
    // The scanner claims "/* */" and nothing else, so a "#" or "@" comment
    // above this block is invisible to the extracted comment list. Transform 5
    // must still see it as a comment and leave the stacked header intact.
    for lead in ["# x86 note about the entry point", "@ arm note about entry"] {
        let src = format!(
            "{lead}\n\t/* The stack pointer is deliberately left misaligned here so the callee can fix it up. */\n\tret\n"
        );
        let out = run(&src, 60);
        assert!(
            !out.contains("\n\n"),
            "stacked header fractured by a blank line:\n{out}"
        );
    }
}

#[test]
fn label_lines_do_not_get_a_blank_line() {
    let src = "\
entry:
\t/* This explanatory block is long enough that reflow keeps it on several lines at this limit. */
\tret
";
    let out = run(src, 50);
    assert!(
        !out.contains("entry:\n\n"),
        "blank line after label:\n{out}"
    );
    assert!(
        out.starts_with("entry:\n\t/*"),
        "label line changed:\n{out}"
    );
}
