//! 异步文件打开与加载结果安装。
//!
//! 工作线程只读取并构造拥有所有权的数据；面板、缓冲区与语言服务由结果处理器接入。
//! 文件打开与资源修改的协调边界见 `teal-bench` README；读取不自动受修改保留状态保护。

use super::{
    App, AppPanel, SplitAxis, EditorPanel, ExplorerPanel, Path, PathBuf, RuntimeEvent, Severity,
};

pub(crate) enum LoadedPath {
    Document(teal_workbench_core::Document),
    Explorer(teal_workbench_services::FileExplorer),
}

impl App {
    pub(super) fn open_path(&mut self, path: PathBuf) {
        self.queue_open(path, false, None);
    }

    pub(super) fn queue_open(
        &mut self,
        path: PathBuf,
        address: bool,
        position: Option<serde_json::Value>,
    ) {
        let path = if path.is_absolute() {
            path
        } else {
            self.workspace.join(path)
        };
        let key = path_key(&path);
        let generation = *self.path_generations.entry(key.clone()).or_default();

        let output = self.runtime_tx.clone();
        let observation = crate::runtime_bridge::Context::with_span(
            tracing::info_span!("teal.bench.file.open", operation.id = teal_workbench_services::observability::operation_id(), file.path = %path.display(), operation.result = tracing::field::Empty, otel.status_code = tracing::field::Empty),
        );

        // 文件系统加载返回拥有所有权的数据；只由结果处理器安装面板。
        self.tasks.spawn(observation.instrument(async move {
            let request_path = path.clone();
            let result = crate::runtime_bridge::spawn_blocking(move || {
                if path.is_dir() {
                    teal_workbench_services::FileExplorer::open(&path)
                        .map(LoadedPath::Explorer)
                        .map_err(|e| e.to_string())
                } else {
                    path.canonicalize()
                        .map_err(|e| e.to_string())
                        .and_then(|path| {
                            teal_workbench_core::Document::open(&path)
                                .map(LoadedPath::Document)
                                .map_err(|e| e.to_string())
                        })
                }
            })
            .await
            .map_err(|e| e.to_string())
            .and_then(std::convert::identity);
            teal_workbench_services::observability::outcome(&result);
            let _ = output
                .send(RuntimeEvent::PathLoaded {
                    result,
                    path: path_key(&request_path),
                    generation,
                    address,
                    position,
                })
                .await;
        }));
    }

    pub(super) async fn finish_open(
        &mut self,
        result: Result<LoadedPath, String>,
        path: PathBuf,
        generation: u64,
        address: bool,
        position: Option<serde_json::Value>,
    ) {
        if self
            .path_generations
            .get(&path)
            .copied()
            .unwrap_or_default()
            != generation
            || self.mutation_path_busy(&path)
        {
            if !self
                .deferred_opens
                .iter()
                .any(|(queued, _, _)| queued == &path)
            {
                self.deferred_opens.push((path, address, position));
            }
            return;
        }
        match result {
            Err(error) => self.message(Severity::Error, "open", error),
            Ok(LoadedPath::Explorer(model)) => {
                self.add_panel(
                    ExplorerPanel::from_model(model, address),
                    SplitAxis::Columns,
                );
            }
            Ok(LoadedPath::Document(document)) => {
                let path = document.path().map(Path::to_owned);
                let mut panel = EditorPanel::new(document, self.keymap);
                if let Some(extension) = path
                    .as_ref()
                    .and_then(|p| p.extension())
                    .and_then(|e| e.to_str())
                    .map(str::to_ascii_lowercase)
                    && let Some(grammar) = self.grammars.get(&extension)
                    && let Some(table) = self.grammar_tables.get(&extension)
                {
                    let languages = self
                        .grammars
                        .iter()
                        .filter_map(|(key, grammar)| {
                            self.grammar_tables
                                .get(key)
                                .map(|table| (grammar.clone(), table.clone()))
                        })
                        .collect();
                    let _ = panel.set_grammar_environment(grammar, table.clone(), languages);
                }

                let id = self.add_panel(panel, SplitAxis::Columns);
                self.recover_editor(id);
                if let Some(position) = position
                    && let Some(editor) = self.panels.get(id).and_then(AppPanel::as_editor)
                {
                    let index = teal_lsp_client::LineIndex::new(editor.document().to_string());
                    if let (Some(line), Some(column)) = (
                        position["line"]
                            .as_u64()
                            .and_then(|v| u32::try_from(v).ok()),
                        position["character"]
                            .as_u64()
                            .and_then(|v| u32::try_from(v).ok()),
                    ) && let Some(offset) = index
                        .offset(u64::from(line), u64::from(column))
                        .and_then(|byte| index.char_offset(byte))
                    {
                        editor.document().set_selections(
                            teal_workbench_core::SelectionSet::single(
                                teal_workbench_core::Selection::cursor(offset),
                            ),
                        );
                    }
                }

                self.fire_hook(
                    "buffer.opened",
                    serde_json::json!({"path":path, "panel":format!("{id:?}")}),
                );
                self.notify_lsp_open(id).await;
            }
        }
    }

    pub(super) fn resume_deferred_opens(&mut self) {
        if !self.io.is_idle() {
            return;
        }
        for (path, address, position) in std::mem::take(&mut self.deferred_opens) {
            self.queue_open(path, address, position);
        }
    }
}

pub(super) fn path_key(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_owned())
}
