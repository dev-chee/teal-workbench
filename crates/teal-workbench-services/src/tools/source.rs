//! 工作台源码查询的不可变覆盖层与按需磁盘快照。
//!
//! 查询不保存文件、不执行 VM，也不持有 UI 缓冲区；调用者在同一事件循环中捕获文本。
use std::{
    cell::RefCell,
    collections::BTreeMap,
    path::{Component, Path, PathBuf},
    sync::OnceLock,
};

use teal_compiler::{
    CompileOptions, LoadedSource, SemanticAnalysis, Source, SourceLoadError, SourceLoader,
};
use teal_workbench_core::ensure_workspace_path;

/// 一次源码查询拥有的工作区文本快照。
///
/// 打开文档的文本优先于磁盘；普通导入首次读取后固定，内置扩展包由可信目录解析。
/// 使用 `new(root, overlays)` 捕获文本，再调用 `analyze` 或 `inspect`；查询不会执行程序。
pub struct SourceQuerySnapshot {
    root: PathBuf,
    files: RefCell<BTreeMap<PathBuf, String>>,
}

impl SourceQuerySnapshot {
    /// 捕获有路径的打开文档；无关工作区文档不属于本次查询。
    ///
    /// # Errors
    /// 根目录无法解析时返回错误；其他工作区或经符号链接越界的覆盖文档不会被接纳。
    pub fn new(
        root: &Path,
        overlays: impl IntoIterator<Item = (PathBuf, String)>,
    ) -> Result<Self, String> {
        let root = root.canonicalize().map_err(|error| error.to_string())?;
        let snapshot = Self {
            root,
            files: RefCell::new(BTreeMap::new()),
        };
        for (path, text) in overlays {
            // 应用可以同时打开其他工作区文档；不能把这些文本导入当前查询。
            let Ok(path) = snapshot.resolve(&path) else {
                continue;
            };
            snapshot.files.borrow_mut().insert(path, text);
        }
        Ok(snapshot)
    }

    /// 返回固定的源码内容；覆盖层之外的文件只在首次查询时读取。
    ///
    /// # Errors
    /// 路径越界、文件不可读或不是 UTF-8 时返回错误。
    pub fn source(&self, path: &Path) -> Result<Source, String> {
        let path = self.resolve(path)?;
        let cached = self.files.borrow().get(&path).cloned();
        let text = match cached {
            Some(text) => text,
            None => {
                let text = std::fs::read_to_string(&path).map_err(|error| error.to_string())?;
                self.files.borrow_mut().insert(path.clone(), text.clone());
                text
            }
        };
        Ok(Source::new(path.to_string_lossy(), text))
    }

    /// 构造容错的单文件符号、引用和导入索引，允许未完成的源码。
    ///
    /// # Errors
    /// 沿用 `source` 的路径与读取错误；语法不完整不导致查询失败。
    pub fn analyze(&self, path: &Path) -> Result<SemanticAnalysis, String> {
        self.source(path)
            .map(|source| teal_compiler::analyze_source(&source))
    }

    /// 编译入口及其导入快照，返回解析完成的分析与图，不执行程序。
    ///
    /// # Errors
    /// 源码、导入、包目录或编译失败时返回诊断；导入不允许逃出工作区。
    pub fn inspect(
        &self,
        path: &Path,
        options: CompileOptions,
    ) -> Result<teal::SourceInspection, String> {
        teal::inspect_source(self.source(path)?, self, options).map_err(|error| error.to_string())
    }

    fn resolve(&self, path: &Path) -> Result<PathBuf, String> {
        let path = if path.is_absolute() {
            path.to_owned()
        } else {
            self.root.join(path)
        };
        ensure_workspace_path(&self.root, &path)?;
        if let Ok(path) = path.canonicalize() {
            return Ok(path);
        }
        // 新建而未保存的文档没有磁盘身份，但仍需折叠词法别名命中覆盖层。
        let mut normalized = PathBuf::new();
        for component in path.components() {
            match component {
                Component::CurDir => {}
                Component::ParentDir => {
                    normalized.pop();
                }
                other => normalized.push(other.as_os_str()),
            }
        }
        Ok(normalized)
    }
}

impl SourceLoader for SourceQuerySnapshot {
    fn load(&self, importer: &Path, import: &str) -> Result<LoadedSource, SourceLoadError> {
        static CATALOG: OnceLock<Result<teal_packages::PackageCatalog, String>> = OnceLock::new();
        let error = |message: String| SourceLoadError { message };
        let catalog = CATALOG
            .get_or_init(|| {
                teal_packages::PackageCatalog::discover().map_err(|error| error.to_string())
            })
            .as_ref()
            .map_err(|message| error(message.clone()))?;
        if let Some(source) = catalog.load_package(importer, import)? {
            return Ok(source);
        }
        let path = importer.parent().unwrap_or(&self.root).join(import);
        let identity = self.resolve(&path).map_err(error)?;
        let source = self.source(&identity).map_err(error)?;
        Ok(LoadedSource {
            identity,
            source,
            precompiled: None,
        })
    }
}
