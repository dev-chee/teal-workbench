//! LSP resolved analysis 快照共享的可重复性能基线。

use std::{hint::black_box, path::PathBuf, sync::Arc, time::Instant};

use serde_json::json;
use teal_compiler::{CompileOptions, Source, compile_with_analysis};
use teal_test_support::{ScopedAllocator, measure};

use super::{ServerState, path_to_uri};

#[global_allocator]
static ALLOCATOR: ScopedAllocator = ScopedAllocator;

const MODULES: usize = 64;
const ROUNDS: usize = 7;
const WARMUP_ROUNDS: usize = 2;

fn state_and_analysis() -> (ServerState, Arc<teal_compiler::CompilationAnalysis>) {
    let root = std::env::current_dir()
        .expect("benchmark working directory")
        .join(PathBuf::from("target/refactor-lsp"));
    let mut state = ServerState::default();
    for index in 0..MODULES {
        let path = root.join(format!("module_{index}.tl"));
        let uri = path_to_uri(&path);
        state.open(&json!({"textDocument": {
            "uri": uri,
            "text": format!("pub fn value_{index}() -> int {{ return {index}; }}")
        }}));
    }

    let root_path = root.join("main.tl");
    let root_uri = path_to_uri(&root_path);
    let mut root_text = String::new();
    for index in 0..MODULES {
        root_text.push_str(&format!(
            "import \"module_{index}.tl\" as module_{index};\n"
        ));
    }
    root_text.push_str("fn main() { print(");
    for index in 0..MODULES {
        if index != 0 {
            root_text.push_str(", ");
        }
        root_text.push_str(&format!("module_{index}::value_{index}()"));
    }
    root_text.push_str("); }\n");
    state.open(&json!({"textDocument": {
        "uri": root_uri,
        "text": root_text
    }}));

    let loader = state.loader();
    let analysis = Arc::new(
        compile_with_analysis(
            Source::new(root_path.to_string_lossy(), root_text),
            &loader,
            CompileOptions::default(),
        )
        .expect("multi-document benchmark should compile")
        .analysis,
    );
    assert_eq!(analysis.sources().len(), MODULES + 1);
    (state, analysis)
}

#[test]
#[ignore = "manual release benchmark"]
fn install_shared_compilation_snapshot_baseline() {
    let (mut state, analysis) = state_and_analysis();
    for _ in 0..WARMUP_ROUNDS {
        state.apply_compilation_analysis(Arc::clone(&analysis));
    }

    let mut samples = Vec::with_capacity(ROUNDS);
    for _ in 0..ROUNDS {
        let started = Instant::now();
        state.apply_compilation_analysis(Arc::clone(&analysis));
        samples.push(started.elapsed());
        black_box(&state);
    }
    samples.sort_unstable();
    println!(
        "BENCH|lsp-shared-analysis|median_ns={}|min_ns={}|max_ns={}|rounds={ROUNDS}|documents={}|sources={}",
        samples[samples.len() / 2].as_nanos(),
        samples[0].as_nanos(),
        samples[samples.len() - 1].as_nanos(),
        state.documents.len(),
        analysis.sources().len(),
    );
    let (_, allocations) = measure(|| {
        state.apply_compilation_analysis(Arc::clone(&analysis));
    });
    println!(
        "ALLOC|lsp-shared-analysis|allocations={}|allocated_bytes={}|live_bytes={}|peak_live_bytes={}",
        allocations.allocations,
        allocations.allocated_bytes,
        allocations.live_bytes,
        allocations.peak_live_bytes,
    );
}

#[test]
fn every_resolved_document_shares_one_compilation_snapshot() {
    let (mut state, analysis) = state_and_analysis();
    state.apply_compilation_analysis(Arc::clone(&analysis));
    assert_eq!(state.documents.len(), analysis.sources().len());
    for document in state.documents.values() {
        let (_, _, installed) = document
            .resolved_compilation
            .as_ref()
            .expect("matching documents should receive resolved analysis");
        assert!(Arc::ptr_eq(installed, &analysis));
    }
}
