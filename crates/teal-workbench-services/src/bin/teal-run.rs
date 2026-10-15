use std::sync::Arc;

use teal_agent::{
    ActionContext, ActionSpec, Decision, PermissionProvider, PolicyError, ProviderFuture,
};

fn main() {
    let args = std::env::args_os().skip(1).collect::<Vec<_>>();
    if args
        .iter()
        .any(|arg| matches!(arg.to_str(), Some("--help" | "-h" | "--version" | "-V")))
    {
        std::process::exit(teal::cli::run_program(args));
    }
    let knowledge = match knowledge_provider() {
        Ok(knowledge) => knowledge,
        Err(error) => {
            eprintln!("cannot initialize knowledge services: {error}");
            std::process::exit(1);
        }
    };
    let result = std::thread::Builder::new()
        .name("teal-run".into())
        .stack_size(8 * 1024 * 1024)
        .spawn(move || teal::cli::run_program_with_knowledge(args, knowledge))
        .and_then(|worker| {
            worker
                .join()
                .map_err(|_| std::io::Error::other("teal-run worker panicked"))
        })
        .unwrap_or(1);
    std::process::exit(result);
}

fn knowledge_provider() -> Result<Arc<dyn teal_agent::KnowledgeProvider>, String> {
    let workspace = std::env::current_dir().map_err(|error| error.to_string())?;
    let effective = teal_security::load(Some(&workspace)).map_err(|error| error.to_string())?;
    let resolved = effective
        .category::<teal_security::TealConfig>()
        .map_err(|error| error.to_string())?;
    let permissions = teal_security::workspace_permissions(effective.paths(), resolved.document());
    let config = resolved.document().knowledge.clone();
    let options = teal_knowledge::LayerOptions {
        cache: config.cache,
        max_files: config.max_files,
        max_bytes: config.max_bytes,
    };
    let user = if config.enabled {
        Some(config.user_scope_root(&effective.paths().teal_home))
    } else {
        None
    };
    let project =
        if config.enabled && (permissions.trust_level == teal_security::TrustLevel::Trusted) {
            Some(config.workspace_scope_root(&workspace))
        } else {
            None
        };
    let mut knowledge = teal_workbench_services::WorkbenchKnowledge::official()
        .map_err(|error| error.to_string())?
        .with_layers(user, project, options)
        .map_err(|error| error.to_string())?
        .with_inline(config.inject_inline);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| error.to_string())?;
    runtime
        .block_on(knowledge.refresh(
            &AllowKnowledge,
            ActionContext {
                run_id: "teal-run/knowledge".into(),
                task_id: "knowledge".into(),
                generation: 0,
                policy_version: "teal-run/knowledge-v1".into(),
            },
            false,
        ))
        .map_err(|error| error.to_string())?;
    Ok(Arc::new(knowledge))
}

struct AllowKnowledge;

impl PermissionProvider for AllowKnowledge {
    fn version(&self) -> &str {
        "teal-run/knowledge-v1"
    }

    fn authorize(
        &self,
        _context: ActionContext,
        _action: ActionSpec,
    ) -> ProviderFuture<Result<Decision, PolicyError>> {
        Box::pin(async { Ok(Decision::Allow) })
    }
}
