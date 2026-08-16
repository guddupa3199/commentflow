// Comments in this crate quote identifiers with "double quotes" rather than
// markdown code spans, so clippy's pedantic doc_markdown lint fires on every
// one of them. The convention is deliberate and the lint cannot be satisfied
// without reverting it, so silence that one lint and keep the rest of the
// pedantic group usable as a signal.
#![allow(clippy::doc_markdown)]

use anyhow::{Context, Result, bail};
use std::borrow::Cow;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use commentflow::cli::{Args, Mode};
use commentflow::config::Resolved;
use commentflow::parse::{Language, ParserPool};
use commentflow::{config, diff, parse, rewrite};

fn main() -> ExitCode {
    match run(Args::parse()) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("commentflow: {e:#}");
            ExitCode::from(2)
        }
    }
}

/// One unit of work resolved from the CLI: either a real file path or the
/// stdin filter source.
enum Input {
    Stdin,
    Path(PathBuf),
}

/// The run-wide switches. Bundled because they are identical for every file
/// in a batch, and because threading them separately pushed "emit" past a
/// sensible parameter count.
struct Opts {
    mode: Mode,
    to_blocks: bool,
}

/// Where a Write-mode result goes.
enum WriteTarget<'a> {
    File(&'a Path),
    Stdout,
}

fn run(args: Args) -> Result<ExitCode> {
    let opts = Opts {
        mode: args.mode(),
        to_blocks: args.to_blocks,
    };
    let inputs = collect_inputs(&args)?;
    let mut any_would_change = false;
    let mut any_io_error = false;

    // One parser per language, built lazily on first use. Reused for every
    // subsequent file of that language so we pay "Parser::new" + "set_language"
    // exactly once per batch run instead of once per file.
    let mut pool = parse::ParserPool::new();

    // Memoize .clang-format discovery by directory so a flat list of files in
    // one tree walks to root once, not once per file. Lives for this run only.
    let mut config_cache = config::ResolveCache::new();

    for input in &inputs {
        let (label, result) = match input {
            Input::Path(path) => (
                path.display().to_string(),
                process_path(path, &args, &opts, &mut pool, &mut config_cache),
            ),
            Input::Stdin => (
                "<stdin>".to_string(),
                process_stdin(&args, &opts, &mut pool),
            ),
        };
        match result {
            Ok(would_change) => any_would_change |= would_change,
            Err(e) => {
                eprintln!("commentflow: {label}: {e:#}");
                any_io_error = true;
            }
        }
    }

    if any_io_error {
        Ok(ExitCode::from(2))
    } else if opts.mode == Mode::Check && any_would_change {
        Ok(ExitCode::from(1))
    } else {
        Ok(ExitCode::from(0))
    }
}

/// Resolve the CLI input-source class into work units. "Args::validate"
/// already guarantees exactly one of "paths" / "--files-from"; this enforces
/// the rules about "-", which lives inside "paths" and so cannot be expressed
/// as an option conflict.
fn collect_inputs(args: &Args) -> Result<Vec<Input>> {
    let mut out = Vec::new();
    if let Some(spec) = &args.files_from {
        for s in read_path_list(spec)? {
            resolve_path_arg(Path::new(&s), &mut out)?;
        }
        return Ok(out);
    }
    if args.paths.len() == 1 && args.paths[0] == "-" {
        if args.lang.is_none() {
            bail!("--lang is required when reading source from stdin ('-')");
        }
        return Ok(vec![Input::Stdin]);
    }
    if args.paths.iter().any(|p| p == "-") {
        bail!("'-' (stdin) cannot be combined with file paths");
    }
    for p in &args.paths {
        resolve_path_arg(Path::new(p), &mut out)?;
    }
    Ok(out)
}

/// A directory arg expands (recursively, extension-gated) into its supported
/// files; anything else becomes a single Input::Path whose extension and
/// readability are validated later (so an explicit bad-extension file still
/// errors, but unsupported files discovered in a tree are silently skipped).
fn resolve_path_arg(path: &Path, out: &mut Vec<Input>) -> Result<()> {
    // Use symlink_metadata (no follow): only a REAL directory is walked. A
    // symlink-to-directory is not expanded: "never follow symlinks" applies to
    // an explicit arg too, so a dir symlink can't redirect the walk into
    // another tree. A symlink-to-file still flows to process_path, which reads
    // through it as before (back-compat for "commentflow link.c").
    let is_real_dir = std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_dir());
    if is_real_dir {
        walk_dir(path, out)?;
    } else {
        out.push(Input::Path(path.to_path_buf()));
    }
    Ok(())
}

/// Recursively collect supported source files under "dir". v0.1 defaults:
/// never follow symlinks (dir or file), descend hidden dirs except VCS
/// metadata (".git"/".hg"/".svn"), ignore ".gitignore", and visit siblings in
/// bytewise path order so output is reproducible across filesystems.
fn walk_dir(dir: &Path, out: &mut Vec<Input>) -> Result<()> {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
        .with_context(|| format!("read directory: {}", dir.display()))?
        .map(|e| e.map(|e| e.path()))
        .collect::<std::io::Result<Vec<_>>>()
        .with_context(|| format!("read directory entry under: {}", dir.display()))?;
    entries.sort();
    for path in entries {
        let ft = std::fs::symlink_metadata(&path)
            .with_context(|| format!("stat: {}", path.display()))?
            .file_type();
        if ft.is_symlink() {
            continue;
        }
        if ft.is_dir() && !is_vcs_metadata_dir(&path) {
            walk_dir(&path, out)?;
        } else if ft.is_file() && parse::detect_language(&path).is_ok() {
            out.push(Input::Path(path));
        }
    }
    Ok(())
}

fn is_vcs_metadata_dir(path: &Path) -> bool {
    matches!(
        path.file_name().and_then(|n| n.to_str()),
        Some(".git" | ".hg" | ".svn")
    )
}

/// Read a newline- or NUL-delimited path list (file, or "-" for stdin).
/// Blank entries are skipped; a trailing "\r" (CRLF lists) is dropped.
fn read_path_list(spec: &str) -> Result<Vec<String>> {
    let raw = if spec == "-" {
        read_stdin().context("read --files-from list from stdin")?
    } else {
        std::fs::read_to_string(spec).with_context(|| format!("read --files-from list: {spec}"))?
    };

    // Pick ONE delimiter: a NUL-delimited list (from "find -print0" / "git
    // ls-files -z") may contain paths with embedded newlines, so a dual "['\n',
    // '\0']" split would shred them. NUL presence selects NUL.
    let delim = if raw.contains('\0') { '\0' } else { '\n' };
    Ok(raw
        .split(delim)
        .map(|s| s.strip_suffix('\r').unwrap_or(s))
        .filter(|s| !s.is_empty())
        .map(String::from)
        .collect())
}

fn read_stdin() -> Result<String> {
    let mut buf = String::new();
    std::io::stdin().read_to_string(&mut buf)?;
    Ok(buf)
}

/// Write to stdout, treating a broken pipe as success. A Unix filter
/// ("commentflow - ... | head") must exit cleanly when its reader goes away,
/// not panic the way "print!" does on EPIPE.
fn write_stdout(s: &str) -> Result<()> {
    match std::io::stdout().write_all(s.as_bytes()) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => Ok(()),
        Err(e) => Err(e.into()),
    }
}

fn process_path(
    path: &Path,
    args: &Args,
    opts: &Opts,
    pool: &mut ParserPool,
    config_cache: &mut config::ResolveCache,
) -> Result<bool> {
    let lang = parse::detect_language(path)?;

    // One unified walk reads both ColumnLimit and the indent keys from the same
    // .clang-format text, memoized per directory across the batch.
    let resolved = config_cache.resolve(path, args.column_limit)?;
    if resolved.column_limit == 0 {
        return Ok(false);
    }
    let source = std::fs::read_to_string(path)?;
    emit(
        &source,
        lang,
        &resolved,
        path,
        WriteTarget::File(path),
        pool,
        opts,
    )
}

fn process_stdin(args: &Args, opts: &Opts, pool: &mut ParserPool) -> Result<bool> {
    // "collect_inputs" already rejected stdin without --lang.
    let lang = args.lang.expect("--lang validated present").to_language();
    let resolved = config::resolve_cwd(args.column_limit)?;
    let source = read_stdin()?;
    if resolved.column_limit == 0 {
        // Reflow disabled, but the filter contract still emits the source.
        if opts.mode == Mode::Write {
            write_stdout(&source)?;
        }
        return Ok(false);
    }
    emit(
        &source,
        lang,
        &resolved,
        Path::new("<stdin>"),
        WriteTarget::Stdout,
        pool,
        opts,
    )
}

/// Source lines spanned by a set of replacements, for the "--dry-run" summary.
fn count_lines(reps: &[rewrite::Replacement], planned_against: &str) -> usize {
    reps.iter()
        .map(|r| planned_against[r.start..r.end].matches('\n').count() + 1)
        .sum()
}

/// Run the pipeline and act on the result per mode. Returns whether the
/// source would change (only meaningful for "--check").
fn emit(
    source: &str,
    lang: Language,
    resolved: &Resolved,
    label: &Path,
    target: WriteTarget,
    pool: &mut ParserPool,
    opts: &Opts,
) -> Result<bool> {
    // "--to-blocks" is a separate pass AHEAD of reflow, not a rule inside it:
    // reflow's contract is that comment style is preserved, and this is the one
    // opt-in that changes it. Everything after this point plans against a
    // source that just happens to have blocks where "//" runs were.
    let conversions = if opts.to_blocks {
        commentflow::convert::line_run_replacements(source, lang, pool)?
    } else {
        Vec::new()
    };
    let converted: Cow<str> = if conversions.is_empty() {
        Cow::Borrowed(source)
    } else {
        Cow::Owned(rewrite::apply(source, &conversions))
    };
    let replacements = commentflow::plan(
        &converted,
        lang,
        resolved.column_limit,
        resolved.indent,
        pool,
    )?;

    // Key every mode on the applied bytes rather than on "are there
    // replacements". A replacement set that cancels out would otherwise rename
    // a fresh temp file over the target on every run (same content, new inode
    // and mtime, so make rebuilds forever) while --check reports it clean.
    let new_source: Cow<str> = if replacements.is_empty() {
        Cow::Borrowed(converted.as_ref())
    } else {
        Cow::Owned(rewrite::apply(&converted, &replacements))
    };
    let changed = new_source.as_ref() != source;

    match opts.mode {
        Mode::Write => {
            match target {
                WriteTarget::File(path) => {
                    if changed {
                        rewrite::write_atomic(path, &new_source)?;
                    }
                }
                // Filter mode: always emit the result, changed or not.
                WriteTarget::Stdout => write_stdout(&new_source)?,
            }
            Ok(false)
        }
        Mode::Check => Ok(changed),
        Mode::Diff => {
            if changed {
                diff::print_unified(source, &new_source, label);
            }
            Ok(false)
        }
        Mode::DryRun => {
            if changed {
                // Count BOTH passes. Reporting only the reflow ranges printed
                // "0 comment range(s) would change" for a file that
                // "--to-blocks" rewrites and reflow then leaves alone. Each
                // pass's ranges index the source it was planned against.
                let line_count =
                    count_lines(&conversions, source) + count_lines(&replacements, &converted);
                println!(
                    "{}: {} comment range(s) would change ({} source line(s)); rerun without --dry-run to write",
                    label.display(),
                    conversions.len() + replacements.len(),
                    line_count
                );
            } else {
                println!("{}: no changes", label.display());
            }
            Ok(false)
        }
    }
}
