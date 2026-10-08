//! 源码格式化器的布局与保持不变量测试。

use super::{FormatError, format, format_named};
use teal_compiler::{LexicalTokenKind, Source, tokenize};

fn format_fixture(input: &str) -> String {
    format_named("format.tl", input).expect("fixture should format")
}

#[test]
fn formats_blocks_aggregates_attributes_and_comments() {
    let input = concat!(
        "// heading\r\n",
        "fn main(){let value=Thing{left:1,right:[2,3]};",
        "if value.left>0{return;// line end\r\n}else{// branch\r\nreturn;}}",
    );

    assert_eq!(
        format_fixture(input),
        concat!(
            "// heading\n",
            "fn main() {\n",
            "    let value = Thing { left: 1, right: [2, 3] };\n",
            "    if value.left > 0 {\n",
            "        return; // line end\n",
            "    } else { // branch\n",
            "        return;\n",
            "    }\n",
            "}\n",
        )
    );
}

#[test]
fn formats_public_class_fields_without_changing_visibility_tokens() {
    let input = "class Record{pub visible:int,hidden:string}";
    let formatted = format_fixture(input);

    assert_eq!(
        formatted,
        "class Record { pub visible: int, hidden: string }\n"
    );
    assert_eq!(format_fixture(&formatted), formatted);
    let before = tokenize(&Source::new("before.tl", input));
    let after = tokenize(&Source::new("after.tl", formatted));
    assert_eq!(
        before
            .iter()
            .map(|token| (token.kind, token.text.as_str()))
            .collect::<Vec<_>>(),
        after
            .iter()
            .map(|token| (token.kind, token.text.as_str()))
            .collect::<Vec<_>>()
    );
}

#[test]
fn preserves_ranges_unary_operators_literals_and_token_spelling() {
    let input = concat!(
        "fn sample(){",
        "let minimum=-2147483648;",
        "let decimal=.5;",
        "let full=values[..];",
        "let suffix=values[2..];",
        "let text=\"// not a comment\";",
        "for index in ..3{return;}",
        "if !false&&minimum<0{return;}",
        "}",
    );
    let formatted = format_fixture(input);
    let before = tokenize(&Source::new("before.tl", input));
    let after = tokenize(&Source::new("after.tl", formatted.clone()));

    assert_eq!(
        before
            .iter()
            .map(|token| (token.kind, token.text.as_str()))
            .collect::<Vec<_>>(),
        after
            .iter()
            .map(|token| (token.kind, token.text.as_str()))
            .collect::<Vec<_>>()
    );
    assert!(formatted.contains("-2147483648"));
    assert!(formatted.contains("values[..]"));
    assert!(formatted.contains("values[2..]"));
    assert!(formatted.contains("for index in ..3"));
    assert!(formatted.contains("if !false && minimum < 0"));
    assert_eq!(
        after
            .iter()
            .filter(|token| token.kind == LexicalTokenKind::Comment)
            .count(),
        0
    );
}

#[test]
fn formatting_is_idempotent_and_preserves_one_blank_line() {
    let input = "fn first(){return;}\n\n\nfn second() {\n    return;\n}\n";
    let once = format_fixture(input);
    let twice = format_fixture(&once);

    assert_eq!(once, twice);
    assert!(once.contains("}\n\nfn second"));
    assert!(!once.contains("}\n\n\nfn second"));
}

#[test]
fn separates_adjacent_named_functions_at_the_same_scope() {
    let formatted = format_fixture("fn first() {} task second() {}");

    assert_eq!(formatted, "fn first() {}\n\ntask second() {}\n");
}

#[test]
fn separates_impl_methods_before_comments_and_attributes_without_block_tail_blank_line() {
    let formatted = format_fixture(
        "class Item {} impl Item { fn first() {} // keep\n @tag pub task second() { return; } }",
    );

    assert!(formatted.contains("fn first() {} // keep\n\n    @tag pub task second()"));
    assert!(!formatted.contains("\n    }\n\n}"));
}

#[test]
fn places_the_separator_before_a_documentation_comment_for_the_next_declaration() {
    let formatted = format_fixture("fn first() {}\n// second\npub task second() {}");

    assert!(formatted.contains("fn first() {}\n\n// second\npub task second() {}"));
}

#[test]
fn recognizes_braced_attributes_as_declaration_prefixes() {
    let formatted = format_fixture("fn first() {} @Tag { label: \"method\" } pub task second() {}");

    assert!(formatted.contains("fn first() {}\n\n@Tag"));
}

#[test]
fn keeps_await_tight_after_member_access() {
    assert_eq!(
        format_fixture("fn main() { run_agent(options). await; }"),
        "fn main() {\n    run_agent(options).await;\n}\n"
    );
}

#[test]
fn anonymous_function_expressions_do_not_create_declaration_separators() {
    let formatted = format_fixture("fn outer() { let callback = fn() { return; }; } fn next() {}");

    assert!(
        formatted.contains("let callback = fn() {\n        return;\n    };\n}\n\nfn next() {}")
    );
    assert!(!formatted.contains("return;\n\n    };"));
}

#[test]
fn empty_and_comment_only_sources_have_stable_endings() {
    assert_eq!(format("").unwrap(), "");
    assert_eq!(format("// note").unwrap(), "// note\n");
}

#[test]
fn rejects_incomplete_syntax_without_returning_partial_output() {
    let error = format_named("broken.tl", "fn main() {").unwrap_err();
    let FormatError::Syntax(error) = error else {
        panic!("invalid syntax should remain a syntax diagnostic");
    };
    assert_eq!(error.source_name(), "broken.tl");
}
