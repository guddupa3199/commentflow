use anyhow::{Context, Result, bail};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::parse::Comment;

static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone)]
pub struct Replacement {
    pub start: usize,
    pub end: usize,
    pub text: String,
}

/// The replacement that swaps a comment's source bytes for its reflowed text,
/// or "None" when the reflow produced nothing or reproduced the original.
pub fn make_replacement(c: &Comment, rewritten: String, source: &str) -> Option<Replacement> {
    let original = &source[c.start_byte..c.end_byte];
    if rewritten.is_empty() {
        return None;
    }
    let mut text = rewritten;
    if original.ends_with("\r\n") && !text.ends_with("\r\n") {
        text.push_str("\r\n");
    } else if original.ends_with('\n') && !text.ends_with('\n') {
        text.push('\n');
    } else if original.ends_with('\r') && !text.ends_with('\r') {
        // tree-sitter-bash captures the trailing "\r" of a CRLF comment inside
        // the node (C's "//"/block nodes do not). Re-append it so the span's
        // "\r" plus the out-of-span "\n" still form a CRLF separator, rather
        // than collapsing to a lone LF.
        text.push('\r');
    }
    if text == original {
        return None;
    }
    Some(Replacement {
        start: c.start_byte,
        end: c.end_byte,
        text,
    })
}

pub fn validate(reps: &[Replacement], source: &str, comments: &[Comment]) -> Result<()> {
    // Sort by (start, end) so a zero-width insertion at "x" orders before any
    // range that also starts at "x". Without the "end" tiebreaker the overlap
    // check below would read a boundary insert as an overlap (the range would
    // set "last_end = y > x", then the insert at "x" trips "x < last_end"). A
    // manual-page relocation never places its insert at another replacement's
    // start, but ordering the check correctly kills the latent fragility.
    let sorted = sorted_replacements(reps);
    let mut last_end = 0usize;

    // Both slices are sorted by start offset, so the containment search below
    // advances a cursor instead of rescanning "comments" from zero for every
    // replacement. That scan was O(replacements x comments); on a 2 MB file
    // with 16k changed comments it was most of the run time.
    let mut cursor = 0usize;
    for r in &sorted {
        if r.start < last_end {
            bail!("replacement ranges overlap at byte {}", r.start);
        }
        if r.start > r.end {
            bail!("replacement range inverted at byte {}", r.start);
        }
        if r.end > source.len() {
            bail!("replacement range past end of source");
        }

        // UTF-8 boundary safety: replace_range panics on a non-char-boundary
        // offset. A malformed range from earlier in the pipeline would crash
        // the process. Fail fast with a clear error.
        if !source.is_char_boundary(r.start) || !source.is_char_boundary(r.end) {
            bail!(
                "replacement [{}..{}] not on UTF-8 char boundaries",
                r.start,
                r.end
            );
        }

        // A zero-width insertion writes new bytes into a code region, the one
        // exception to "never touch non-comment bytes". Authorize it only at a
        // recorded relocation target: a comment's "relocate_before"
        // (manual-page hoist) or "param_shift" (drifted parameter comment). A
        // stray offset from the plan layer can't splice text into arbitrary
        // code. Empty-text zero-width spans are no-ops and always fine.
        if r.start == r.end {
            if !r.text.is_empty()
                && !comments.iter().any(|c| {
                    c.relocate_before == Some(r.start)
                        || c.param_shift.map(|s| s.insert_at) == Some(r.start)
                })
            {
                bail!(
                    "zero-width insertion at {} is not an authorized relocation target",
                    r.start
                );
            }
            last_end = r.end;
            continue;
        }

        // Comments are disjoint and sorted, so the first one ending past
        // "r.start" is the only possible container.
        while cursor < comments.len() && comments[cursor].end_byte <= r.start {
            cursor += 1;
        }
        let rest = &comments[cursor..];
        let contained = rest
            .first()
            .is_some_and(|c| r.start >= c.start_byte && r.end <= c.end_byte);
        if !contained {
            // Group span: a replacement covering several comment ranges plus
            // the whitespace between them. Walk the gaps directly instead of
            // painting a coverage bitmap: every byte not inside a comment must
            // be whitespace.
            let mut gap_start = r.start;
            for c in rest {
                if c.start_byte >= r.end {
                    break;
                }
                check_whitespace_gap(source, gap_start, c.start_byte.min(r.end), r)?;
                gap_start = gap_start.max(c.end_byte).min(r.end);
            }
            check_whitespace_gap(source, gap_start, r.end, r)?;
        }
        last_end = r.end;
    }
    Ok(())
}

/// Every byte in "[start, end)" must be whitespace: it lies inside a
/// replacement range but outside every comment, so rewriting it is only
/// authorized when it is the gap between two comments in a group.
fn check_whitespace_gap(source: &str, start: usize, end: usize, r: &Replacement) -> Result<()> {
    if start >= end {
        return Ok(());
    }
    let bytes = source.as_bytes();
    for (offset, &byte) in bytes[start..end].iter().enumerate() {
        if !byte.is_ascii_whitespace() {
            bail!(
                "replacement [{}..{}] covers non-comment non-whitespace byte at {}",
                r.start,
                r.end,
                start + offset
            );
        }
    }
    Ok(())
}

pub fn apply(source: &str, reps: &[Replacement]) -> String {
    // Build the result in one forward pass. The old form spliced end-to-start
    // with "replace_range", which memmoved the tail once per replacement; on a
    // file with thousands of changed comments that is quadratic for no reason.
    //
    // Sort by (start, end) ascending, the same total order "validate" uses, so
    // that when a zero-width insert shares a start with a range, the insert is
    // emitted first and lands just before the range, deterministically rather
    // than depending on "reps" order.
    let sorted = sorted_replacements(reps);
    let grown: usize = sorted.iter().map(|r| r.text.len()).sum();
    let mut out = String::with_capacity(source.len() + grown);
    let mut cut = 0usize;
    for r in &sorted {
        // "validate" rejects overlaps, so "r.start >= cut" always holds; the
        // clamp only keeps an unvalidated caller from panicking on a slice.
        out.push_str(&source[cut..r.start.max(cut)]);
        out.push_str(&r.text);
        cut = r.end.max(cut);
    }
    out.push_str(&source[cut.min(source.len())..]);
    out
}

fn sorted_replacements(reps: &[Replacement]) -> Vec<&Replacement> {
    let mut sorted: Vec<&Replacement> = reps.iter().collect();
    sorted.sort_by_key(|r| (r.start, r.end));
    sorted
}

pub fn write_atomic(path: &Path, content: &str) -> Result<()> {
    // Symlink safety: rename() onto a symlink replaces the symlink with a
    // regular file, leaving the real target unchanged. Resolve to the canonical
    // target first so we always rewrite the file the caller expects.
    let target: PathBuf = fs::canonicalize(path)
        .with_context(|| format!("canonicalize write target: {}", path.display()))?;
    let parent = target.parent().unwrap_or(Path::new("."));
    let file_name = target.file_name().and_then(|n| n.to_str()).unwrap_or("out");

    // Unique temp name: pid + monotonic counter + nanos. Avoids the race where
    // two concurrent runs on the same file truncate each other's temp file.
    let pid = process::id();
    let seq = TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    let tmp_name = format!(".{file_name}.commentflow.{pid}.{seq}.{nanos}.tmp");
    let tmp_path = parent.join(tmp_name);

    // Preserve original file permissions across the rename.
    let original_perms = fs::metadata(&target).ok().map(|m| m.permissions());

    {
        let mut opts = fs::OpenOptions::new();
        opts.write(true).create_new(true);

        // Ask for the original mode at CREATE time, not after the write. The
        // temp file holds a full copy of the source, so a 0600 file whose copy
        // is born 0644 is readable by anyone with the directory for as long as
        // the write takes. umask can only clear bits, so the file is never born
        // more permissive than the original; the exact bits are restored below.
        #[cfg(unix)]
        if let Some(p) = &original_perms {
            use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
            opts.mode(p.mode() & 0o7777);
        }
        let mut f = opts
            .open(&tmp_path)
            .with_context(|| format!("create temp file: {}", tmp_path.display()))?;

        // From here on the temp file exists, so every failure path has to
        // remove it. Leaking ".foo.h.commentflow.1234.0.567.tmp" into the
        // user's source tree on a full disk is not an acceptable way to fail.
        if let Err(e) = f.write_all(content.as_bytes()).and_then(|()| f.sync_all()) {
            let _ = fs::remove_file(&tmp_path);
            return Err(e).with_context(|| format!("write temp file: {}", tmp_path.display()));
        }
    }

    // Restore the exact bits: umask may have cleared some at create time. Best
    // effort on purpose. The create-time mode above means the temp file can
    // only ever be LESS permissive than the original (umask clears bits, it
    // never sets them), so a failure here leaks nothing; it only leaves a bit
    // that umask narrowed. Filesystems that reject chmod outright (exFAT,
    // FAT32, some SMB mounts) are common enough that failing the whole rewrite
    // over a narrower mode would be the worse outcome.
    if let Some(p) = original_perms {
        let _ = fs::set_permissions(&tmp_path, p);
    }
    if let Err(e) = fs::rename(&tmp_path, &target) {
        let _ = fs::remove_file(&tmp_path);
        return Err(e).with_context(|| format!("rename onto: {}", target.display()));
    }
    // Fsync the parent directory so the rename survives a crash.
    if let Ok(dir) = fs::File::open(parent) {
        let _ = dir.sync_all();
    }
    Ok(())
}
