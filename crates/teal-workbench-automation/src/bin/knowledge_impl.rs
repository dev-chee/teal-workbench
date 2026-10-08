//! `teal knowledge` 的非交互入口：官方制品校验、分层查询、导出与显式写入。
//!
//! 官方知识来自当前二进制构建期生成的内嵌制品；用户级与工作区级层是层根下的可读文档。
//! `add`、`write`、`refresh` 和显式知识审核重试可能写盘；工作区层只在用户层记录该工作区为
//! trusted 时装配。草稿状态与知识索引分开保存。

use std::{
    path::{Path, PathBuf},
    sync::Arc,
};
use sha2::{Digest, Sha256};

use teal_knowledge::{
    ExportFormat, KnowledgeCatalog, KnowledgeScope, KnowledgeStore, LayerIndex, LayerOptions,
    LayeredKnowledge,
};

use crate::KnowledgeCommand;

/// 执行一个知识子命令；错误以退出码 1 呈现。
///
/// # Errors
/// 配置不可读、层未装配、查询无效、写入或导出失败时返回错误文本。
pub(crate) fn execute(command: &KnowledgeCommand) -> Result<(), String> {
    match command {
        KnowledgeCommand::Proposals => {
            let context = Context::load()?;
            if !context.trusted || !context.enabled() {
                return Err("workspace knowledge proposals require a trusted workspace".into());
            }
            if !context
                .teal_home
                .join("state/knowledge-proposals.sqlite3")
                .exists()
            {
                println!("No Agent knowledge proposals");
                return Ok(());
            }
            let (context, proposals, workspace_id) = proposal_context()?;
            for proposal in proposals
                .list(&workspace_id)
                .map_err(|error| error.to_string())?
            {
                println!("{}\t{:?}\t{}", proposal.id, proposal.status, proposal.title);
            }
            let _ = context;
            Ok(())
        }
        KnowledgeCommand::Show { id } => {
            let context = Context::load()?;
            if !context.trusted || !context.enabled() {
                return Err("workspace knowledge proposals require a trusted workspace".into());
            }
            if !context
                .teal_home
                .join("state/knowledge-proposals.sqlite3")
                .exists()
            {
                return Err("knowledge proposal does not exist in this workspace".into());
            }
            let (_, proposals, workspace_id) = proposal_context()?;
            let proposal = proposals
                .get(id)
                .map_err(|error| error.to_string())?
                .filter(|proposal| proposal.workspace == workspace_id)
                .ok_or_else(|| "knowledge proposal does not exist in this workspace".to_owned())?;
            println!(
                "{}",
                serde_json::to_string_pretty(&proposal).map_err(|error| error.to_string())?
            );
            Ok(())
        }
        KnowledgeCommand::RetryReview { id } => retry_review(id),
        KnowledgeCommand::Status => status(),
        KnowledgeCommand::Verify => verify(),
        KnowledgeCommand::Search { query, scope } => {
            let layers = layered(false)?;
            let scope = scope_of(scope.as_deref());
            let hits = layers.search(query, scope, 10).map_err(error_text)?;
            if hits.is_empty() {
                println!("No matching knowledge entries");
            }
            for hit in hits {
                println!(
                    "{}:{}:{} {}",
                    hit.scope.name(),
                    hit.kind.name(),
                    hit.id,
                    hit.snippet
                );
            }
            Ok(())
        }
        KnowledgeCommand::Read { id, max_bytes } => {
            let layers = layered(false)?;
            let entry = layers.read(id, *max_bytes).map_err(error_text)?;
            println!("{}", entry.body);
            Ok(())
        }
        KnowledgeCommand::Lookup { symbol, limit } => {
            let layers = layered(false)?;
            for hit in layers.lookup(symbol, *limit).map_err(error_text)? {
                println!(
                    "{}:{}:{} {}",
                    hit.scope.name(),
                    hit.kind.name(),
                    hit.id,
                    hit.snippet
                );
            }
            Ok(())
        }
        KnowledgeCommand::Export {
            format,
            scope,
            output,
            force,
        } => export(format, scope.as_deref(), output.as_deref(), *force),
        KnowledgeCommand::Add { file, layer, name } => add(file, layer, name.as_deref()),
        KnowledgeCommand::Write {
            layer,
            title,
            text,
            file,
            name,
        } => write(
            layer,
            title,
            text.as_deref(),
            file.as_deref(),
            name.as_deref(),
        ),
        KnowledgeCommand::Refresh { layer } => refresh(layer.as_deref()),
    }
}

fn proposal_context() -> Result<(Context, teal_knowledge::KnowledgeProposalStore, String), String> {
    let context = Context::load()?;
    if !context.enabled() || !context.trusted {
        return Err("workspace knowledge proposals require a trusted workspace".into());
    }
    let workspace = context
        .workspace
        .canonicalize()
        .map_err(|error| error.to_string())?;
    let workspace_id = format!(
        "{:x}",
        Sha256::digest(workspace.to_string_lossy().as_bytes())
    );
    let proposals = teal_knowledge::KnowledgeProposalStore::open_with_audit(
        &context.teal_home.join("state/knowledge-proposals.sqlite3"),
        teal_security::process_context(),
    )
    .map_err(|error| error.to_string())?;
    Ok((context, proposals, workspace_id))
}

fn retry_review(id: &str) -> Result<(), String> {
    let (context, proposals, workspace_id) = proposal_context()?;
    let proposal = proposals
        .get(id)
        .map_err(|error| error.to_string())?
        .filter(|proposal| proposal.workspace == workspace_id)
        .ok_or_else(|| "knowledge proposal does not exist in this workspace".to_owned())?;
    if proposal.status == teal_knowledge::ProposalStatus::NeedsEvidence {
        proposals
            .retry_review(id)
            .map_err(|error| error.to_string())?;
    }
    let layers = layered(false)?;
    let store = context
        .store(KnowledgeScope::Project)?
        .ok_or_else(|| "workspace knowledge layer is unavailable".to_owned())?
        .with_audit_context(teal_security::process_context());
    let effective = teal_config::load_declarations(Some(&context.workspace))
        .map_err(|error| error.to_string())?;
    let permissions = KnowledgeCliPermissions {
        policy: teal_agent::WorkspacePermissionPolicy::new(effective.paths().clone()),
    };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| error.to_string())?;
    let status = runtime.block_on(teal_workbench_automation::process_knowledge_proposal(
        &proposals,
        id,
        &context.workspace,
        &context.config.workspace_layer_root(&context.workspace),
        Some(&layers),
        &permissions,
        teal_workbench_automation::KnowledgePublicationWriter::Headless(&store),
        teal_security::process_context(),
    ))?;
    println!("{id}\t{status:?}");
    if status == teal_knowledge::ProposalStatus::Published {
        refresh(Some("workspace"))?;
    }
    Ok(())
}

/// 在已完成的源码 Agent 运行之后审核本工作区待审草稿。
pub(crate) async fn process_pending_reviews() -> Result<(), String> {
    let (context, proposals, workspace_id) = proposal_context()?;
    let runtime_store =
        teal_agent::RuntimeStore::open(&context.workspace).map_err(|error| error.to_string())?;
    let completed = runtime_store
        .runs()
        .map_err(|error| error.to_string())?
        .into_iter()
        .filter(|run| run.state == teal_agent::RunState::Completed)
        .map(|run| run.id)
        .collect::<std::collections::BTreeSet<_>>();
    if completed.is_empty() {
        return Ok(());
    }
    let store = context
        .store(KnowledgeScope::Project)?
        .ok_or_else(|| "workspace knowledge layer is unavailable".to_owned())?
        .with_audit_context(teal_security::process_context());
    let layers = layered(false)?;
    let effective = teal_config::load_declarations(Some(&context.workspace))
        .map_err(|error| error.to_string())?;
    let permissions = KnowledgeCliPermissions {
        policy: teal_agent::WorkspacePermissionPolicy::new(effective.paths().clone()),
    };
    let mut published = false;
    for proposal in proposals
        .list(&workspace_id)
        .map_err(|error| error.to_string())?
        .into_iter()
        .filter(|proposal| {
            matches!(
                proposal.status,
                teal_knowledge::ProposalStatus::Pending
                    | teal_knowledge::ProposalStatus::Reviewing
                    | teal_knowledge::ProposalStatus::Unknown
            ) && completed.contains(&proposal.source_run)
        })
    {
        match teal_workbench_automation::process_knowledge_proposal(
            &proposals,
            &proposal.id,
            &context.workspace,
            &context.config.workspace_layer_root(&context.workspace),
            Some(&layers),
            &permissions,
            teal_workbench_automation::KnowledgePublicationWriter::Headless(&store),
            teal_security::process_context(),
        )
        .await
        {
            Ok(teal_knowledge::ProposalStatus::Published) => published = true,
            Ok(_) => {}
            Err(error) => eprintln!("Knowledge proposal {}: {error}", proposal.id),
        }
    }
    if published {
        refresh(Some("workspace"))?;
    }
    Ok(())
}

struct KnowledgeCliPermissions {
    policy: teal_agent::WorkspacePermissionPolicy,
}

impl teal_agent::PermissionProvider for KnowledgeCliPermissions {
    fn version(&self) -> &str {
        "knowledge-cli/v1"
    }

    fn authorize(
        &self,
        context: teal_agent::ActionContext,
        action: teal_agent::ActionSpec,
    ) -> teal_agent::ProviderFuture<Result<teal_agent::Decision, teal_agent::PolicyError>> {
        let result = self
            .policy
            .evaluate(context, action)
            .map(|evaluation| match evaluation {
                teal_agent::PermissionEvaluation::Allow => teal_agent::Decision::Allow,
                teal_agent::PermissionEvaluation::Deny(reason) => {
                    teal_agent::Decision::Deny(reason)
                }
                teal_agent::PermissionEvaluation::Prompt(prompt) => {
                    teal_agent::Decision::ApprovalRequired(prompt)
                }
            });
        Box::pin(async move { result })
    }
}

/// 装配官方知识与本地层；`write_cache` 只有显式命令才为真。
///
/// # Errors
/// 配置不可读、内嵌制品无效或层装配失败时返回错误。
pub(crate) fn layered(write_cache: bool) -> Result<LayeredKnowledge, String> {
    let context = Context::load()?;
    let mut layers = LayeredKnowledge::new(context.official()?);
    let options = context.options();
    let user = context
        .enabled()
        .then(|| {
            KnowledgeStore::open(
                context.config.user_layer_root(&context.teal_home),
                KnowledgeScope::User,
                options,
            )
            .map(|store| store.with_audit_context(teal_security::process_context()))
        })
        .transpose()
        .map_err(error_text)?;
    let project = (context.enabled() && context.trusted)
        .then(|| {
            KnowledgeStore::open(
                context.config.workspace_layer_root(&context.workspace),
                KnowledgeScope::Project,
                options,
            )
            .map(|store| store.with_audit_context(teal_security::process_context()))
        })
        .transpose()
        .map_err(error_text)?;
    if write_cache {
        for store in [&user, &project].into_iter().flatten() {
            let documents = store.documents().map_err(error_text)?;
            let layer = load_layer(store, &documents)?;
            store.save_cache(&documents, &layer).map_err(error_text)?;
            layers.set_layer(store.scope(), layer).map_err(error_text)?;
        }
    } else {
        let mut loaded = Vec::new();
        for store in [&user, &project].into_iter().flatten() {
            let documents = store.documents().map_err(error_text)?;
            loaded.push((store.scope(), load_layer(store, &documents)?));
        }
        for (scope, layer) in loaded {
            layers.set_layer(scope, layer).map_err(error_text)?;
        }
    }
    layers = layers.with_layers(user, project);
    Ok(layers)
}

/// 显示官方制品身份与各层状态。
fn status() -> Result<(), String> {
    let context = Context::load()?;
    // 状态查询只解码并做廉价结构校验；严格的内容与索引重建校验属于 `teal knowledge verify`。
    let catalog = KnowledgeCatalog::embedded().map_err(error_text)?;
    println!(
        "knowledge\t{}\t{}\t{}",
        catalog.toolchain_version(),
        catalog.toolchain_digest(),
        catalog.content_digest()
    );
    for scope in [KnowledgeScope::User, KnowledgeScope::Project] {
        let Some(store) = context.store(scope)? else {
            println!("{}\tnot configured", scope.name());
            continue;
        };
        let documents = store.documents().map_err(error_text)?;
        let cached = if documents.is_empty() {
            None
        } else {
            store.cached(&documents)
        };
        let layer = match cached {
            Some(layer) => layer,
            None => store
                .build_from_map(&Default::default(), &documents)
                .map_err(error_text)?,
        };
        println!(
            "{}\t{}\tfiles {}\tbytes {}\tcache {}",
            scope.name(),
            store.root().display(),
            layer.files(),
            layer.bytes(),
            if layer.cached() { "hit" } else { "rebuilt" }
        );
    }
    Ok(())
}

/// 严格校验当前二进制内嵌制品。
fn verify() -> Result<(), String> {
    let catalog = KnowledgeCatalog::verify().map_err(error_text)?;
    println!(
        "knowledge\t{}\t{}\t{}\tverified",
        catalog.toolchain_version(),
        catalog.toolchain_digest(),
        catalog.content_digest()
    );
    Ok(())
}

/// 导出只读阅读视图。
fn export(
    format: &str,
    scope: Option<&str>,
    output: Option<&Path>,
    force: bool,
) -> Result<(), String> {
    let context = Context::load()?;
    let layers = layered(false)?;
    let format = ExportFormat::parse(format).ok_or("--format must be md, jsonl, or toml")?;
    let records = layers.entries(scope_of(scope));
    let directory = match output {
        Some(path) => path.to_path_buf(),
        None => context
            .config
            .workspace_layer_root(&context.workspace)
            .join("export"),
    };
    let roots = vec![
        context.config.user_layer_root(&context.teal_home),
        context.config.workspace_layer_root(&context.workspace),
    ];
    let target = teal_knowledge::ExportTarget::resolve(&directory, &roots).map_err(error_text)?;
    let report = teal_knowledge::export(&target, &records, format, force).map_err(error_text)?;
    println!(
        "knowledge\texport\t{}\tformat {}\tentries {}\tfiles {}",
        report.directory.display(),
        format.name(),
        report.entries,
        report.files
    );
    Ok(())
}

/// 把一份文件复制进某一层并刷新它的派生缓存。
fn add(file: &Path, layer: &str, name: Option<&str>) -> Result<(), String> {
    let (store, context) = layer_store(layer)?;
    let target = store.import(file, name).map_err(error_text)?;
    report_written(&store, &target, &context)
}

/// 写入一篇带标题的文档并刷新派生缓存。
fn write(
    layer: &str,
    title: &str,
    text: Option<&str>,
    file: Option<&Path>,
    name: Option<&str>,
) -> Result<(), String> {
    let (store, context) = layer_store(layer)?;
    let target = match file {
        Some(source) => store
            .import(source, name)
            .map_err(|error| format!("knowledge write failed: {error}"))?,
        None => store
            .write_document(name, title, text.unwrap_or_default())
            .map_err(|error| format!("knowledge write failed: {error}"))?,
    };
    report_written(&store, &target, &context)
}

/// 刷新显式指定的层并写回派生缓存。
fn refresh(layer: Option<&str>) -> Result<(), String> {
    let context = Context::load()?;
    let scopes = match layer {
        None | Some("all") => vec![KnowledgeScope::User, KnowledgeScope::Project],
        Some("user") => vec![KnowledgeScope::User],
        Some("workspace") | Some("project") => vec![KnowledgeScope::Project],
        Some(_) => return Err("--layer must be user, workspace, or all".into()),
    };
    for scope in scopes {
        let Some(store) = context.store(scope)? else {
            println!("{}\tnot configured", scope.name());
            continue;
        };
        let documents = store.documents().map_err(error_text)?;
        let layer = load_layer(&store, &documents)?;
        store.save_cache(&documents, &layer).map_err(error_text)?;
        println!(
            "{}\t{}\tfiles {}\tbytes {}",
            scope.name(),
            store.root().display(),
            layer.files(),
            layer.bytes()
        );
    }
    Ok(())
}

/// 打开 `--layer` 指定的层；工作区层需要受信任工作区。
fn layer_store(layer: &str) -> Result<(KnowledgeStore, Context), String> {
    let context = Context::load()?;
    let scope =
        teal_knowledge::scope_for_layer(layer).ok_or("--layer must be user or workspace")?;
    let store = context
        .store(scope)?
        .ok_or_else(|| format!("the {} knowledge layer is not configured", scope.name()))?;
    Ok((store, context))
}

/// 刷新一层并打印被写入文档生成的条目身份。
fn report_written(store: &KnowledgeStore, target: &Path, context: &Context) -> Result<(), String> {
    let documents = store.documents().map_err(error_text)?;
    let layer = load_layer(store, &documents)?;
    store.save_cache(&documents, &layer).map_err(error_text)?;
    let relative = target
        .strip_prefix(store.root())
        .map_err(|_| "knowledge write escaped its layer root".to_owned())?
        .to_string_lossy()
        .replace('\\', "/");
    println!("{}\t{}", store.scope().name(), target.display());
    for record in layer
        .records()
        .iter()
        .filter(|record| record.source == relative)
    {
        println!("knowledge\tentry\t{}", record.id);
    }
    let _ = context;
    Ok(())
}

/// 读取一层文档，并在派生缓存一致时直接复用。
fn load_layer(
    store: &KnowledgeStore,
    documents: &[teal_knowledge::DocumentRef],
) -> Result<LayerIndex, String> {
    if let Some(layer) = store.cached(documents) {
        return Ok(layer);
    }
    store
        .build_from_map(&Default::default(), documents)
        .map_err(error_text)
}

/// 把规范化范围名映射到知识范围。
fn scope_of(scope: Option<&str>) -> Option<KnowledgeScope> {
    match scope {
        None | Some("all") => None,
        Some("official") => Some(KnowledgeScope::Official),
        Some("user") => Some(KnowledgeScope::User),
        Some("project") | Some("workspace") => Some(KnowledgeScope::Project),
        Some(_) => None,
    }
}

fn error_text(error: teal_knowledge::KnowledgeError) -> String {
    error.to_string()
}

/// 一次知识命令共用的配置上下文。
struct Context {
    config: teal_config::KnowledgeConfig,
    teal_home: PathBuf,
    workspace: PathBuf,
    trusted: bool,
}

impl Context {
    /// 从当前目录加载配置与信任决定。
    fn load() -> Result<Self, String> {
        let current = std::env::current_dir().map_err(|error| error.to_string())?;
        let effective =
            teal_config::load_declarations(Some(&current)).map_err(|error| error.to_string())?;
        Ok(Self {
            config: effective.knowledge().clone(),
            teal_home: effective.paths().teal_home.clone(),
            workspace: effective.paths().workspace.clone().unwrap_or(current),
            trusted: effective.workspace_trusted(),
        })
    }

    /// 返回内嵌官方目录。
    fn official(&self) -> Result<KnowledgeCatalog, String> {
        KnowledgeCatalog::embedded().map_err(error_text)
    }

    /// 返回层装配选项。
    fn options(&self) -> LayerOptions {
        LayerOptions {
            cache: self.config.cache,
            max_files: self.config.max_files,
            max_bytes: self.config.max_bytes,
        }
    }

    /// 是否允许装配本地层。
    fn enabled(&self) -> bool {
        self.config.enabled
    }

    /// 打开一层；未装配时返回 `None`。
    fn store(&self, scope: KnowledgeScope) -> Result<Option<KnowledgeStore>, String> {
        let root = match scope {
            KnowledgeScope::Official => return Ok(None),
            KnowledgeScope::User if self.enabled() => self.config.user_layer_root(&self.teal_home),
            KnowledgeScope::Project if self.enabled() && self.trusted => {
                self.config.workspace_layer_root(&self.workspace)
            }
            _ => return Ok(None),
        };
        KnowledgeStore::open(root, scope, self.options())
            .map(|store| store.with_audit_context(teal_security::process_context()))
            .map(Some)
            .map_err(error_text)
    }
}

/// 分层知识在运行期只读装配；供 `teal run` 复用。
///
/// # Errors
/// 配置不可读或层装配失败时返回错误。
pub(crate) fn runtime_layers() -> Result<Arc<LayeredKnowledge>, String> {
    layered(false).map(Arc::new)
}
