//! 文档快照、覆盖源码加载与文档生命周期。

use super::*;

#[derive(Clone, Debug)]
pub(super) struct Document {
    pub(super) uri: String,
    pub(super) path: PathBuf,
    pub(super) text: Arc<str>,
    pub(super) index: LineIndex,
    pub(super) current: SemanticAnalysis,
    pub(super) resolved_compilation:
        Option<(Arc<str>, teal_compiler::SourceId, Arc<CompilationAnalysis>)>,
    pub(super) syntax_valid: bool,
    /// 依赖快照改变后，当前文本相同也不能继续把旧编译当作当前分析。
    pub(super) dependencies_valid: bool,
}

impl Document {
    pub(super) fn new(uri: String, path: PathBuf, text: String) -> Self {
        let text: Arc<str> = text.into();
        let source = Source::new(path.to_string_lossy(), text.to_string());
        let current = analyze_source(&source);
        let syntax_valid = parse(&source).is_ok();
        Self {
            uri,
            path,
            index: LineIndex::from_shared(Arc::clone(&text)),
            text,
            current,
            resolved_compilation: None,
            syntax_valid,
            dependencies_valid: true,
        }
    }

    pub(super) fn replace(&mut self, text: impl Into<Arc<str>>) {
        self.text = text.into();
        self.index = LineIndex::from_shared(Arc::clone(&self.text));
        let source = Source::new(self.path.to_string_lossy(), self.text.to_string());
        self.current = analyze_source(&source);
        self.syntax_valid = parse(&source).is_ok();
    }

    pub(super) fn analysis(&self) -> &SemanticAnalysis {
        self.compilation()
            .and_then(|(source, analysis)| analysis.source(source))
            .unwrap_or(&self.current)
    }

    pub(super) fn compilation(&self) -> Option<(teal_compiler::SourceId, &CompilationAnalysis)> {
        let (text, source, analysis) = self.resolved_compilation.as_ref()?;
        ((text == &self.text && self.dependencies_valid) || !self.syntax_valid)
            .then_some((*source, analysis))
    }

    /// 写操作只能消费当前有效分析；只读导航的历史回退不构成编辑坐标证明。
    pub(super) fn edit_analysis(&self) -> Option<&SemanticAnalysis> {
        (self.syntax_valid && self.dependencies_valid).then(|| self.analysis())
    }
}

#[derive(Clone)]
pub(super) struct OverlayLoader {
    pub(super) files: HashMap<PathBuf, Arc<str>>,
    pub(super) reads: std::cell::RefCell<BTreeSet<PathBuf>>,
}

impl SourceLoader for OverlayLoader {
    fn load(&self, importer: &Path, import: &str) -> Result<LoadedSource, SourceLoadError> {
        if let Some(source) = package_catalog()?.load_package(importer, import)? {
            return Ok(source);
        }
        let unresolved = importer
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(import);
        let path = unresolved.canonicalize().unwrap_or(unresolved);
        // 在实际读取之前记录，缺失文件和编译失败同样必须参与后续失效。
        self.reads.borrow_mut().insert(normalize_path(path.clone()));
        let contents = self
            .files
            .get(&path)
            .cloned()
            .or_else(|| std::fs::read_to_string(&path).ok().map(Arc::from))
            .ok_or_else(|| SourceLoadError {
                message: format!("cannot read imported source {}", path.display()),
            })?;
        Ok(LoadedSource {
            identity: path.clone(),
            source: Source::new(path.to_string_lossy(), contents.to_string()),
            precompiled: None,
        })
    }
}

#[derive(Default)]
pub(super) struct ServerState {
    pub(super) documents: HashMap<String, Document>,
    pub(super) roots: Vec<PathBuf>,
    pub(super) entry_files: Vec<PathBuf>,
    pub(super) shutdown: bool,
    pub(super) builtin_uri: Option<String>,
    pub(super) builtin_catalog_ready: bool,
    pub(super) dependencies: HashMap<String, BTreeSet<PathBuf>>,
}

impl ServerState {
    pub(super) fn initialize(&mut self, params: &Value) {
        self.builtin_catalog_ready = package_catalog().is_ok();
        self.builtin_uri = self
            .builtin_catalog_ready
            .then(materialize_builtin_interface)
            .flatten();
        if let Some(uri) = params
            .get("rootUri")
            .and_then(Value::as_str)
            .and_then(uri_to_path)
        {
            self.roots.push(uri);
        }
        if let Some(folders) = params.get("workspaceFolders").and_then(Value::as_array) {
            for folder in folders {
                if let Some(path) = folder
                    .get("uri")
                    .and_then(Value::as_str)
                    .and_then(uri_to_path)
                    && !self.roots.contains(&path)
                {
                    self.roots.push(path);
                }
            }
        }
        if let Some(entries) = params
            .get("initializationOptions")
            .and_then(|options| options.get("entryFiles"))
            .and_then(Value::as_array)
        {
            self.entry_files = entries
                .iter()
                .filter_map(Value::as_str)
                .map(PathBuf::from)
                .collect();
        }
        for root in self.roots.clone() {
            self.scan_workspace(&root);
        }
    }

    pub(super) fn scan_workspace(&mut self, root: &Path) {
        let mut pending = vec![root.to_path_buf()];
        let mut visited = 0_usize;
        while let Some(directory) = pending.pop() {
            if visited >= 10_000 {
                break;
            }
            let Ok(entries) = std::fs::read_dir(directory) else {
                continue;
            };
            for entry in entries.flatten() {
                visited += 1;
                let path = entry.path();
                if path.is_dir() {
                    if !matches!(
                        path.file_name().and_then(|name| name.to_str()),
                        Some("target" | ".git")
                    ) {
                        pending.push(path);
                    }
                } else if path.extension().and_then(|extension| extension.to_str()) == Some("tl")
                    && let Ok(text) = std::fs::read_to_string(&path)
                {
                    let uri = path_to_uri(&path);
                    self.documents
                        .entry(uri.clone())
                        .or_insert_with(|| Document::new(uri, normalize_path(path), text));
                }
            }
        }
    }

    pub(super) fn loader(&self) -> OverlayLoader {
        OverlayLoader {
            reads: Default::default(),
            files: self
                .documents
                .values()
                .map(|document| (document.path.clone(), document.text.clone()))
                .collect(),
        }
    }

    pub(super) fn open(&mut self, params: &Value) -> Option<String> {
        let item = params.get("textDocument")?;
        let supplied_uri = item.get("uri")?.as_str()?;
        let text = item.get("text")?.as_str()?.to_owned();
        let (uri, path) = document_uri_and_path(supplied_uri)?;
        self.documents
            .insert(uri.clone(), Document::new(uri.clone(), path, text));
        Some(uri)
    }

    pub(super) fn change(&mut self, params: &Value) -> Option<String> {
        let supplied_uri = params.get("textDocument")?.get("uri")?.as_str()?;
        let (uri, _) = document_uri_and_path(supplied_uri)?;
        let document = self.documents.get_mut(&uri)?;
        let mut text = document.text.to_string();
        let mut index = document.index.clone();
        for change in params.get("contentChanges")?.as_array()? {
            let replacement = change.get("text")?.as_str()?;
            if let Some(range) = change.get("range") {
                let start = position_offset(&index, range.get("start")?)?;
                let end = position_offset(&index, range.get("end")?)?;
                if start > end || !text.is_char_boundary(start) || !text.is_char_boundary(end) {
                    return None;
                }
                text.replace_range(start..end, replacement);
                index = LineIndex::new(text.as_str());
            } else {
                text = replacement.to_owned();
                index = LineIndex::new(text.as_str());
            }
        }
        // 批次内坐标按前一条临时文本解析；全部有效后才发布分析状态。
        document.replace(text);
        Some(uri)
    }

    /// 关闭覆盖缓冲区；工作区内仍存在的文件恢复为当前磁盘快照。
    pub(super) fn close(&mut self, supplied_uri: &str) -> Option<(String, bool)> {
        let (uri, _) = document_uri_and_path(supplied_uri)?;
        let Some(document) = self.documents.remove(&uri) else {
            return Some((uri, false));
        };
        if self
            .roots
            .iter()
            .any(|root| document.path.starts_with(root))
            && let Ok(text) = std::fs::read_to_string(&document.path)
        {
            self.documents.insert(
                uri.to_owned(),
                Document::new(uri.to_owned(), document.path, text),
            );
            Some((uri, true))
        } else {
            Some((uri, false))
        }
    }

    /// 把同一次成功编译产生的各文件索引绑定到仍匹配当前覆盖内容的文档。
    pub(super) fn apply_compilation_analysis(&mut self, analysis: Arc<CompilationAnalysis>) {
        for source in analysis.sources() {
            let source_path = normalize_path(PathBuf::from(source.source_name()));
            if let Some(document) = self.documents.values_mut().find(|document| {
                document.path == source_path && document.text.as_ref() == source.source_contents()
            }) {
                document.dependencies_valid = true;
                document.resolved_compilation = Some((
                    document.text.clone(),
                    source.source_id(),
                    Arc::clone(&analysis),
                ));
            }
        }
    }
}
