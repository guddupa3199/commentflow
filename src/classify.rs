use crate::parse::Language;

/// Which comment form the source used. Preserved end to end: nothing in the
/// pipeline may turn one style into another (the sole exception is the opt-in
/// "--to-blocks" pass, which runs before any of this).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Style {
    /// "//" or a shell "#" run.
    Line,
    /// "/* ... */".
    Block,
    /// "///" or "//!".
    DocLine,
    /// "/** ... */" or "/*! ... */".
    DocBlock,
}

/// Which documentation convention a doc comment follows. Taken from the source
/// language, not from the marker: a "///" in C++ carries Doxygen tag semantics
/// while the same marker in Rust honors Rustdoc markdown sections.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocFlavor {
    Doxygen,
    Rustdoc,
    /// Shell, which has no doc-comment convention: prose only.
    None,
}

#[derive(Debug, Clone)]
pub struct Kind {
    pub style: Style,
    pub flavor: DocFlavor,
}

pub fn classify(text: &str, lang: Language) -> Kind {
    let style = classify_style(text);
    let flavor = match style {
        Style::DocLine | Style::DocBlock => match lang {
            Language::Rust => DocFlavor::Rustdoc,
            _ => DocFlavor::Doxygen,
        },
        _ => DocFlavor::None,
    };
    Kind { style, flavor }
}

/// The style a comment's own bytes spell out. Used both to classify the source
/// and, in "plan", to check that the rewritten text still spells out the same
/// style before it is allowed to replace anything.
pub(crate) fn classify_style(text: &str) -> Style {
    if text.starts_with("///") || text.starts_with("//!") {
        Style::DocLine
    } else if text.starts_with("//") {
        Style::Line
    } else if crate::textline::block_is_doc(text) {
        Style::DocBlock
    } else if text.starts_with("/*") {
        Style::Block
    } else {
        Style::Line
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_line_comments() {
        assert_eq!(classify("// foo", Language::C).style, Style::Line);
        assert_eq!(classify("/// foo", Language::Rust).style, Style::DocLine);
        assert_eq!(classify("//! foo", Language::Rust).style, Style::DocLine);
    }

    #[test]
    fn classifies_block_comments() {
        assert_eq!(classify("/* foo */", Language::C).style, Style::Block);
        assert_eq!(classify("/** foo */", Language::C).style, Style::DocBlock);
        assert_eq!(classify("/*! foo */", Language::Cpp).style, Style::DocBlock);
    }

    #[test]
    fn doc_flavor_per_language() {
        assert_eq!(
            classify("/// foo", Language::Cpp).flavor,
            DocFlavor::Doxygen
        );
        assert_eq!(
            classify("/// foo", Language::Rust).flavor,
            DocFlavor::Rustdoc
        );
        assert_eq!(classify("// foo", Language::C).flavor, DocFlavor::None);
    }
}
