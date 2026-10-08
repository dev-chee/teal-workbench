//! 跨文档 LSP 回归测试。

use serde_json::json;

use super::{DiagnosticTracker, ServerState, path_to_uri, valid_identifier};

#[test]
fn source_tool_diagnostics_follow_the_compiler_model_boundary() {
    let uri = "file:///memory/source-tool.tl";
    let mut state = ServerState::default();
    state.open(&json!({ "textDocument": {
        "uri": uri,
        "text": "@tool(\"local.echo\", \"Echo text\") task echo(value: string) -> string { return value; } task main() {}"
    } }));
    assert!(state.diagnostics(uri).is_empty());

    state.change(&json!({ "textDocument": { "uri": uri, "version": 2 },
        "contentChanges": [{ "text": "@tool(\"local.bad\", \"Bad handle\") task bad(value: Task<int>) -> int { return 1; } task main() {}" }]
    }));
    let diagnostics = state.diagnostics(uri);
    assert!(
        diagnostics.iter().any(|diagnostic| diagnostic["message"]
            .as_str()
            .is_some_and(|message| message.contains("tool") || message.contains("model"))),
        "{diagnostics:?}"
    );
}

#[test]
fn public_class_field_tokens_and_private_field_diagnostics_match_the_compiler() {
    let dependency_uri = "file:///memory/field-visibility-dependency.tl";
    let dependency = r#"
pub class Vault { secret: int, pub label: string }
pub fn make() -> Vault { return Vault { secret: 7, label: "safe" }; }
"#;
    let root_uri = "file:///memory/field-visibility-root.tl";
    let root = r#"import "field-visibility-dependency.tl" as dep; fn main() { let value = dep::make(); print(value.secret); }"#;
    let mut state = ServerState::default();
    state.open(&json!({ "textDocument": {
        "uri": dependency_uri,
        "text": dependency
    } }));
    state.open(&json!({ "textDocument": {
        "uri": root_uri,
        "text": root
    } }));

    let tokens = state.semantic_tokens(&json!({ "textDocument": { "uri": dependency_uri } }));
    let data = tokens["data"]
        .as_array()
        .expect("semantic token data should be an array");
    let mut line = 0_u64;
    let mut character = 0_u64;
    let public_offset = dependency.find("pub label").expect("public field modifier");
    let public_position = dependency[..public_offset]
        .chars()
        .fold((0_u64, 0_u64), |state, ch| {
            if ch == '\n' {
                (state.0 + 1, 0)
            } else {
                (state.0, state.1 + 1)
            }
        });
    let mut found_public_keyword = false;
    for token in data.as_chunks::<5>().0 {
        let delta_line = token[0].as_u64().expect("delta line");
        let delta_character = token[1].as_u64().expect("delta character");
        if delta_line == 0 {
            character += delta_character;
        } else {
            line += delta_line;
            character = delta_character;
        }
        if (line, character) == public_position {
            assert_eq!(token[2], 3, "pub token length");
            assert_eq!(token[3], 9, "pub should use the keyword legend entry");
            found_public_keyword = true;
        }
    }
    assert!(
        found_public_keyword,
        "public field modifier token was not emitted"
    );

    let diagnostics = state.diagnostics(root_uri);
    assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
    assert!(diagnostics[0]["message"].as_str().is_some_and(|message| {
        message.to_ascii_lowercase().contains("private")
            && message.contains("secret")
            && message.contains("field-visibility-dependency.tl")
    }));
}

#[test]
fn hover_details_of_bodyless_declarations_stop_at_the_declaration() {
    let uri = "file:///memory/declaration-detail.tl";
    let text = "trait Label { fn label() -> string; }\n\
                host \"example\" as api { task fetch(scope: string) -> string; }\n\
                dylib \"native\" { fn add(left: int, right: int) -> int; }\n\
                fn lookup(table: {string: int}) -> {string: int} { return table; }\n\
                fn main() { print(lookup({})); }";
    let mut state = ServerState::default();
    state.open(&json!({ "textDocument": { "uri": uri, "text": text } }));

    for (needle, expected) in [
        ("label", "fn label() -> string"),
        ("fetch", "task fetch(scope: string) -> string"),
        ("add", "fn add(left: int, right: int) -> int"),
        ("lookup", "fn lookup(table: {string: int}) -> {string: int}"),
    ] {
        let offset = text.find(needle).expect("fixture declares the name");
        // 夹具全部为 ASCII，字符列与字节列一致。
        let prefix = &text[..offset + needle.len() - 1];
        let line = prefix.matches('\n').count() as u64;
        let character = prefix.rsplit('\n').next().map_or(0, str::len) as u64;
        let hover = state.hover(&json!({
            "textDocument": { "uri": uri },
            "position": { "line": line, "character": character }
        }));
        let expected_value = format!("```teal\n{expected}\n```");
        assert_eq!(
            hover["contents"]["value"].as_str(),
            Some(expected_value.as_str()),
            "hover must stop at the declaration: {hover}"
        );
    }
}

#[test]
fn none_hover_and_uninitialized_diagnostics_remain_distinct() {
    let initialized_uri = "file:///memory/initialized-none.tl";
    let initialized = "fn main() { let value: none = none; print(value); }";
    let mut state = ServerState::default();
    state.open(&json!({ "textDocument": {
        "uri": initialized_uri,
        "text": initialized
    } }));
    let use_offset = initialized.rfind("value").expect("fixture uses value");
    let hover = state.hover(&json!({
        "textDocument": { "uri": initialized_uri },
        "position": { "line": 0, "character": use_offset }
    }));
    assert!(
        hover["contents"]["value"]
            .as_str()
            .is_some_and(|value| value.contains("none")),
        "initialized none should retain its source type: {hover}"
    );

    let uninitialized_uri = "file:///memory/uninitialized-none.tl";
    state.open(&json!({ "textDocument": {
        "uri": uninitialized_uri,
        "text": "fn main() { let value: none; print(value); }"
    } }));
    assert!(
        state
            .diagnostics(uninitialized_uri)
            .iter()
            .any(|diagnostic| {
                diagnostic["severity"] == 1
                    && diagnostic["message"].as_str().is_some_and(|message| {
                        message.to_ascii_lowercase().contains("uninitialized")
                    })
            })
    );
}

#[test]
fn nullable_array_hover_preserves_element_union_parentheses() {
    let uri = "file:///memory/nullable-array.tl";
    let text = "fn main() { let values: (int | none)[] = [1, none]; print(values); }";
    let mut state = ServerState::default();
    state.open(&json!({ "textDocument": { "uri": uri, "text": text } }));
    let use_offset = text.rfind("values").expect("fixture uses values");
    let hover = state.hover(&json!({
        "textDocument": { "uri": uri },
        "position": { "line": 0, "character": use_offset }
    }));

    assert!(
        hover["contents"]["value"]
            .as_str()
            .is_some_and(|value| value.contains("(int | none)[]")),
        "nullable element unions need parentheses in hover: {hover}"
    );
}

#[test]
fn builtin_and_package_catalogs_drive_lsp_completion_hover_signature_and_definition() {
    let uri = "file:///memory/builtins.tl";
    let text = "import \"std/fs.tl\" as fs; fn main() { range(1, 4); fs::; \"text\".; unknown::; }";
    let mut state = ServerState::default();
    state.initialize(&json!({}));
    state.open(&json!({ "textDocument": { "uri": uri, "text": text } }));

    let core_offset = text.find("range").expect("fixture has range") + "range".len();
    let core_completion = state.completion(&json!({
        "textDocument": { "uri": uri },
        "position": { "line": 0, "character": core_offset }
    }));
    let range = core_completion
        .as_array()
        .expect("completion is an array")
        .iter()
        .find(|item| item["label"] == "range")
        .expect("core range should be offered");
    assert_eq!(range["detail"], "fn range(start: int, end: int) -> int[]");

    let fs_offset = text.find("fs::").expect("fixture has fs namespace") + "fs::".len();
    let fs_completion = state.completion(&json!({
        "textDocument": { "uri": uri },
        "position": { "line": 0, "character": fs_offset }
    }));
    assert!(
        fs_completion
            .as_array()
            .expect("completion is an array")
            .iter()
            .any(|item| item["label"] == "read"
                && item["detail"] == "task read(path: string) -> string"),
        "filesystem package completion was missing: {fs_completion}"
    );

    let reflect_text = "fn main() { reflect::; }";
    let reflect_uri = "file:///memory/reflect.tl";
    state.open(&json!({
        "textDocument": { "uri": reflect_uri, "text": reflect_text }
    }));
    let reflect_completion = state.completion(&json!({
        "textDocument": { "uri": reflect_uri },
        "position": { "line": 0, "character": reflect_text.find("reflect::").unwrap() + "reflect::".len() }
    }));
    let reflect_item = reflect_completion
        .as_array()
        .expect("reflect completion is an array")
        .iter()
        .find(|item| item["label"] == "item")
        .expect("reflect::item should be offered");
    assert_eq!(reflect_item["detail"], "fn item(target: item) -> item");

    let method_offset =
        text.find("\"text\".").expect("fixture has string receiver") + "\"text\".".len();
    let method_completion = state.completion(&json!({
        "textDocument": { "uri": uri },
        "position": { "line": 0, "character": method_offset }
    }));
    assert!(
        method_completion
            .as_array()
            .expect("completion is an array")
            .iter()
            .any(
                |item| item["label"] == "uppercase" && item["detail"] == "fn uppercase() -> string"
            )
    );

    let unknown_offset = text
        .find("unknown::")
        .expect("fixture has unknown namespace")
        + "unknown::".len();
    let unknown_completion = state.completion(&json!({
        "textDocument": { "uri": uri },
        "position": { "line": 0, "character": unknown_offset }
    }));
    let unknown_labels = unknown_completion
        .as_array()
        .expect("completion is an array")
        .iter()
        .filter_map(|item| item["label"].as_str())
        .collect::<Vec<_>>();
    assert!(
        !unknown_labels
            .iter()
            .any(|label| { matches!(*label, "read" | "range" | "run" | "uppercase" | "status") }),
        "an arbitrary namespace must not receive builtin members: {unknown_labels:?}"
    );

    let hover = state.hover(&json!({
        "textDocument": { "uri": uri },
        "position": { "line": 0, "character": core_offset - 1 }
    }));
    assert!(
        hover["contents"]["value"]
            .as_str()
            .is_some_and(|value| value.contains("fn range(start: int, end: int) -> int[]"))
    );

    let signature = state.signature_help(&json!({
        "textDocument": { "uri": uri },
        "position": { "line": 0, "character": core_offset + 1 }
    }));
    assert_eq!(
        signature["signatures"][0]["label"],
        "fn range(start: int, end: int) -> int[]"
    );
    assert_eq!(signature["activeParameter"], 0);

    let definition = state.definition(&json!({
        "textDocument": { "uri": uri },
        "position": { "line": 0, "character": core_offset - 1 }
    }));
    assert_ne!(definition["uri"], uri);
    assert!(
        definition["uri"]
            .as_str()
            .is_some_and(|value| value.ends_with("/builtins.tli"))
    );

    let package_uri = "file:///memory/filesystem-usage.tl";
    let package_text =
        "import \"std/fs.tl\" as storage; task main() { storage::read(\"x\").await; storage::re; }";
    state.open(&json!({
        "textDocument": { "uri": package_uri, "text": package_text }
    }));
    let partial_offset = package_text.rfind("storage::re").unwrap() + "storage::re".len();
    let package_completion = state.completion(&json!({
        "textDocument": { "uri": package_uri },
        "position": { "line": 0, "character": partial_offset }
    }));
    let labels = package_completion
        .as_array()
        .expect("package completion is an array")
        .iter()
        .filter_map(|item| item["label"].as_str())
        .collect::<Vec<_>>();
    assert!(
        labels.contains(&"read"),
        "missing aliased package member: {labels:?}"
    );
    assert!(
        !labels.contains(&"decode_entry"),
        "private package member leaked"
    );
    assert!(!labels.contains(&"native_fs"), "private Host alias leaked");

    let read_offset = package_text.find("read(").unwrap() + 1;
    let read_hover = state.hover(&json!({
        "textDocument": { "uri": package_uri },
        "position": { "line": 0, "character": read_offset }
    }));
    assert!(
        read_hover["contents"]["value"]
            .as_str()
            .is_some_and(|value| value.contains("task read(path: string) -> string")),
        "unexpected package hover: {read_hover}"
    );
    let read_signature = state.signature_help(&json!({
        "textDocument": { "uri": package_uri },
        "position": { "line": 0, "character": package_text.find("read(").unwrap() + "read(".len() }
    }));
    assert_eq!(
        read_signature["signatures"][0]["label"],
        "task read(path: string) -> string"
    );
    let read_definition = state.definition(&json!({
        "textDocument": { "uri": package_uri },
        "position": { "line": 0, "character": read_offset }
    }));
    assert_ne!(read_definition["uri"], package_uri);
    assert!(
        read_definition["uri"]
            .as_str()
            .is_some_and(|value| value.replace('\\', "/").ends_with("/fs.tl")),
        "unexpected package definition: {read_definition}"
    );

    let reference_uri = "file:///memory/filesystem-references.tl";
    let reference_text = "import \"std/fs.tl\" as first; import \"std/fs.tl\" as second; task main() { first::read(\"a\").await; second::read(\"b\").await; }";
    state.open(&json!({
        "textDocument": { "uri": reference_uri, "text": reference_text }
    }));
    let reference_offset = reference_text.find("read(").unwrap() + 1;
    let reference_params = json!({
        "textDocument": { "uri": reference_uri },
        "position": { "line": 0, "character": reference_offset }
    });
    let references = state.references(&reference_params, true);
    let locations = references
        .as_array()
        .expect("package references are an array");
    assert_eq!(
        locations.len(),
        3,
        "unexpected package references: {references}"
    );
    assert_eq!(
        locations
            .iter()
            .filter(|location| location["uri"] == reference_uri)
            .count(),
        2
    );
    assert!(locations.iter().any(|location| {
        location["uri"]
            .as_str()
            .is_some_and(|value| value.replace('\\', "/").ends_with("/fs.tl"))
    }));
    let rename = state.rename(&json!({
        "textDocument": { "uri": reference_uri },
        "position": { "line": 0, "character": reference_offset },
        "newName": "load"
    }));
    assert!(
        rename.is_null(),
        "package member rename must remain closed: {rename}"
    );
}

#[test]
fn synchronous_console_builtins_expose_completion_hover_and_signatures() {
    let uri = "file:///memory/synchronous-console.tl";
    let text = "fn main() { print(\"value\"); let answer = input(\"Prompt: \"); pri; }";
    let mut state = ServerState::default();
    state.initialize(&json!({}));
    state.open(&json!({ "textDocument": { "uri": uri, "text": text } }));

    let completion_offset = text.rfind("pri;").unwrap() + "pri".len();
    let completion = state.completion(&json!({
        "textDocument": { "uri": uri },
        "position": { "line": 0, "character": completion_offset }
    }));
    assert!(
        completion
            .as_array()
            .expect("completion is an array")
            .iter()
            .any(|item| item["label"] == "print"
                && item["detail"] == "fn print(values: printable...)"),
        "print completion missing: {completion}"
    );

    let print_offset = text.find("print").unwrap() + 1;
    let hover = state.hover(&json!({
        "textDocument": { "uri": uri },
        "position": { "line": 0, "character": print_offset }
    }));
    assert!(
        hover["contents"]["value"]
            .as_str()
            .is_some_and(|value| value.contains("fn print(values: printable...)")),
        "unexpected print hover: {hover}"
    );

    let input_call = text.find("input(").unwrap();
    let signatures = state.signature_help(&json!({
        "textDocument": { "uri": uri },
        "position": { "line": 0, "character": input_call + "input(".len() }
    }));
    let labels = signatures["signatures"]
        .as_array()
        .expect("input signatures are an array")
        .iter()
        .filter_map(|signature| signature["label"].as_str())
        .collect::<Vec<_>>();
    assert!(labels.contains(&"fn input() -> string"));
    assert!(labels.contains(&"fn input(prompt: string) -> string"));
}

#[test]
fn rename_and_references_keep_unrelated_same_named_symbols_untouched() {
    let first_uri = "file:///memory/first.tl";
    let second_uri = "file:///memory/second.tl";
    let first = "fn first() { let value = 1; print(value); }";
    let second = "fn second() { let value = 2; print(value); }";
    let mut state = ServerState::default();
    state.open(&json!({ "textDocument": { "uri": first_uri, "text": first } }));
    state.open(&json!({ "textDocument": { "uri": second_uri, "text": second } }));
    let use_offset = first
        .rfind("value")
        .expect("fixture contains a variable use")
        + 1;
    let params = json!({
        "textDocument": { "uri": first_uri },
        "position": { "line": 0, "character": use_offset }
    });

    let rename = state.rename(&json!({
        "textDocument": { "uri": first_uri },
        "position": { "line": 0, "character": use_offset },
        "newName": "renamed"
    }));
    assert_eq!(
        rename["changes"][first_uri].as_array().map(Vec::len),
        Some(2)
    );
    assert!(rename["changes"].get(second_uri).is_none());

    let references = state.references(&params, true);
    assert_eq!(references.as_array().map(Vec::len), Some(2));
    assert!(
        references
            .as_array()
            .expect("references are an array")
            .iter()
            .all(|location| location["uri"] == first_uri)
    );
}

#[test]
fn identifier_validation_and_completion_follow_the_compiler_lexer() {
    for keyword in [
        "fn", "class", "struct", "type", "agent", "task", "pub", "const", "impl", "trait",
    ] {
        assert!(
            !valid_identifier(keyword),
            "{keyword} is reserved by the compiler lexer"
        );
    }
    assert!(valid_identifier("while"));
    assert!(!valid_identifier("\u{53d8}\u{91cf}"));

    let uri = "file:///memory/while.tl";
    let text = "fn while() {} fn main() { while(); }";
    let mut state = ServerState::default();
    state.open(&json!({ "textDocument": { "uri": uri, "text": text } }));
    let completion = state.completion(&json!({
        "textDocument": { "uri": uri },
        "position": { "line": 0, "character": text.len() }
    }));
    let item = completion
        .as_array()
        .expect("completion result is an array")
        .iter()
        .find(|item| item["label"] == "while")
        .expect("the visible function should be offered");
    assert_eq!(item["kind"], 3);
}

#[test]
fn const_symbols_keywords_and_expanded_methods_have_lsp_kinds_and_scope() {
    let uri = "file:///memory/const-completion.tl";
    let text = concat!(
        "const VALUE = 1;\n",
        "fn main() {\n",
        "  const LOCAL = [1];\n",
        "  print(LOCAL);\n",
        "}\n",
    );
    let mut state = ServerState::default();
    state.initialize(&json!({}));
    state.open(&json!({ "textDocument": { "uri": uri, "text": text } }));

    let top_level = state.completion(&json!({
        "textDocument": { "uri": uri },
        "position": { "line": 0, "character": 0 }
    }));
    assert!(
        top_level
            .as_array()
            .expect("completion result is an array")
            .iter()
            .any(|item| item["label"] == "const" && item["kind"] == 14),
        "const keyword completion missing: {top_level}"
    );
    assert!(
        top_level
            .as_array()
            .expect("completion result is an array")
            .iter()
            .all(|item| item["label"] != "LOCAL"),
        "block const must not be visible outside main: {top_level}"
    );

    for (index, receiver_text, labels) in [
        (
            0,
            "fn main() { \"text\".; }",
            &["char_len", "chars", "replace_once"][..],
        ),
        (
            1,
            "fn main() { let values = [1]; values.; }",
            &["any", "all", "find_index", "fold"][..],
        ),
        (
            2,
            "fn main() { let bytes: int[] = [65]; bytes.; }",
            &["to_utf8_string"][..],
        ),
    ] {
        let method_uri = format!("file:///memory/const-method-{index}.tl");
        state.open(&json!({
            "textDocument": { "uri": method_uri, "text": receiver_text }
        }));
        let completion = state.completion(&json!({
            "textDocument": { "uri": method_uri },
            "position": { "line": 0, "character": receiver_text.rfind('.').unwrap() + 1 }
        }));
        let items = completion
            .as_array()
            .expect("method completion is an array");
        for label in labels {
            assert!(
                items.iter().any(|item| item["label"] == *label),
                "missing {label} for {receiver_text}: {completion}"
            );
        }
    }

    let symbols = state.document_symbols(&json!({ "textDocument": { "uri": uri } }));
    let symbols = symbols.as_array().expect("document symbols are an array");
    let value = symbols
        .iter()
        .find(|symbol| symbol["name"] == "VALUE")
        .expect("module const should be a document symbol");
    assert_eq!(value["kind"], 14);
    let local = symbols
        .iter()
        .find(|symbol| symbol["name"] == "LOCAL")
        .expect("block const should be represented in document symbols");
    assert_eq!(local["kind"], 14);

    let inside = state.completion(&json!({
        "textDocument": { "uri": uri },
        "position": { "line": 3, "character": 2 }
    }));
    assert!(
        inside
            .as_array()
            .expect("scoped completion is an array")
            .iter()
            .any(|item| item["label"] == "LOCAL" && item["kind"] == 21),
        "block const should be visible inside its function: {inside}"
    );
}

#[test]
fn document_symbols_use_declaration_locations_for_utf16_and_parameter_scopes() {
    let uri = "file:///memory/%E4%B8%AD%E6%96%87.tl";
    let text = "fn helper(value: int) -> int { return value; }\nfn main() { helper(1); }";
    let mut state = ServerState::default();
    state.open(&json!({ "textDocument": { "uri": uri, "text": text } }));
    let symbols = state.document_symbols(&json!({ "textDocument": { "uri": uri } }));
    let helper = symbols
        .as_array()
        .expect("document symbols are a flat SymbolInformation array")
        .iter()
        .find(|symbol| symbol["name"] == "helper")
        .expect("function declaration is indexed");
    assert_eq!(helper["location"]["uri"], uri);
    assert_eq!(
        helper["location"]["range"]["start"],
        json!({ "line": 0, "character": 3 })
    );
    assert_eq!(
        helper["location"]["range"]["end"],
        json!({ "line": 0, "character": 9 })
    );
}

#[test]
fn formatting_uses_the_open_buffer_and_refuses_incomplete_syntax() {
    let uri = "file:///memory/format.tl";
    let compact = "fn main(){let answer=42;}";
    let mut state = ServerState::default();
    state.open(&json!({ "textDocument": { "uri": uri, "text": compact } }));

    let edits = state.formatting(&json!({ "textDocument": { "uri": uri }, "options": {
        "tabSize": 2, "insertSpaces": false
    } }));
    assert_eq!(edits.as_array().map(Vec::len), Some(1));
    assert_eq!(
        edits[0]["range"]["start"],
        json!({ "line": 0, "character": 0 })
    );
    assert_eq!(
        edits[0]["range"]["end"],
        json!({ "line": 0, "character": compact.len() })
    );
    assert_eq!(
        edits[0]["newText"],
        "fn main() {\n    let answer = 42;\n}\n"
    );

    state.change(&json!({
        "textDocument": { "uri": uri },
        "contentChanges": [{ "text": "fn main() {" }]
    }));
    assert_eq!(
        state.formatting(&json!({ "textDocument": { "uri": uri }, "options": {} })),
        json!([])
    );
}

#[test]
fn imported_compile_errors_target_the_imported_document() {
    let dependency_uri = "file:///memory/dep.tl";
    let root_uri = "file:///memory/root.tl";
    let dependency = "pub fn broken() -> int { return missing; }";
    let mut state = ServerState::default();
    state.open(&json!({ "textDocument": { "uri": dependency_uri, "text": dependency } }));
    state.open(&json!({ "textDocument": {
        "uri": root_uri,
        "text": "import \"dep.tl\" as dep;  fn main() {}"
    } }));

    let report = state.diagnostic_report(root_uri);
    let (target, diagnostics) = report
        .publications
        .iter()
        .find(|(uri, _)| uri == dependency_uri)
        .expect("the imported source should own the diagnostic");
    assert_eq!(target, dependency_uri);
    assert_eq!(diagnostics.len(), 1);
    assert!(
        diagnostics[0]["message"]
            .as_str()
            .is_some_and(|message| message.contains("missing"))
    );
    assert_eq!(diagnostics[0]["range"]["start"]["character"], 32);
}

#[test]
fn diagnostic_tracker_clears_imported_errors_that_a_root_no_longer_produces() {
    let dependency_uri = "file:///memory/dep.tl";
    let root_uri = "file:///memory/root.tl";
    let mut state = ServerState::default();
    state.open(&json!({ "textDocument": {
        "uri": dependency_uri,
        "text": "pub fn broken() -> int { return missing; }"
    } }));
    state.open(&json!({ "textDocument": {
        "uri": root_uri,
        "text": "import \"dep.tl\" as dep;  fn main() {}"
    } }));
    let mut tracker = DiagnosticTracker::default();

    let first = tracker.update(root_uri, state.diagnostic_report(root_uri));
    assert_eq!(
        first
            .publications
            .iter()
            .find(|(uri, _)| uri == dependency_uri)
            .map(|(_, diagnostics)| diagnostics.len()),
        Some(1)
    );

    state.change(&json!({
        "textDocument": { "uri": root_uri },
        "contentChanges": [{ "text": " fn main() {}" }]
    }));
    let second = tracker.update(root_uri, state.diagnostic_report(root_uri));
    assert_eq!(
        second
            .publications
            .iter()
            .find(|(uri, _)| uri == dependency_uri)
            .map(|(_, diagnostics)| diagnostics.len()),
        Some(0)
    );
}

#[cfg(windows)]
#[test]
fn scanned_windows_file_and_open_buffer_share_one_standard_uri() {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!("teal-lsp-uri-{nonce}-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("main.tl");
    std::fs::write(&path, "pub fn disk_symbol() {}").unwrap();
    let root_uri = path_to_uri(&root);
    let file_uri = path_to_uri(&path);
    assert!(root_uri.starts_with("file:///"));
    assert!(!root_uri.contains("/?/"));

    let mut state = ServerState::default();
    state.initialize(&json!({ "rootUri": root_uri }));
    assert_eq!(state.documents.len(), 1);
    assert!(state.documents.contains_key(&file_uri));

    let client_uri = file_uri.to_lowercase();
    assert_eq!(
        state.open(&json!({ "textDocument": {
        "uri": client_uri,
        "text": "pub fn overlay_symbol() {}"
    } })),
        Some(file_uri.clone())
    );
    assert_eq!(state.documents.len(), 1);
    assert_eq!(
        state.workspace_symbols(&json!({ "query": "disk_symbol" })),
        json!([])
    );
    assert_eq!(
        state
            .workspace_symbols(&json!({ "query": "overlay_symbol" }))
            .as_array()
            .map(Vec::len),
        Some(1)
    );

    assert_eq!(state.close(&client_uri), Some((file_uri, true)));
    assert_eq!(
        state
            .workspace_symbols(&json!({ "query": "disk_symbol" }))
            .as_array()
            .map(Vec::len),
        Some(1)
    );

    std::fs::remove_file(path).unwrap();
    std::fs::remove_dir(root).unwrap();
}

#[test]
fn successful_lints_publish_warning_severity_and_canonical_code() {
    let uri = "file:///memory/lint.tl";
    let mut state = ServerState::default();
    state.open(&json!({ "textDocument": {
        "uri": uri,
        "text": " fn main() { let unused = 1; }"
    } }));
    let diagnostics = state.diagnostics(uri);
    let warning = diagnostics
        .iter()
        .find(|diagnostic| diagnostic["code"] == "unused-variable")
        .expect("unused variable warning should be published");
    assert_eq!(warning["severity"], 2);
}

#[test]
fn imported_configuration_warnings_target_the_imported_uri() {
    let dependency_uri = "file:///memory/config.tl";
    let root_uri = "file:///memory/config-root.tl";
    let mut state = ServerState::default();
    state.open(&json!({ "textDocument": {
        "uri": dependency_uri,
        "text": "@optimization(\"none\"); pub fn helper() {}"
    } }));
    state.open(&json!({ "textDocument": {
        "uri": root_uri,
        "text": "import \"config.tl\" as config;  fn main() { config::helper(); }"
    } }));
    let report = state.diagnostic_report(root_uri);
    let (_, diagnostics) = report
        .publications
        .iter()
        .find(|(uri, _)| uri == dependency_uri)
        .expect("imported warning should target the imported document");
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic["code"] == "ignored-compiler-option")
    );
}

#[test]
fn lsp_accepts_the_explicit_standard_filesystem_import() {
    let uri = "file:///memory/extensions.tl";
    let mut state = ServerState::default();
    state.open(&json!({ "textDocument": {
        "uri": uri,
        "text": "import \"std/fs.tl\" as fs; task main() { fs::exists(\"x\").await; }"
    } }));
    assert!(
        state
            .diagnostics(uri)
            .iter()
            .all(|diagnostic| diagnostic["severity"] != 1)
    );
}

#[test]
fn lsp_rejects_the_removed_implicit_filesystem_namespace() {
    let uri = "file:///memory/implicit-fs.tl";
    let mut state = ServerState::default();
    state.open(&json!({ "textDocument": {
        "uri": uri,
        "text": "fn main() { fs::exists(\"x\"); }"
    } }));
    assert!(state.diagnostics(uri).iter().any(|diagnostic| {
        diagnostic["severity"] == 1
            && diagnostic["message"]
                .as_str()
                .is_some_and(|message| message.contains("fs"))
    }));
}

fn definition_at(state: &ServerState, uri: &str, text: &str, needle: &str) -> serde_json::Value {
    let character = text
        .find(needle)
        .unwrap_or_else(|| panic!("missing fixture token {needle}"))
        + needle.len()
        - 1;
    state.definition(&json!({
        "textDocument": { "uri": uri },
        "position": { "line": 0, "character": character }
    }))
}

#[test]
fn resolved_re_export_navigation_targets_the_original_source() {
    let leaf_uri = "file:///memory/leaf.tl";
    let facade_uri = "file:///memory/facade.tl";
    let root_uri = "file:///memory/root.tl";
    let leaf = "pub fn answer() -> int { return 7; }";
    let facade = "import \"leaf.tl\" as leaf; pub leaf::answer;";
    let root = "import \"facade.tl\" as facade; fn main() { print(facade::answer()); }";
    let mut state = ServerState::default();
    state.open(&json!({ "textDocument": { "uri": leaf_uri, "text": leaf } }));
    state.open(&json!({ "textDocument": { "uri": facade_uri, "text": facade } }));
    state.open(&json!({ "textDocument": { "uri": root_uri, "text": root } }));

    assert!(state.diagnostics(root_uri).is_empty());
    let leaf_document = state
        .documents
        .values()
        .find(|document| document.path.ends_with("leaf.tl"))
        .expect("leaf document");
    let normalized_leaf_uri = path_to_uri(&leaf_document.path);
    let root_document = state
        .documents
        .values()
        .find(|document| document.path.ends_with("root.tl"))
        .expect("root document");
    let normalized_root_uri = path_to_uri(&root_document.path);
    let target = definition_at(&state, root_uri, root, "answer");
    assert_eq!(target["uri"], normalized_leaf_uri);
    assert_eq!(
        target["range"]["start"]["character"],
        leaf.find("answer").unwrap()
    );
    let character = root.find("answer").unwrap() + 1;
    let references = state.references(
        &json!({
            "textDocument": { "uri": root_uri },
            "position": { "line": 0, "character": character }
        }),
        true,
    );
    let locations = references.as_array().expect("reference locations");
    assert_eq!(locations.len(), 2, "{references}");
    assert!(
        locations
            .iter()
            .any(|location| location["uri"] == normalized_leaf_uri)
    );
    assert!(
        locations
            .iter()
            .any(|location| location["uri"] == normalized_root_uri)
    );
}

#[test]
fn valid_changes_invalidate_resolved_snapshots_and_incomplete_text_reuses_the_latest_one() {
    let uri = "file:///memory/snapshot.tl";
    let original = "fn helper() {} fn main() { helper(); }";
    let renamed = "fn renamed() {} fn main() { renamed(); }";
    let incomplete = "fn renamed() {} fn main() { renamed(; }";
    let mut state = ServerState::default();
    state.open(&json!({ "textDocument": { "uri": uri, "text": original } }));
    assert!(state.diagnostics(uri).is_empty());
    assert_eq!(
        definition_at(&state, uri, original, "helper")["range"]["start"]["character"],
        original.find("helper").unwrap()
    );

    state.change(&json!({
        "textDocument": { "uri": uri },
        "contentChanges": [{ "text": renamed }]
    }));
    assert_eq!(
        definition_at(&state, uri, renamed, "renamed")["range"]["start"]["character"],
        renamed.find("renamed").unwrap()
    );
    assert!(state.diagnostics(uri).is_empty());

    state.change(&json!({
        "textDocument": { "uri": uri },
        "contentChanges": [{ "text": incomplete }]
    }));
    assert_eq!(
        definition_at(&state, uri, incomplete, "renamed")["range"]["start"]["character"],
        renamed.find("renamed").unwrap()
    );
}

#[test]
fn invalid_later_content_change_leaves_text_and_analysis_unchanged() {
    let uri = "file:///memory/transactional-change.tl";
    let original = "fn helper() {} fn main() { helper(); }";
    let mut state = ServerState::default();
    state.open(&json!({ "textDocument": { "uri": uri, "text": original } }));
    let definition = definition_at(&state, uri, original, "helper");

    assert!(
        state
            .change(&json!({
                "textDocument": { "uri": uri },
                "contentChanges": [
                    {
                        "range": {
                            "start": { "line": 0, "character": 3 },
                            "end": { "line": 0, "character": 9 }
                        },
                        "text": "changed"
                    },
                    {
                        "range": {
                            "start": { "line": 99, "character": 0 },
                            "end": { "line": 99, "character": 1 }
                        },
                        "text": "invalid"
                    }
                ]
            }))
            .is_none()
    );
    let document = state.documents.get(uri).expect("open document");
    assert_eq!(document.text.as_ref(), original);
    assert_eq!(definition_at(&state, uri, original, "helper"), definition);
}

#[test]
fn signature_help_tracks_the_innermost_unclosed_call_only() {
    let uri = "file:///memory/signature-nesting.tl";
    let text = "fn inner(first: int, second: int) -> int { return first + second; } fn outer(first: int, second: int) {} fn main() { outer(inner(1, 2), 3); }";
    let mut state = ServerState::default();
    state.open(&json!({ "textDocument": { "uri": uri, "text": text } }));

    let outer_second = text.find("), 3").unwrap() + 4;
    let help = state.signature_help(&json!({
        "textDocument": { "uri": uri },
        "position": { "line": 0, "character": outer_second }
    }));
    assert!(
        help["signatures"][0]["label"]
            .as_str()
            .is_some_and(|label| label.contains("outer")),
        "{help}"
    );
    assert_eq!(help["activeParameter"], 1);

    let after_closed = text.rfind(";").unwrap() + 1;
    assert_eq!(
        state.signature_help(&json!({
            "textDocument": { "uri": uri },
            "position": { "line": 0, "character": after_closed }
        })),
        serde_json::Value::Null
    );
}

#[test]
fn rename_refuses_stale_occurrence_coordinates() {
    let uri = "file:///memory/stale-rename.tl";
    let original = "fn main() { let target = 1; print(target); }";
    let incomplete = "fn main() { let target = 1; unexpected( print(target); }";
    let mut state = ServerState::default();
    state.open(&json!({ "textDocument": { "uri": uri, "text": original } }));
    assert!(state.diagnostics(uri).is_empty());

    state.change(&json!({
        "textDocument": { "uri": uri },
        "contentChanges": [{ "text": incomplete }]
    }));
    let declaration = incomplete.find("target").unwrap() + 1;
    let rename = state.rename(&json!({
        "textDocument": { "uri": uri },
        "position": { "line": 0, "character": declaration },
        "newName": "renamed"
    }));
    assert_eq!(rename, serde_json::Value::Null);

    assert_eq!(
        definition_at(&state, uri, incomplete, "target")["range"]["start"]["character"],
        original.find("target").unwrap(),
        "read-only navigation may still use the explicitly retained snapshot"
    );
}

#[test]
fn changing_imported_document_rechecks_all_dependent_roots() {
    let dependency_uri = "file:///memory/shared-dep.tl";
    let first_root = "file:///memory/first-root.tl";
    let second_root = "file:///memory/second-root.tl";
    let mut state = ServerState::default();
    state.open(&json!({ "textDocument": {
        "uri": dependency_uri,
        "text": "pub fn broken() -> int { return missing; }"
    }}));
    for root in [first_root, second_root] {
        state.open(&json!({ "textDocument": {
            "uri": root,
            "text": "import \"shared-dep.tl\" as dep; fn main() { print(dep::broken()); }"
        }}));
    }
    let mut tracker = DiagnosticTracker::default();
    for root in [first_root, second_root] {
        state.refresh_diagnostics(root, &mut tracker);
    }
    assert_eq!(
        tracker
            .aggregate(std::collections::BTreeSet::from(
                [dependency_uri.to_owned()]
            ))
            .publications[0]
            .1
            .len(),
        1,
        "shared diagnostics are deduplicated before publication"
    );

    state.change(&json!({
        "textDocument": { "uri": dependency_uri },
        "contentChanges": [{ "text": "pub fn broken() -> int { return 1; }" }]
    }));
    let refreshed = state.refresh_diagnostics(dependency_uri, &mut tracker);
    assert_eq!(
        refreshed
            .publications
            .iter()
            .find(|(uri, _)| uri == dependency_uri)
            .map(|(_, diagnostics)| diagnostics.len()),
        Some(0)
    );
}

#[test]
fn opening_and_closing_a_missing_import_invalidates_its_root() {
    let dependency_uri = "file:///memory/late-dep.tl";
    let root_uri = "file:///memory/late-root.tl";
    let mut state = ServerState::default();
    state.open(&json!({ "textDocument": {
        "uri": root_uri,
        "text": "import \"late-dep.tl\" as dep; fn main() { print(dep::answer()); }"
    }}));
    let mut tracker = DiagnosticTracker::default();
    let missing = state.refresh_diagnostics(root_uri, &mut tracker);
    assert!(
        missing
            .publications
            .iter()
            .flat_map(|(_, diagnostics)| diagnostics)
            .any(|diagnostic| diagnostic["message"]
                .as_str()
                .is_some_and(|message| message.contains("cannot read imported source")))
    );

    state.open(&json!({ "textDocument": {
        "uri": dependency_uri,
        "text": "pub fn answer() -> int { return 42; }"
    }}));
    let available = state.refresh_diagnostics(dependency_uri, &mut tracker);
    assert!(available.publications.iter().all(|(_, diagnostics)| {
        diagnostics
            .iter()
            .all(|diagnostic| diagnostic["severity"] != 1)
    }));

    let (closed, restored) = state.close(dependency_uri).unwrap();
    assert!(!restored);
    let missing_again = state.refresh_diagnostics(&closed, &mut tracker);
    assert!(
        missing_again
            .publications
            .iter()
            .flat_map(|(_, diagnostics)| diagnostics)
            .any(|diagnostic| diagnostic["message"]
                .as_str()
                .is_some_and(|message| message.contains("cannot read imported source")))
    );
}
