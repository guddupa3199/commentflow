/*
 * DO NOT REFLOW THIS FILE. It is test input, not source: running the tool on it
 * destroys the very shapes it exists to hold. Exclude tests/corpus when
 * reflowing the repo's own sources.
 *
 * Assembly shapes: a macro whose backslash continuation makes a newline a
 * terminator rather than whitespace, target-specific line-comment characters
 * that are never claimed, a "/*" that a line comment already swallowed, and
 * quoting that must not leak.
 */

#define SAVE_REGS \
	/* inside a continuation chain a newline ends the macro, so this comment is frozen: no reflow, and no blank line above it either */ \
	push %rbx; \
	push %rbp

/* Register usage. This block is ordinary prose and is the one shape here that reflows, so it is written long enough to prove the packer ran.
 *
 *	%rax	return value
 *	%rdi	first argument
 */

	.section .rodata,"a",@progbits	/* an ELF section-type marker, not a comment */
	.ascii	"a /* inside a string is not a comment opener"
	.byte	'/', '*'

# a "#" line comment on x86 is left alone, /* even when it holds an opener */
@ an ARM32 line comment is left alone too, /* opener and all */
; a GAS statement separator, /* likewise */
// an AArch64 line comment, /* likewise */

	mov	$1, %eax	/* trailing block comment, the one form claimed on every target */
	ret
