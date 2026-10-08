//! 工作台官方知识、用户级层与工作区级层的装配、刷新和状态展示。
//!
//! 层内容是层根下的可读文档，派生缓存只在显式刷新时写回；工作区层沿用
//! `workspace.read_range` 逐文件授权，未受信任工作区不装配该层。

use std::{collections::BTreeMap, path::Path, path::PathBuf, sync::RwLock};

use teal_agent::{
    ActionContext, ActionSpec, Decision, KnowledgeProvider, PermissionProvider, ResourceVersion,
};
use teal_knowledge::{
    KnowledgeCatalog, KnowledgeEntry, KnowledgeError, KnowledgeHit, KnowledgeOverview,
    KnowledgeScope, KnowledgeStore, LayerIndex, LayerOptions, LayeredKnowledge,
};

/// 一个本地知识层的刷新状态。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KnowledgeLayerStatus {
    /// 层范围。
    pub scope: KnowledgeScope,
    /// 层根目录。
    pub root: PathBuf,
    /// 层内文件数。
    pub files: usize,
    /// 层内 UTF-8 字节数。
    pub bytes: usize,
    /// 本次内容是否来自派生缓存。
    pub cached: bool,
    /// 最近一次失败原因；成功时为空。
    pub error: Option<String>,
}

impl KnowledgeLayerStatus {
    /// 尚未加载或已禁用的层状态。
    #[must_use]
    fn disabled(scope: KnowledgeScope) -> Self {
        Self {
            scope,
            root: PathBuf::new(),
            files: 0,
            bytes: 0,
            cached: false,
            error: None,
        }
    }

    /// 是否已经装配该层。
    #[must_use]
    pub fn configured(&self) -> bool {
        !self.root.as_os_str().is_empty()
    }
}

/// 逐层状态；官方层不参与本地刷新。
#[derive(Clone, Debug)]
struct LayerStatuses {
    user: KnowledgeLayerStatus,
    project: KnowledgeLayerStatus,
}

/// 组合内嵌官方知识与本地知识层的工作台提供器。
pub struct WorkbenchKnowledge {
    layers: LayeredKnowledge,
    statuses: RwLock<LayerStatuses>,
    inject_inline: bool,
}

impl WorkbenchKnowledge {
    /// 创建只含官方知识的工作台提供器。
    ///
    /// # Errors
    /// 内嵌知识制品解码或校验失败时返回错误。
    pub fn official() -> Result<Self, KnowledgeError> {
        Ok(Self {
            layers: LayeredKnowledge::new(KnowledgeCatalog::embedded()?),
            statuses: RwLock::new(LayerStatuses {
                user: KnowledgeLayerStatus::disabled(KnowledgeScope::User),
                project: KnowledgeLayerStatus::disabled(KnowledgeScope::Project),
            }),
            inject_inline: true,
        })
    }

    /// 装配用户级与工作区级层根；未受信任的工作区不装配工作区层。
    ///
    /// 缓存是否读写由 `options.cache` 决定，`teal run` 与工作台启动只读。
    ///
    /// # Errors
    /// 层根或配额无效时返回错误。
    pub fn with_layers(
        mut self,
        user_root: Option<PathBuf>,
        workspace_root: Option<PathBuf>,
        options: LayerOptions,
    ) -> Result<Self, KnowledgeError> {
        let user = match user_root {
            Some(root) => Some(KnowledgeStore::open(root, KnowledgeScope::User, options)?),
            None => None,
        };
        let project = match workspace_root {
            Some(root) => Some(KnowledgeStore::open(
                root,
                KnowledgeScope::Project,
                options,
            )?),
            None => None,
        };
        {
            let mut statuses = self.statuses.write().map_err(|_| poisoned())?;
            if let Some(store) = &user {
                statuses.user = KnowledgeLayerStatus {
                    scope: KnowledgeScope::User,
                    root: store.root().to_path_buf(),
                    files: 0,
                    bytes: 0,
                    cached: false,
                    error: None,
                };
            }
            if let Some(store) = &project {
                statuses.project = KnowledgeLayerStatus {
                    scope: KnowledgeScope::Project,
                    root: store.root().to_path_buf(),
                    files: 0,
                    bytes: 0,
                    cached: false,
                    error: None,
                };
            }
        }
        self.layers = self.layers.with_layers(user, project);
        Ok(self)
    }

    /// 设置行内补全是否使用知识摘要。
    #[must_use]
    pub fn with_inline(mut self, enabled: bool) -> Self {
        self.inject_inline = enabled;
        self
    }

    /// 返回是否允许行内补全注入知识。
    #[must_use]
    pub fn inline_enabled(&self) -> bool {
        self.inject_inline
    }

    /// 返回本地层装配。
    #[must_use]
    pub fn layers(&self) -> &LayeredKnowledge {
        &self.layers
    }

    /// 返回某一层的最近状态。
    #[must_use]
    pub fn status(&self, scope: KnowledgeScope) -> KnowledgeLayerStatus {
        self.statuses.read().map_or_else(
            |_| KnowledgeLayerStatus::disabled(scope),
            |statuses| match scope {
                KnowledgeScope::Official => KnowledgeLayerStatus::disabled(scope),
                KnowledgeScope::User => statuses.user.clone(),
                KnowledgeScope::Project => statuses.project.clone(),
            },
        )
    }

    /// 刷新全部已装配的本地层。
    ///
    /// # Errors
    /// 层读取、授权或缓存写入失败时返回错误；失败不会清空既有索引。
    pub async fn refresh(
        &self,
        permissions: &dyn PermissionProvider,
        context: ActionContext,
        write_cache: bool,
    ) -> Result<Vec<KnowledgeLayerStatus>, String> {
        let mut statuses = Vec::new();
        for scope in [KnowledgeScope::User, KnowledgeScope::Project] {
            if self.layers.store(scope).is_none() {
                continue;
            }
            statuses.push(
                self.refresh_layer(scope, permissions, context.clone(), write_cache)
                    .await?,
            );
        }
        Ok(statuses)
    }

    /// 刷新一个本地层。
    ///
    /// # Errors
    /// 层未装配、读取失败、工作区层授权被拒或缓存写入失败时返回错误。
    pub async fn refresh_layer(
        &self,
        scope: KnowledgeScope,
        permissions: &dyn PermissionProvider,
        context: ActionContext,
        write_cache: bool,
    ) -> Result<KnowledgeLayerStatus, String> {
        let store = self
            .layers
            .store(scope)
            .ok_or_else(|| format!("the {} knowledge layer is not configured", scope.name()))?;
        let result = self
            .load_layer(store, scope, permissions, context, write_cache)
            .await;
        match result {
            Ok(layer) => {
                self.layers
                    .set_layer(scope, layer.clone())
                    .map_err(|error| error.to_string())?;
                let status = KnowledgeLayerStatus {
                    scope,
                    root: store.root().to_path_buf(),
                    files: layer.files(),
                    bytes: layer.bytes(),
                    cached: layer.cached(),
                    error: None,
                };
                self.store_status(scope, status.clone())?;
                Ok(status)
            }
            Err(error) => {
                let mut status = self.status(scope);
                status.error = Some(error.clone());
                self.store_status(scope, status)?;
                Err(error)
            }
        }
    }

    /// 读取一层文档、按需授权，并复用一致的派生缓存。
    async fn load_layer(
        &self,
        store: &KnowledgeStore,
        scope: KnowledgeScope,
        permissions: &dyn PermissionProvider,
        context: ActionContext,
        write_cache: bool,
    ) -> Result<LayerIndex, String> {
        let documents = store.documents().map_err(|error| error.to_string())?;
        if scope == KnowledgeScope::Project {
            for document in &documents {
                authorize(permissions, context.clone(), document).await?;
            }
        }
        let layer = match store.cached(&documents) {
            Some(cached) => cached,
            None => {
                let mut texts = BTreeMap::new();
                for document in &documents {
                    texts.insert(
                        document.absolute.clone(),
                        store.read_document(document).map_err(|error| {
                            format!("knowledge document {}: {error}", document.relative)
                        })?,
                    );
                }
                store
                    .build_from_map(&texts, &documents)
                    .map_err(|error| error.to_string())?
            }
        };
        if write_cache {
            store
                .save_cache(&documents, &layer)
                .map_err(|error| format!("knowledge cache write failed: {error}"))?;
        }
        Ok(layer)
    }

    /// 写入一层状态。
    fn store_status(
        &self,
        scope: KnowledgeScope,
        status: KnowledgeLayerStatus,
    ) -> Result<(), String> {
        let mut statuses = self
            .statuses
            .write()
            .map_err(|_| "knowledge status lock poisoned".to_owned())?;
        match scope {
            KnowledgeScope::Official => {}
            KnowledgeScope::User => statuses.user = status,
            KnowledgeScope::Project => statuses.project = status,
        }
        Ok(())
    }
}

impl KnowledgeProvider for WorkbenchKnowledge {
    fn toolchain_version(&self) -> &str {
        self.layers.toolchain_version()
    }

    fn content_digest(&self) -> &str {
        self.layers.content_digest()
    }

    fn toolchain_digest(&self) -> &str {
        self.layers.toolchain_digest()
    }

    fn overview(&self, budget: usize) -> KnowledgeOverview {
        self.layers.overview(budget)
    }

    fn search(
        &self,
        query: &str,
        scope: Option<KnowledgeScope>,
        limit: usize,
    ) -> Result<Vec<KnowledgeHit>, KnowledgeError> {
        self.layers.search(query, scope, limit)
    }

    fn read(&self, id: &str, max_bytes: usize) -> Result<KnowledgeEntry, KnowledgeError> {
        self.layers.read(id, max_bytes)
    }

    fn read_ref(
        &self,
        reference: &str,
        digest: &str,
        max_bytes: usize,
    ) -> Result<KnowledgeEntry, KnowledgeError> {
        self.layers.read_ref(reference, digest, max_bytes)
    }

    fn lookup(&self, symbol: &str, limit: usize) -> Result<Vec<KnowledgeHit>, KnowledgeError> {
        self.layers.lookup(symbol, limit)
    }
}

/// 对工作区层的一份文档执行 `workspace.read_range` 授权。
async fn authorize(
    permissions: &dyn PermissionProvider,
    context: ActionContext,
    document: &teal_knowledge::DocumentRef,
) -> Result<(), String> {
    let resource = format!("file://{}", document.absolute.display());
    let action = ActionSpec {
        id: format!("knowledge:{}", document.absolute.display()),
        tool: "workspace.read_range".into(),
        arguments: serde_json::json!({"path": document.absolute}),
        resources: vec![ResourceVersion {
            access: teal_agent::ActionResourceAccess::Read,
            resource,
            version: document.sha256.clone(),
        }],
    };
    match permissions
        .authorize(context, action)
        .await
        .map_err(|error| error.to_string())?
    {
        Decision::Allow => Ok(()),
        Decision::Deny(reason) => Err(format!("knowledge denied: {reason}")),
        Decision::ApprovalRequired(_) => Err("knowledge requires approval".into()),
    }
}

/// 判断一个工作区路径是否位于工作区知识层内。
#[must_use]
pub fn within_workspace_layer(root: &Path, path: &Path) -> bool {
    path.starts_with(root)
}

fn poisoned() -> KnowledgeError {
    KnowledgeError::Invalid("knowledge status lock poisoned".into())
}
