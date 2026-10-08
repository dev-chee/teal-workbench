use std::path::Path;
pub fn config(root: &Path) -> teal_lsp_client::ServerConfig {
    let program = root.join(if cfg!(windows) {
        "mock-lsp.exe"
    } else {
        "mock-lsp"
    });
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/support/mock_lsp.rs");
    let source = if source.exists() {
        source
    } else {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../teal-bench/tests/support/mock_lsp.rs")
    };
    let output = std::process::Command::new("rustc")
        .args(["--edition=2024"])
        .arg(source)
        .arg("-o")
        .arg(&program)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    teal_lsp_client::ServerConfig {
        program: program.to_string_lossy().into_owned(),
        args: vec![],
        root: root.to_owned(),
        initialization_options: serde_json::Value::Null,
        environment: std::collections::HashMap::from([(
            "TEAL_BENCH_MOCK_LOG".into(),
            root.join("lsp.jsonl").to_string_lossy().into_owned(),
        )]),
    }
}
pub fn log(root: &Path) -> Vec<serde_json::Value> {
    std::fs::read_to_string(root.join("lsp.jsonl"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}
