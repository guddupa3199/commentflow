//! Comment scanning for assembly (".s" / ".S").
//!
//! No tree-sitter grammar: assembly has no single syntax to parse. GAS picks
//! its line-comment character per target, and every candidate is load-bearing
//! syntax on some other target. "#" is a comment on x86/RISC-V/MIPS but the
//! immediate prefix on ARM32 ("mov r0, #1") and the cpp directive character in
//! a ".S" file; "@" is a comment on ARM32 but a relocation suffix on x86-64
//! ("foo@GOTPCREL") and the ELF section-type marker in a .section directive;
//! ";" separates statements in GAS x86; "!" is ARM writeback. The source file
//! does not state its target, so none of them is decidable here.
//!
//! "/* ... */" is the one form GAS accepts on every target, and it is the one
//! that carries the prose worth reflowing (file headers, function banners,
//! register-usage blocks). This scanner finds those and nothing else. A "/*"
//! that a target-specific line comment could have swallowed is skipped rather
//! than guessed at: mangling an immediate operand is not a cosmetic failure.
//!
//! When a block comment opens on a line the scanner skipped ("mov r0, #1 /*
//! note"), scanning resumes inside that comment's body, so a stray "/*" in its
//! prose can be claimed. Harmless by construction: the first "*/" after any
//! offset inside a comment is that same comment's closer, so the claimed span
//! is always a sub-range of a real comment and no code byte is reachable.
//!
//! Returns byte spans in source order; the caller turns them into "Comment"s.

/// Characters that begin a line comment on *some* target. Anything after one
/// of them on the line is unscannable, so the rest of the line is skipped.
/// This only ever suppresses a candidate comment; it never emits one.
fn opens_line_comment(bytes: &[u8], i: usize) -> bool {
    match bytes[i] {
        b'#' | b'@' | b';' | b'!' => true,

        // "//" (AArch64). On targets where "/" is division this can't start a
        // statement anyway, so skipping the line costs nothing.
        b'/' => bytes.get(i + 1) == Some(&b'/'),
        _ => false,
    }
}

/// True when "line" starts with a character that opens a line comment on some
/// assembler target. "scan" never claims those, so callers that need to know
/// "is this line a comment?" (rather than "which spans do we rewrite?") ask
/// here instead of consulting the extracted comment list.
pub fn opens_line_comment_at(line: &str) -> bool {
    let t = line.trim_start();
    t.starts_with(['#', '@', ';', '!']) || t.starts_with("//")
}

pub fn scan(source: &str) -> Vec<(usize, usize)> {
    let bytes = source.as_bytes();
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < bytes.len() {
        match bytes[i] {
            // Block comment: the only form we claim.
            b'/' if bytes.get(i + 1) == Some(&b'*') => {
                match source[i + 2..].find("*/") {
                    Some(rel) => {
                        let end = i + 2 + rel + 2;
                        out.push((i, end));
                        i = end;
                    }

                    // Unterminated: the assembler would reject the file. Emit
                    // nothing and stop rather than guess at an end.
                    None => break,
                }
            }
            b'"' => {
                i += 1;
                while i < bytes.len() {
                    match bytes[i] {
                        // A trailing backslash cannot escape the newline: a GAS
                        // string does not span lines. Stop here so string mode
                        // can't run on and swallow the next line's comment.
                        b'\\' if bytes.get(i + 1) == Some(&b'\n') => break,
                        b'\\' => i += 2,
                        b'"' => {
                            i += 1;
                            break;
                        }

                        // A GAS string does not span a line. Stopping here
                        // keeps one unterminated quote from desyncing the rest
                        // of the file.
                        b'\n' => break,
                        _ => i += 1,
                    }
                }
            }

            // GAS character constant: "'a", with no closing quote (an
            // apostrophe followed by a double quote is a perfectly good
            // double-quote literal). Consume the quoted char so it can't open a
            // phantom string or comment.
            b'\'' => {
                let escaped = bytes.get(i + 1) == Some(&b'\\');
                i += if escaped { 3 } else { 2 };
            }
            _ if opens_line_comment(bytes, i) => {
                let line_end = match source[i..].find('\n') {
                    Some(rel) => i + rel + 1,
                    None => break,
                };

                // If the skipped text opens a block comment that does not close
                // on this line, that comment's body continues below. Resume
                // after its closer, or scanning would restart *inside* it and
                // claim a stray "/*" in its prose as a comment of its own.
                let seg = &source[i..line_end];
                match seg.rfind("/*") {
                    Some(open) if !seg[open + 2..].contains("*/") => {
                        let body = i + open + 2;
                        i = match source[body..].find("*/") {
                            Some(rel) => body + rel + 2,
                            None => break,
                        };
                    }
                    _ => i = line_end,
                }
            }
            _ => i += 1,
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::scan;

    fn spans(src: &str) -> Vec<&str> {
        scan(src).into_iter().map(|(s, e)| &src[s..e]).collect()
    }

    #[test]
    fn finds_block_comments() {
        assert_eq!(
            spans("/* header */\n\tmov %eax, %ebx\t/* trailing */\n"),
            ["/* header */", "/* trailing */"]
        );
        assert_eq!(
            spans("/*\n * multi\n * line\n */\nnop\n"),
            ["/*\n * multi\n * line\n */"]
        );
    }

    #[test]
    fn skips_strings_and_char_constants() {
        assert_eq!(
            spans(".ascii \"/* not a comment */\"\n"),
            Vec::<&str>::new()
        );
        assert_eq!(spans(".ascii \"esc \\\" /* no */\"\n"), Vec::<&str>::new());

        // An apostrophe followed by a double quote is a char constant, not the
        // start of a string: a scanner that got this wrong would swallow the
        // real comment that follows.
        assert_eq!(spans(".byte '\"\n/* real */\n"), ["/* real */"]);
        assert_eq!(spans(".byte '\\'\n/* real */\n"), ["/* real */"]);
    }

    #[test]
    fn skips_after_target_specific_line_comment_chars() {
        // "#" is a comment on x86 and an immediate on ARM; either way the rest
        // of the line is not ours.
        assert_eq!(spans("# note /* not mine */\n/* mine */\n"), ["/* mine */"]);
        assert_eq!(spans("@ arm comment /* not mine */\n"), Vec::<&str>::new());
        assert_eq!(spans("mov r0, #1 /* not mine */\n"), Vec::<&str>::new());
        assert_eq!(spans("// aarch64 /* not mine */\n"), Vec::<&str>::new());
        assert_eq!(
            spans(".section .note,\"a\",@progbits\n/* mine */\n"),
            ["/* mine */"]
        );
    }

    #[test]
    fn a_comment_opened_on_a_skipped_line_is_consumed_whole() {
        // "#" abandons line 1, but the "/*" on it opens a comment whose body
        // runs on below. Resuming inside that body would claim the stray "/*"
        // in its prose and reflow a fragment of someone else's comment.
        let src = "\tmov r0, #1 /* set r0 to one\n/* stray opener in the prose\n */\n\tret\n";
        assert_eq!(spans(src), Vec::<&str>::new());

        // The same shape, but the comment closes on its own line: nothing to
        // consume, and the next real comment is still found.
        assert_eq!(
            spans("\tmov r0, #1 /* one */\n/* mine */\n"),
            ["/* mine */"]
        );
    }

    #[test]
    fn unterminated_block_emits_nothing() {
        assert_eq!(spans("nop\n/* runs off the end\n"), Vec::<&str>::new());
    }

    #[test]
    fn non_ascii_bytes_do_not_split_spans() {
        let src = "/* café */\n.ascii \"naïve\"\n/* two */\n";
        assert_eq!(spans(src), ["/* café */", "/* two */"]);
    }

    #[test]
    fn multibyte_char_constant_does_not_panic() {
        // The "'x" skip is byte-counted, so it can land mid-UTF-8. Every
        // continuation byte is >= 0x80 and falls through to the plain advance,
        // so no slice is ever taken off a char boundary.
        assert_eq!(spans(".byte 'é\n/* real */\n"), ["/* real */"]);
        assert_eq!(spans(".ascii \"\\é\"\n/* real */\n"), ["/* real */"]);
    }
}
