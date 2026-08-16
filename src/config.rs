use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

const DEFAULT_COLUMN_LIMIT: usize = 80;
const DEFAULT_TAB_WIDTH: usize = 8;
const DEFAULT_INDENT_WIDTH: usize = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UseTab {
    Never,
    ForIndentation,
    ForContinuationAndIndentation,
    Always,
    AlignWithSpaces,
}

#[derive(Debug, Clone, Copy)]
pub struct IndentConfig {
    pub tab_width: usize,
    pub indent_width: usize,
    pub use_tab: UseTab,
}

impl Default for IndentConfig {
    fn default() -> Self {
        Self {
            tab_width: DEFAULT_TAB_WIDTH,
            indent_width: DEFAULT_INDENT_WIDTH,
            use_tab: UseTab::Never,
        }
    }
}

/// Everything one ".clang-format" walk yields.
#[derive(Debug, Clone)]
pub struct Resolved {
    pub column_limit: usize,
    pub indent: IndentConfig,
}

/// The settings that apply with no ".clang-format" in sight, which is also what
/// "--column-limit N" selects: the flag says the caller is not interested in
/// what the tree has to say about either key.
fn overridden(column_limit: usize) -> Resolved {
    Resolved {
        column_limit,
        indent: IndentConfig::default(),
    }
}

/// Resolve the column limit and the indent config for one file with a single
/// upward ".clang-format" walk. Keeping both keys on one walk is the point:
/// resolving them separately doubled the filesystem traffic of a batch run.
///
/// The override is honored before the path is canonicalized, so it works on a
/// path that does not exist yet. That branch reads no file at all.
pub fn resolve(file: &Path, cli_col_override: Option<usize>) -> Result<Resolved> {
    if let Some(n) = cli_col_override {
        return Ok(overridden(n));
    }
    let file_abs = std::fs::canonicalize(file)
        .with_context(|| format!("canonicalize input path: {}", file.display()))?;
    let file_dir = file_abs.parent().unwrap_or(Path::new("/")).to_path_buf();

    let cwd_abs = std::env::current_dir().context("read current working directory")?;
    let cwd_abs = std::fs::canonicalize(&cwd_abs).unwrap_or(cwd_abs);
    let under_cwd = file_abs.starts_with(&cwd_abs);

    let (column_limit, indent) =
        read_keys(find_clang_format_for_paths(&file_dir, &cwd_abs, under_cwd));

    Ok(Resolved {
        column_limit,
        indent,
    })
}

/// Per-invocation memo over "resolve", keyed by the canonical parent
/// directory of the input file. A flat file list in one tree repeats the same
/// upward ".clang-format" walk for every sibling; caching by directory does it
/// once. Create one per run (the cache must not outlive an invocation, since
/// the filesystem can change between runs).
#[derive(Default)]
pub struct ResolveCache {
    by_dir: HashMap<PathBuf, Resolved>,
    /// Count of underlying "resolve" calls, each of which performs exactly one
    /// ".clang-format" walk. A cache hit and a "--column-limit" bypass do not
    /// increment it. Per-instance so the count is race-free under parallel
    /// tests (a global static would be shared across the whole binary).
    walks: usize,
}

impl ResolveCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of ".clang-format" walks performed so far. Test instrumentation.
    pub fn walk_count(&self) -> usize {
        self.walks
    }

    pub fn resolve(&mut self, file: &Path, cli_col_override: Option<usize>) -> Result<Resolved> {
        // "--column-limit" bypasses discovery entirely, so there is no walk to
        // save: don't pay for a cache lookup, and don't count a walk.
        if cli_col_override.is_some() {
            return resolve(file, cli_col_override);
        }
        let dir = std::fs::canonicalize(file)
            .with_context(|| format!("canonicalize input path: {}", file.display()))?
            .parent()
            .unwrap_or(Path::new("/"))
            .to_path_buf();
        if let Some(r) = self.by_dir.get(&dir) {
            return Ok(r.clone());
        }
        let resolved = resolve(file, cli_col_override)?;
        self.walks += 1;
        self.by_dir.insert(dir, resolved.clone());
        Ok(resolved)
    }
}

/// Stdin ("-") resolution: there is no input file to walk from, so anchor the
/// ".clang-format" discovery at the current working directory. "--column-limit"
/// still bypasses discovery entirely, mirroring "resolve".
pub fn resolve_cwd(cli_col_override: Option<usize>) -> Result<Resolved> {
    if let Some(n) = cli_col_override {
        return Ok(overridden(n));
    }
    let cwd_abs = std::env::current_dir().context("read current working directory")?;
    let cwd_abs = std::fs::canonicalize(&cwd_abs).unwrap_or(cwd_abs);

    let (column_limit, indent) = read_keys(walk_for_clang_format_text(&cwd_abs));

    Ok(Resolved {
        column_limit,
        indent,
    })
}

/// Read every key we care about out of one ".clang-format" text, falling back
/// to the defaults per key. "None" (no file found) yields all defaults.
fn read_keys(text: Option<String>) -> (usize, IndentConfig) {
    let text = text.as_deref();
    (
        text.and_then(parse_column_limit)
            .unwrap_or(DEFAULT_COLUMN_LIMIT),
        text.and_then(parse_indent_config).unwrap_or_default(),
    )
}

/// Walk the discovery precedence and return the first ".clang-format" content
/// found: file-dir walk when the file is under cwd, cwd-anchored walk
/// otherwise. One file governs every key we read.
fn find_clang_format_for_paths(file_dir: &Path, cwd: &Path, under_cwd: bool) -> Option<String> {
    if under_cwd {
        walk_for_clang_format_text(file_dir)
    } else {
        walk_for_clang_format_text(cwd).or_else(|| walk_for_clang_format_text(file_dir))
    }
}

fn walk_for_clang_format_text(start: &Path) -> Option<String> {
    let mut cur: &Path = start;
    loop {
        let candidate = cur.join(".clang-format");
        if let Ok(text) = std::fs::read_to_string(&candidate) {
            return Some(text);
        }
        match cur.parent() {
            Some(p) if p != cur => cur = p,
            _ => return None,
        }
    }
}

fn parse_indent_config(text: &str) -> Option<IndentConfig> {
    let mut tab_width: Option<usize> = None;
    let mut indent_width: Option<usize> = None;
    let mut use_tab: Option<UseTab> = None;
    for line in text.lines() {
        if let Some(n) = parse_int_key(line, "TabWidth") {
            tab_width = Some(n);
        }
        if let Some(n) = parse_int_key(line, "IndentWidth") {
            indent_width = Some(n);
        }
        if let Some(v) = parse_string_key(line, "UseTab") {
            use_tab = match v.as_str() {
                "Never" => Some(UseTab::Never),
                "ForIndentation" => Some(UseTab::ForIndentation),
                "ForContinuationAndIndentation" => Some(UseTab::ForContinuationAndIndentation),
                "Always" => Some(UseTab::Always),
                "AlignWithSpaces" => Some(UseTab::AlignWithSpaces),
                _ => None,
            };
        }
    }
    if tab_width.is_none() && indent_width.is_none() && use_tab.is_none() {
        return None;
    }
    Some(IndentConfig {
        // Floor tab_width at 1: it divides in advance_col's tab-stop math, so a
        // "TabWidth: 0" would panic with divide-by-zero. Treat 0 as "unset".
        tab_width: tab_width.filter(|&n| n > 0).unwrap_or(DEFAULT_TAB_WIDTH),
        indent_width: indent_width.unwrap_or(DEFAULT_INDENT_WIDTH),
        use_tab: use_tab.unwrap_or(UseTab::Never),
    })
}

fn parse_int_key(line: &str, key: &str) -> Option<usize> {
    let stripped = strip_key_prefix(line, key)?;
    let mut chars = stripped.chars().peekable();
    skip_spaces(&mut chars);
    let mut digits = String::new();
    while let Some(&c) = chars.peek() {
        if c.is_ascii_digit() {
            digits.push(c);
            chars.next();
        } else {
            break;
        }
    }
    if digits.is_empty() {
        return None;
    }
    skip_spaces(&mut chars);
    if chars.peek().is_some_and(|&c| c != '#') {
        return None;
    }
    digits.parse().ok()
}

fn parse_string_key(line: &str, key: &str) -> Option<String> {
    let stripped = strip_key_prefix(line, key)?;
    let mut chars = stripped.chars().peekable();
    skip_spaces(&mut chars);
    let mut val = String::new();
    while let Some(&c) = chars.peek() {
        if c.is_ascii_alphanumeric() || c == '_' {
            val.push(c);
            chars.next();
        } else {
            break;
        }
    }
    if val.is_empty() {
        return None;
    }
    skip_spaces(&mut chars);
    if chars.peek().is_some_and(|&c| c != '#') {
        return None;
    }
    Some(val)
}

fn strip_key_prefix<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    // Skip leading whitespace by counting bytes (ASCII " " and "\t" are 1 byte
    // each, so this is safe and matches char_indices semantics).
    let ws_bytes = line
        .bytes()
        .take_while(|b| *b == b' ' || *b == b'\t')
        .count();
    let rest = line.get(ws_bytes..)?;
    let after = rest.strip_prefix(key)?;
    // Skip whitespace after the key by byte count; same ASCII guarantee.
    let after_ws_bytes = after
        .bytes()
        .take_while(|b| *b == b' ' || *b == b'\t')
        .count();
    let after_value = after.get(after_ws_bytes..)?;
    after_value.strip_prefix(':')
}

fn parse_column_limit(text: &str) -> Option<usize> {
    text.lines()
        .find_map(|line| parse_int_key(line, "ColumnLimit"))
}

#[cfg(test)]
fn walk_for_clang_format(start: &Path) -> Option<usize> {
    walk_for_clang_format_text(start).and_then(|text| parse_column_limit(&text))
}

fn skip_spaces(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) {
    while let Some(&c) = chars.peek() {
        if c == ' ' || c == '\t' {
            chars.next();
        } else {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn parses_basic_column_limit() {
        assert_eq!(parse_column_limit("ColumnLimit: 100\n"), Some(100));
        assert_eq!(parse_column_limit("ColumnLimit:80"), Some(80));
        assert_eq!(parse_column_limit("  ColumnLimit:   120  \n"), Some(120));
    }

    #[test]
    fn ignores_other_keys() {
        let text = "Language: Cpp\nBasedOnStyle: Google\nIndentWidth: 4\n";
        assert_eq!(parse_column_limit(text), None);
    }

    #[test]
    fn accepts_trailing_comment() {
        assert_eq!(
            parse_column_limit("ColumnLimit: 100 # project default"),
            Some(100)
        );
    }

    #[test]
    fn picks_first_match_among_many_lines() {
        let text = "BasedOnStyle: Google\nColumnLimit: 99\nColumnLimit: 77\n";
        assert_eq!(parse_column_limit(text), Some(99));
    }

    #[test]
    fn column_limit_zero_disables() {
        assert_eq!(parse_column_limit("ColumnLimit: 0"), Some(0));
    }

    #[test]
    fn tab_width_zero_falls_back_to_default() {
        // "TabWidth: 0" must not reach advance_col's divide. It floors to the
        // default rather than panicking on a divide-by-zero.
        let cfg = parse_indent_config("TabWidth: 0\n").expect("indent key present");
        assert_eq!(cfg.tab_width, DEFAULT_TAB_WIDTH);
    }

    #[test]
    fn resolve_reads_every_key_from_one_file() {
        let tmp = under_target("cf-resolve-test");
        fs::write(
            tmp.join(".clang-format"),
            "ColumnLimit: 72\nTabWidth: 4\nIndentWidth: 3\nUseTab: Always\n",
        )
        .unwrap();
        let file = tmp.join("src.c");
        fs::write(&file, "int x;\n").unwrap();

        let r = resolve(&file, None).unwrap();
        assert_eq!(r.column_limit, 72);
        assert_eq!(r.indent.tab_width, 4);
        assert_eq!(r.indent.indent_width, 3);
        assert_eq!(r.indent.use_tab, UseTab::Always);

        // The override bypasses discovery for the indent keys too, not just for
        // the column limit.
        let overridden = resolve(&file, Some(50)).unwrap();
        assert_eq!(overridden.column_limit, 50);
        assert_eq!(overridden.indent.use_tab, UseTab::Never);

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn rejects_malformed() {
        assert_eq!(parse_column_limit("ColumnLimit"), None);
        assert_eq!(parse_column_limit("ColumnLimit: "), None);
        assert_eq!(parse_column_limit("ColumnLimit: abc"), None);
        assert_eq!(parse_column_limit("ColumnLimit: 80junk"), None);
        assert_eq!(parse_string_key("UseTab: Never junk", "UseTab"), None);
    }

    fn write_clang_format(dir: &Path, n: usize) {
        fs::write(dir.join(".clang-format"), format!("ColumnLimit: {n}\n")).unwrap();
    }

    #[test]
    fn walk_finds_nearest_ancestor() {
        let tmp = tempdir();
        let nested = tmp.join("a/b/c");
        fs::create_dir_all(&nested).unwrap();
        write_clang_format(&tmp, 80);
        write_clang_format(&tmp.join("a"), 100);
        assert_eq!(walk_for_clang_format(&nested), Some(100));
        assert_eq!(walk_for_clang_format(&tmp.join("a/b")), Some(100));
    }

    #[test]
    fn walk_stops_at_nearest_clang_format_without_column_limit() {
        let tmp = tempdir();
        let nested = tmp.join("a/b/c");
        fs::create_dir_all(&nested).unwrap();
        write_clang_format(&tmp, 100);
        fs::write(tmp.join("a/.clang-format"), "BasedOnStyle: LLVM\n").unwrap();
        assert_eq!(walk_for_clang_format(&nested), None);
    }

    #[test]
    fn outside_cwd_precedence_stops_at_cwd_config_without_column_limit() {
        let tmp = tempdir();
        let cwd = tmp.join("cwd/subdir");
        let file_dir = tmp.join("external/project/src");
        fs::create_dir_all(&cwd).unwrap();
        fs::create_dir_all(&file_dir).unwrap();
        fs::write(tmp.join("cwd/.clang-format"), "BasedOnStyle: LLVM\n").unwrap();
        fs::write(
            tmp.join("external/project/.clang-format"),
            "ColumnLimit: 120\n",
        )
        .unwrap();

        let text = find_clang_format_for_paths(&file_dir, &cwd, false).unwrap();
        assert_eq!(parse_column_limit(&text), None);
    }

    #[test]
    fn outside_cwd_cwd_walk_wins_when_it_has_column_limit() {
        let tmp = tempdir();
        let cwd = tmp.join("cwd/subdir");
        let file_dir = tmp.join("external/project/src");
        fs::create_dir_all(&cwd).unwrap();
        fs::create_dir_all(&file_dir).unwrap();
        fs::write(tmp.join("cwd/.clang-format"), "ColumnLimit: 100\n").unwrap();
        fs::write(
            tmp.join("external/project/.clang-format"),
            "ColumnLimit: 120\n",
        )
        .unwrap();

        let text = find_clang_format_for_paths(&file_dir, &cwd, false).unwrap();
        assert_eq!(parse_column_limit(&text), Some(100));
    }

    #[test]
    fn outside_cwd_falls_back_to_file_dir_when_cwd_finds_nothing() {
        let tmp = tempdir();
        let cwd = tmp.join("cwd/subdir");
        let file_dir = tmp.join("external/project/src");
        fs::create_dir_all(&cwd).unwrap();
        fs::create_dir_all(&file_dir).unwrap();

        // The cwd-anchored walk goes all the way to filesystem root. If any
        // ancestor of "tmp" (a user's $HOME, /tmp, /var, /) has a
        // ".clang-format", that file wins before the fallback fires and this
        // test's premise breaks. Skip cleanly in that environment instead of
        // failing. The precedence rule itself is still pinned by the sibling
        // test outside_cwd_cwd_walk_wins_when_it_has_column_limit.
        if walk_for_clang_format_text(&cwd).is_some() {
            eprintln!(
                "skip outside_cwd_falls_back_to_file_dir_when_cwd_finds_nothing: ancestor .clang-format exists above {}",
                cwd.display()
            );
            return;
        }
        fs::write(
            tmp.join("external/project/.clang-format"),
            "ColumnLimit: 120\n",
        )
        .unwrap();

        let text = find_clang_format_for_paths(&file_dir, &cwd, false).unwrap();
        assert_eq!(parse_column_limit(&text), Some(120));
    }

    #[test]
    fn cache_skips_walk_for_sibling_files() {
        let tmp = under_target("cf-cache-test");
        fs::write(tmp.join(".clang-format"), "ColumnLimit: 70\n").unwrap();
        fs::write(tmp.join("a.c"), "int a;\n").unwrap();
        fs::write(tmp.join("b.c"), "int b;\n").unwrap();

        let mut cache = ResolveCache::new();
        let r1 = cache.resolve(&tmp.join("a.c"), None).unwrap();
        assert_eq!(r1.column_limit, 70);
        assert_eq!(cache.walk_count(), 1, "first file performs one walk");

        // Second file in the SAME dir must hit the cache: zero extra walks.
        let r2 = cache.resolve(&tmp.join("b.c"), None).unwrap();
        assert_eq!(r2.column_limit, 70);
        assert_eq!(
            cache.walk_count(),
            1,
            "sibling file must perform ZERO additional walks"
        );

        // --column-limit override bypasses discovery entirely: no walk.
        let r3 = cache.resolve(&tmp.join("a.c"), Some(50)).unwrap();
        assert_eq!(r3.column_limit, 50);
        assert_eq!(cache.walk_count(), 1, "override must not walk");

        let _ = fs::remove_dir_all(&tmp);
    }

    /// A fresh empty directory under "target/". Tests that need "resolve" to
    /// take its file-dir walk branch must sit under cwd, which rules out
    /// env::temp_dir (and dodges the macOS /var symlink while we are here).
    fn under_target(tag: &str) -> std::path::PathBuf {
        fresh_dir(std::env::current_dir().unwrap().join("target"), tag)
    }

    /// A fresh empty directory outside the source tree, for the walks that must
    /// climb to the filesystem root.
    fn tempdir() -> std::path::PathBuf {
        fresh_dir(std::env::temp_dir(), "commentflow-test")
    }

    /// Pid and nanos alone collided once under parallel test execution at the
    /// same timer tick, racing one test's remove_dir_all against another's
    /// setup. The atomic counter guarantees cross-thread uniqueness even when
    /// two concurrent callers read identical nanos.
    fn fresh_dir(base: std::path::PathBuf, tag: &str) -> std::path::PathBuf {
        static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = base.join(format!(
            "{tag}-{}-{seq}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }
}
