/*
 * DO NOT REFLOW THIS FILE. It is test input, not source: running the tool on it
 * destroys the very shapes it exists to hold. Exclude tests/corpus when
 * reflowing the repo's own sources.
 *
 * C++ shapes: a Doxygen block whose tags must survive, a pipe-aligned table,
 * ASCII art, two backslash-continued line comments (frozen: a splice would
 * swallow the code below; the second's whitespace-only continuation line is
 * load-bearing, do not strip it), a fake nested opener, an unterminated block
 * at the end of the file, and CRLF line endings on every line, which every
 * emitted line must keep. The corpus test asserts the pipeline turns this file
 * into its ".expected" sibling.
 */

/**
 * @brief Construct a widget.
 *
 * @param name the widget's name, which must outlive the widget itself
 * @param size how many slots to reserve up front
 * @return a widget, or nullptr when the allocation fails
 */
class Widget;

/* State transitions. The bars are alignment, not prose, so the rows hold their
 * lines while the paragraphs around them reflow.
 *
 * | from    | event | to      |
 * |---------|-------|---------|
 * | idle    | start | running |
 * | running | pause | idle    |
 * | running | stop  | done    |
 */

/* A diagram passes through byte for byte, including the interior spacing that
 * lines the boxes up.
 *
 *      +--------+      +--------+
 *      | parser | ---> | reflow |
 *      +--------+      +--------+
 */

// A line comment ending in a backslash continues onto the next physical \
line, so the comment node spans both lines. Reflowing it can park the \
as the last thing on the last line, where it swallows the code below.
int continued_comment = 0;

// The shape that actually deleted a line of code. The continuation below \
   
int whitespace_continuation = 1;

/* A block comment holding what looks like a nested opener: /* the inner opener
 * is only text, and the first closer ends the comment.
 */
int after_fake_nesting = 1;

/* An unterminated block comment at the end of the file swallows everything
 * after it, so nothing may follow.
