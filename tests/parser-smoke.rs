use tree_sitter::Parser;

#[test]
fn c_grammar_emits_comment_node() {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_c::LANGUAGE.into())
        .expect("set C language");
    let src = "int x = 0; // a line comment\n";
    let tree = parser.parse(src, None).expect("parse C");
    let root = tree.root_node();
    assert!(
        walk_find_kind(root, "comment"),
        "no comment node in C parse"
    );
}

#[test]
fn cpp_grammar_emits_comment_node() {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_cpp::LANGUAGE.into())
        .expect("set C++ language");
    let src = "int x = 0; /* a block comment */\n";
    let tree = parser.parse(src, None).expect("parse C++");
    let root = tree.root_node();
    assert!(
        walk_find_kind(root, "comment"),
        "no comment node in C++ parse"
    );
}

#[test]
fn bash_grammar_emits_comment_node() {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_bash::LANGUAGE.into())
        .expect("set Bash language");

    // Tree-sitter-bash exposes shebangs and regular "#" comments under the same
    // "comment" node kind. Our extraction layer is responsible for recognising
    // the file-leading "#!" line and skipping it, since the grammar will not.
    let src = "#!/usr/bin/env bash\necho hi # tail comment\n# standalone\n";
    let tree = parser.parse(src, None).expect("parse bash");
    let root = tree.root_node();

    assert!(
        walk_find_kind(root, "comment"),
        "no comment node in bash parse"
    );
}

#[test]
fn rust_grammar_emits_line_and_block_comments() {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_rust::LANGUAGE.into())
        .expect("set Rust language");

    let line_src = "fn f() {} // line\n";
    let tree = parser.parse(line_src, None).expect("parse Rust line");
    assert!(
        walk_find_kind(tree.root_node(), "line_comment"),
        "no line_comment node in Rust parse"
    );

    let block_src = "fn f() {} /* block */\n";
    let tree = parser.parse(block_src, None).expect("parse Rust block");
    assert!(
        walk_find_kind(tree.root_node(), "block_comment"),
        "no block_comment node in Rust parse"
    );
}

fn walk_find_kind(node: tree_sitter::Node, kind: &str) -> bool {
    if node.kind() == kind {
        return true;
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if walk_find_kind(child, kind) {
            return true;
        }
    }
    false
}

#[test]
fn parser_pool_extracts_correctly_across_mixed_languages() {
    // Pin the pool's reuse contract: one pool handed across multiple calls of
    // different languages must produce the same output as creating a fresh pool
    // every call. Switching language back and forth ensures per-language slots
    // stay isolated.
    use commentflow::parse::{Language, ParserPool, extract_comments, extract_comments_with};
    let mut pool = ParserPool::new();
    let c_src = "int x; // comment\n";
    let cpp_src = "int x; /* block */\n";
    let rust_src = "fn f() {} // line\n";
    let sh_src = "echo hi # tail\n";

    for _ in 0..3 {
        let a = extract_comments_with(c_src, Language::C, &mut pool).unwrap();
        let a_ref = extract_comments(c_src, Language::C).unwrap();
        assert_eq!(a.len(), a_ref.len());

        let b = extract_comments_with(cpp_src, Language::Cpp, &mut pool).unwrap();
        let b_ref = extract_comments(cpp_src, Language::Cpp).unwrap();
        assert_eq!(b.len(), b_ref.len());

        let r = extract_comments_with(rust_src, Language::Rust, &mut pool).unwrap();
        let r_ref = extract_comments(rust_src, Language::Rust).unwrap();
        assert_eq!(r.len(), r_ref.len());

        let s = extract_comments_with(sh_src, Language::Shell, &mut pool).unwrap();
        let s_ref = extract_comments(sh_src, Language::Shell).unwrap();
        assert_eq!(s.len(), s_ref.len());
    }
}
