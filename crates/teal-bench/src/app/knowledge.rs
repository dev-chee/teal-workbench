//! 工作台本地知识层的刷新、缓存写回与状态展示。

use std::{path::PathBuf, sync::Arc};
use sha2::{Digest, Sha256};

use teal_knowledge::KnowledgeScope;

use super::{App, Message, Severity};

impl App {
    pub(crate) fn proposal_store(
        &self,
    ) -> Result<(Arc<teal_knowledge::KnowledgeProposalStore>, String, PathBuf), String> {
        let config = teal_config::load_declarations(Some(&self.workspace))
            .map_err(|error| error.to_string())?;
        if !config.workspace_trusted() || !config.knowledge().enabled {
            return Err("workspace knowledge proposals require a trusted workspace".into());
        }
        let workspace = self
            .workspace
            .canonicalize()
            .map_err(|error| error.to_string())?;
        let identity = format!(
            "{:x}",
            Sha256::digest(workspace.to_string_lossy().as_bytes())
        );
        let store = teal_knowledge::KnowledgeProposalStore::open_with_audit(
            &config
                .paths()
                .teal_home
                .join("state/knowledge-proposals.sqlite3"),
            teal_security::process_context(),
        )
        .map_err(|error| error.to_string())?;
        Ok((
            Arc::new(store),
            identity,
            config.knowledge().workspace_layer_root(&workspace),
        ))
    }

    /// 刷新本地知识层；只有显式命令才写回派生缓存。
    ///
    /// [`KnowledgeScope::Official`] 或 `None` 之外的取值只在对应层已装配时刷新。
    ///
    /// # Errors
    /// 配置不可读、层根不可访问、工作区层授权被拒或缓存写入失败时返回错误；官方知识始终可用。
    pub(crate) async fn refresh_knowledge(
        &self,
        scope: Option<KnowledgeScope>,
        write_cache: bool,
    ) -> Result<Vec<teal_workbench_services::KnowledgeLayerStatus>, String> {
        let config = match teal_config::load_declarations(Some(&self.workspace)) {
            Ok(config) => config.knowledge().clone(),
            Err(error) => return Err(error.to_string()),
        };
        if !config.enabled {
            return Ok(Vec::new());
        }
        let context = teal_agent::ActionContext {
            run_id: "teal-bench/knowledge".into(),
            task_id: "knowledge".into(),
            generation: 0,
            policy_version: String::new(),
        };
        let permissions = self.agent_permissions.as_ref();
        match scope {
            Some(scope) => Ok(vec![
                self.agent_knowledge
                    .refresh_layer(scope, permissions, context, write_cache)
                    .await?,
            ]),
            None => {
                self.agent_knowledge
                    .refresh(permissions, context, write_cache)
                    .await
            }
        }
    }

    /// Agent 变更落地后，如果目标位于工作区知识层内则后台刷新该层。
    ///
    /// 刷新失败只记录告警，不回滚已经落地的编辑，也不写回派生缓存。
    pub(crate) fn refresh_workspace_knowledge_after_write(&mut self, paths: &[PathBuf]) {
        let config = match teal_config::load_declarations(Some(&self.workspace)) {
            Ok(config) => config.knowledge().clone(),
            Err(_) => return,
        };
        if !config.enabled {
            return;
        }
        let root = config.workspace_layer_root(&self.workspace);
        let touched = paths.iter().any(|path| {
            let path = if path.is_absolute() {
                path.clone()
            } else {
                self.workspace.join(path)
            };
            teal_workbench_services::within_workspace_layer(&root, &path)
        });
        if !touched {
            return;
        }
        let knowledge = Arc::clone(&self.agent_knowledge);
        let permissions = Arc::clone(&self.agent_permissions);
        let output = self.background_tx.clone();
        self.tasks.spawn(async move {
            let context = teal_agent::ActionContext {
                run_id: "teal-bench/knowledge".into(),
                task_id: "knowledge".into(),
                generation: 0,
                policy_version: String::new(),
            };
            let message = match knowledge
                .refresh_layer(
                    KnowledgeScope::Project,
                    permissions.as_ref(),
                    context,
                    false,
                )
                .await
            {
                Ok(status) => Message::new(
                    Severity::Info,
                    "knowledge",
                    format!(
                        "Workspace knowledge refreshed: {} files, {} bytes",
                        status.files, status.bytes
                    ),
                ),
                Err(error) => Message::new(
                    Severity::Warning,
                    "knowledge",
                    format!("workspace knowledge refresh failed: {error}"),
                ),
            };
            let _ = output.send(message).await;
        });
    }
}
