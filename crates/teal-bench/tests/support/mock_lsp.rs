use std::{
    fs::OpenOptions,
    io::{self, BufRead, Read, Write},
};
fn send(payload: &str) {
    let mut out = io::stdout().lock();
    write!(out, "Content-Length: {}\r\n\r\n{}", payload.len(), payload).unwrap();
    out.flush().unwrap();
}
fn string_field<'a>(json: &'a str, name: &str) -> Option<&'a str> {
    let marker = format!("\"{name}\":\"");
    let value = json.split_once(&marker)?.1;
    value.split_once('"').map(|(value, _)| value)
}
fn main() {
    let mut input = io::stdin().lock();
    let log_path = std::env::var("TEAL_BENCH_MOCK_LOG").unwrap();
    let mut log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)
        .unwrap();
    loop {
        let mut length = 0;
        loop {
            let mut line = String::new();
            if input.read_line(&mut line).unwrap() == 0 {
                return;
            }
            if line == "\r\n" {
                break;
            }
            if let Some(n) = line.trim().strip_prefix("Content-Length: ") {
                length = n.parse::<usize>().unwrap();
            }
        }
        let mut bytes = vec![0; length];
        input.read_exact(&mut bytes).unwrap();
        let data = String::from_utf8(bytes).unwrap();
        writeln!(log, "{data}").unwrap();
        log.flush().unwrap();
        if data.contains("\"method\":\"exit\"") {
            break;
        }
        let Some(after) = data.split("\"id\":").nth(1) else {
            continue;
        };
        let id = after
            .chars()
            .take_while(char::is_ascii_digit)
            .collect::<String>();
        if id.is_empty() || data.contains("$/cancelRequest") {
            continue;
        }
        if data.contains("\"method\":\"mock/slow\"") {
            continue;
        }
        let result = if data.contains("\"method\":\"initialize\"") {
            r#"{"capabilities":{"textDocumentSync":2,"completionProvider":{},"hoverProvider":true,"definitionProvider":true,"referencesProvider":true,"renameProvider":true,"documentFormattingProvider":true,"codeActionProvider":true,"semanticTokensProvider":{"full":{"delta":true},"legend":{"tokenTypes":["keyword"],"tokenModifiers":[]}}}}"#.to_owned()
        } else if data.contains("textDocument/completion") {
            r#"[{"label":"call","insertText":"call(${1:value})$0","insertTextFormat":2}]"#
                .to_owned()
        } else if data.contains("textDocument/hover") {
            "{\"contents\":{\"kind\":\"markdown\",\"value\":\"**hover \u{4e16}\u{754c}**\"}}"
                .to_owned()
        } else if data.contains("semanticTokens/full/delta") && data.contains("invalid-semantic.rs")
        {
            r#"{"resultId":"invalid","edits":[{"start":0,"deleteCount":0,"data":[0,0,1,0]}]}"#
                .to_owned()
        } else if data.contains("semanticTokens/full/delta") {
            r#"{"resultId":"second","edits":[{"start":0,"deleteCount":5,"data":[0,0,2,0,0]}]}"#
                .to_owned()
        } else if data.contains("semanticTokens/full") {
            r#"{"resultId":"first","data":[0,0,1,0,0]}"#.to_owned()
        } else if data.contains("textDocument/formatting") {
            r#"[{"range":{"start":{"line":0,"character":0},"end":{"line":0,"character":0}},"newText":"// formatted\n"}]"#.to_owned()
        } else if data.contains("textDocument/codeAction") && data.contains("code-action.rs") {
            let uri = string_field(&data, "uri").unwrap();
            format!(
                r#"[{{"title":"Edit then execute","edit":{{"changes":{{"{uri}":[{{"range":{{"start":{{"line":0,"character":0}},"end":{{"line":0,"character":0}}}},"newText":"edited "}}]}}}},"command":{{"command":"mock.afterEdit","arguments":[]}}}}]"#
            )
        } else if data.contains("textDocument/codeAction") {
            r#"[{"title":"Explain","command":"mock.explain","arguments":[]}]"#.to_owned()
        } else {
            "null".to_owned()
        };
        send(&format!(
            r#"{{"jsonrpc":"2.0","id":{id},"result":{result}}}"#
        ));
    }
}
