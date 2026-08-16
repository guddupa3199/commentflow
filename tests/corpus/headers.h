/*
 * DO NOT REFLOW THIS FILE. It is test input, not source: running the tool on it
 * destroys the very shapes it exists to hold. Exclude tests/corpus when
 * reflowing the repo's own sources.
 *
 * Shapes taken from real C headers that broke this tool, reproduced here rather
 * than vendored (the originals are SDK files with their own licenses). Every
 * one of these was found by running the binary over 228 macOS SDK headers, not
 * by writing test cases. The corpus test only asserts that the file is STABLE:
 * the in-process helper reruns the pipeline on its own output and requires a
 * no-op. Add a shape here whenever a real file surprises you.
 */

/* Public header file for the library. bzlib.h
 *
 * A run of one-line block comments that merge: rule, labeled rule, rule.
 * Deleting the rules used to leave the dashes on the label for the next pass to
 * strip. See is_bsd_dash_opener and strip_merged_member.
 */

/* CAPI3REF: Run-Time Library Version Numbers
 * KEYWORDS: sqlite3_version sqlite3_sourceid
 *
 * sqlite3.h's doc-generator directives. Packing these into the prose below them
 * destroys the markup, so a run of banner rows holds its lines.
 */

/* File: aio.h
	Author:	Somebody <nobody@example.com>
 * 05-Feb-2003 created.
 *
 * A tab-indented banner with no star markers.
 */

/* Any application code which uses these declarations will get the following:
 *
 * compile link run
 *
 * funcA: normal normal normal funcB: warning normal normal funcF: normal weak
 * on 10.3 normal typeA: warning
 *
 * AvailabilityMacros.h's table. Its separator row reads as a bookend whose
 * label is another dash run; collapsing that made a fake setext underline and
 * split the header off, which the next run could not see. See bookend_match.
 */

/* unsigned int isn't 100% accurate as it should be a strict 4-byte value.
 * XXX: Tcl is currently UCS-2 and planning UTF-16 for the Unicode
 * XXX: string rep that Tcl_UniChar represents.
 * XXX: Changing the size of Tcl_UniChar is not supported.
 */

/* Reproduction of the COPYRIGHT file:
 *
Copyright 1995-2002 University Corporation for Atmospheric Research
 *
 * A metadata line that lost its marker in the original. It must not be
 * rewrapped into the prose around it, and it must not be retouched either.
 */

/* Interaction flags (should be passed about in a control) Automatic (default):
 * use defaults, prompt otherwise
 *  Interactive: prompt always
 *  Quiet: never prompt
 */

int f(int a, int b);
