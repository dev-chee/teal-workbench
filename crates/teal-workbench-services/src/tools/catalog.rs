//! 工作区操作的封闭身份和唯一 Host/Agent 元数据目录。
use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum WorkspaceOperation {
    WorkspaceReadFile,
    WorkspaceList,
    WorkspaceStat,
    WorkspaceReadRange,
    WorkspaceSearch,
    WorkspaceWriteFile,
    WorkspaceCreateDir,
    WorkspaceMove,
    WorkspaceDelete,
    WorkspaceReplaceFile,
    WorkspaceApplyPatch,
    WorkspaceApplyChangeset,
    WorkspaceRun,
    WorkspaceDiagnostics,
    TealCheck,
    TealFmtCheck,
    TealTest,
    TealRun,
    WorkspaceDiagnosticsJson,
}

struct OperationDefinition {
    module: &'static str,
    function: &'static str,
    name: &'static str,
    description: &'static str,
    parameters: &'static [(&'static str, &'static str)],
    arguments: &'static [RuntimeType],
    return_type: RuntimeType,
    capability: &'static str,
    effect: HostToolEffect,
    recovery: ExternalCallRecovery,
    checkpointed: bool,
    exported: bool,
}

impl WorkspaceOperation {
    const ALL: &'static [Self] = &[
        Self::WorkspaceReadFile,
        Self::WorkspaceList,
        Self::WorkspaceStat,
        Self::WorkspaceReadRange,
        Self::WorkspaceSearch,
        Self::WorkspaceWriteFile,
        Self::WorkspaceCreateDir,
        Self::WorkspaceMove,
        Self::WorkspaceDelete,
        Self::WorkspaceReplaceFile,
        Self::WorkspaceApplyPatch,
        Self::WorkspaceApplyChangeset,
        Self::WorkspaceRun,
        Self::WorkspaceDiagnostics,
        Self::TealCheck,
        Self::TealFmtCheck,
        Self::TealTest,
        Self::TealRun,
        Self::WorkspaceDiagnosticsJson,
    ];

    pub(super) fn name(self) -> &'static str {
        self.definition().name
    }

    fn definition(self) -> OperationDefinition {
        match self {
            Self::WorkspaceReadFile => OperationDefinition {
                module: "std.bench.workspace",
                function: "read_file",
                name: "workspace.read_file",
                description: "Read a UTF-8 encoded workspace file",
                parameters: &[("path", "Workspace-relative or absolute path")],
                arguments: &[RuntimeType::String],
                return_type: RuntimeType::String,
                capability: "editor.read",
                effect: HostToolEffect::ReadOnly,
                recovery: ExternalCallRecovery::Pure,
                checkpointed: false,
                exported: true,
            },
            Self::WorkspaceList => OperationDefinition {
                module: "std.bench.workspace",
                function: "list",
                name: "workspace.list",
                description: "List workspace directory entries with a bounded optional recursive traversal",
                parameters: &[
                    ("path", "Workspace directory path"),
                    ("recursive", "Whether to traverse descendants"),
                    ("limit", "Maximum entries from 1 through 5000"),
                ],
                arguments: &[RuntimeType::String, RuntimeType::Bool, RuntimeType::Int],
                return_type: RuntimeType::Array,
                capability: "editor.read",
                effect: HostToolEffect::ReadOnly,
                recovery: ExternalCallRecovery::Pure,
                checkpointed: false,
                exported: true,
            },
            Self::WorkspaceStat => OperationDefinition {
                module: "std.bench.workspace",
                function: "stat",
                name: "workspace.stat",
                description: "Read workspace path metadata without following the final symbolic link",
                parameters: &[("path", "Workspace path")],
                arguments: &[RuntimeType::String],
                return_type: RuntimeType::Map,
                capability: "editor.read",
                effect: HostToolEffect::ReadOnly,
                recovery: ExternalCallRecovery::Pure,
                checkpointed: false,
                exported: true,
            },
            Self::WorkspaceReadRange => OperationDefinition {
                module: "std.bench.workspace",
                function: "read_range",
                name: "workspace.read_range",
                description: "Read a bounded one-based line range from a UTF-8 workspace file",
                parameters: &[
                    ("path", "Workspace file path"),
                    ("start_line", "One-based first line"),
                    ("line_count", "Maximum number of lines"),
                ],
                arguments: &[RuntimeType::String, RuntimeType::Int, RuntimeType::Int],
                return_type: RuntimeType::String,
                capability: "editor.read",
                effect: HostToolEffect::ReadOnly,
                recovery: ExternalCallRecovery::Pure,
                checkpointed: false,
                exported: true,
            },
            Self::WorkspaceSearch => OperationDefinition {
                module: "std.bench.workspace",
                function: "search",
                name: "workspace.search",
                description: "Search workspace files using regular expressions",
                parameters: &[("query", "Regular expression")],
                arguments: &[RuntimeType::String],
                return_type: RuntimeType::String,
                capability: "editor.read",
                effect: HostToolEffect::ReadOnly,
                recovery: ExternalCallRecovery::Pure,
                checkpointed: false,
                exported: true,
            },
            Self::WorkspaceWriteFile => OperationDefinition {
                module: "std.bench.workspace",
                function: "write_file",
                name: "workspace.write_file",
                description: "Create or replace a workspace file and create a checkpoint",
                parameters: &[
                    ("path", "Workspace-relative or absolute path"),
                    ("content", "Complete UTF-8 file contents"),
                ],
                arguments: &[RuntimeType::String, RuntimeType::String],
                return_type: RuntimeType::Map,
                capability: "editor.write",
                effect: HostToolEffect::Mutating,
                recovery: ExternalCallRecovery::Journaled,
                checkpointed: true,
                exported: true,
            },
            Self::WorkspaceCreateDir => OperationDefinition {
                module: "std.bench.workspace",
                function: "create_dir",
                name: "workspace.create_dir",
                description: "Create a workspace directory and any missing parent directories",
                parameters: &[("path", "Workspace directory path")],
                arguments: &[RuntimeType::String],
                return_type: RuntimeType::String,
                capability: "editor.write",
                effect: HostToolEffect::Mutating,
                recovery: ExternalCallRecovery::Journaled,
                checkpointed: false,
                exported: true,
            },
            Self::WorkspaceMove => OperationDefinition {
                module: "std.bench.workspace",
                function: "move_path",
                name: "workspace.move",
                description: "Move a workspace path into an existing destination directory without overwriting",
                parameters: &[
                    ("source", "Existing workspace source path"),
                    ("destination", "Existing workspace destination directory"),
                ],
                arguments: &[RuntimeType::String, RuntimeType::String],
                return_type: RuntimeType::String,
                capability: "editor.write",
                effect: HostToolEffect::Mutating,
                recovery: ExternalCallRecovery::Journaled,
                checkpointed: false,
                exported: true,
            },
            Self::WorkspaceDelete => OperationDefinition {
                module: "std.bench.workspace",
                function: "delete_path",
                name: "workspace.delete",
                description: "Move a workspace path to the platform recycle bin",
                parameters: &[("path", "Existing workspace path")],
                arguments: &[RuntimeType::String],
                return_type: RuntimeType::String,
                capability: "editor.write",
                effect: HostToolEffect::Mutating,
                recovery: ExternalCallRecovery::Journaled,
                checkpointed: false,
                exported: true,
            },
            Self::WorkspaceReplaceFile => OperationDefinition {
                module: "std.bench.workspace",
                function: "replace_file",
                name: "workspace.replace_file",
                description: "Replace an exact match of text and create a checkpoint",
                parameters: &[
                    ("path", "Workspace-relative or absolute path"),
                    ("old", "Exact text to replace"),
                    ("new", "Replacement text"),
                ],
                arguments: &[
                    RuntimeType::String,
                    RuntimeType::String,
                    RuntimeType::String,
                ],
                return_type: RuntimeType::Map,
                capability: "editor.write",
                effect: HostToolEffect::Mutating,
                recovery: ExternalCallRecovery::Journaled,
                checkpointed: true,
                exported: true,
            },
            Self::WorkspaceApplyPatch => OperationDefinition {
                module: "std.bench.workspace",
                function: "apply_patch",
                name: "workspace.apply_patch",
                description: "Apply an exact multi-hunk unified diff to one versioned workspace file",
                parameters: &[
                    ("path", "Existing workspace text file"),
                    (
                        "expected_sha256",
                        "SHA-256 of the current normalized text; empty binds the current content",
                    ),
                    ("patch", "Unified diff hunks for this file"),
                ],
                arguments: &[
                    RuntimeType::String,
                    RuntimeType::String,
                    RuntimeType::String,
                ],
                return_type: RuntimeType::Map,
                capability: "editor.write",
                effect: HostToolEffect::Mutating,
                recovery: ExternalCallRecovery::Journaled,
                checkpointed: true,
                exported: true,
            },
            Self::WorkspaceApplyChangeset => OperationDefinition {
                module: "std.bench.workspace",
                function: "apply_changeset",
                name: "workspace.apply_changeset",
                description: "Review and apply a versioned multi-file ChangeSet",
                parameters: &[(
                    "changes",
                    "JSON object whose files array holds 1-64 path and unified diff patch entries; \
               optional expected_sha256 and new_sha256 are verified when present",
                )],
                arguments: &[RuntimeType::String],
                return_type: RuntimeType::String,
                capability: "editor.write",
                effect: HostToolEffect::Mutating,
                recovery: ExternalCallRecovery::Journaled,
                checkpointed: false,
                exported: true,
            },
            Self::WorkspaceRun => OperationDefinition {
                module: "std.bench.workspace",
                function: "run",
                name: "workspace.run",
                description: "Run a workspace command after user confirmation",
                parameters: &[("command", "Platform shell command")],
                arguments: &[RuntimeType::String],
                return_type: RuntimeType::String,
                capability: "process.spawn",
                effect: HostToolEffect::Mutating,
                recovery: ExternalCallRecovery::Journaled,
                checkpointed: false,
                exported: true,
            },
            Self::WorkspaceDiagnostics => OperationDefinition {
                module: "std.bench.workspace",
                function: "diagnostics",
                name: "workspace.diagnostics",
                description: "Read editor diagnostics; use an empty path for the whole workspace",
                parameters: &[("path", "Workspace path or an empty string")],
                arguments: &[RuntimeType::String],
                return_type: RuntimeType::Array,
                capability: "editor.read",
                effect: HostToolEffect::ReadOnly,
                recovery: ExternalCallRecovery::Pure,
                checkpointed: false,
                exported: true,
            },
            Self::TealCheck => OperationDefinition {
                module: "std.bench.toolchain",
                function: "check",
                name: "teal.check",
                description: "Check a workspace .tl file with the current Teal compiler",
                parameters: &[
                    ("path", "Workspace Teal source path"),
                    ("tier", "Teal runtime profile"),
                ],
                arguments: &[RuntimeType::String, RuntimeType::String],
                return_type: RuntimeType::Map,
                capability: "editor.read",
                effect: HostToolEffect::ReadOnly,
                recovery: ExternalCallRecovery::Pure,
                checkpointed: false,
                exported: true,
            },
            Self::TealFmtCheck => OperationDefinition {
                module: "std.bench.toolchain",
                function: "fmt_check",
                name: "teal.fmt_check",
                description: "Check a workspace .tl file's formatting without modifying it",
                parameters: &[("path", "Workspace Teal source path")],
                arguments: &[RuntimeType::String],
                return_type: RuntimeType::Map,
                capability: "editor.read",
                effect: HostToolEffect::ReadOnly,
                recovery: ExternalCallRecovery::Pure,
                checkpointed: false,
                exported: true,
            },
            Self::TealTest => OperationDefinition {
                module: "std.bench.toolchain",
                function: "test",
                name: "teal.test",
                description: "Run the `@test` functions registered in a workspace .tl file",
                parameters: &[
                    ("path", "Workspace Teal source path"),
                    ("tier", "Teal runtime profile"),
                    (
                        "filter",
                        "Optional test-name substring; empty runs every test",
                    ),
                ],
                arguments: &[
                    RuntimeType::String,
                    RuntimeType::String,
                    RuntimeType::String,
                ],
                return_type: RuntimeType::Map,
                capability: "process.spawn",
                effect: HostToolEffect::Mutating,
                recovery: ExternalCallRecovery::Journaled,
                checkpointed: false,
                exported: true,
            },
            Self::TealRun => OperationDefinition {
                module: "std.bench.toolchain",
                function: "run",
                name: "teal.run",
                description: "Run a Teal script with the current distribution after user confirmation",
                parameters: &[
                    ("path", "Workspace Teal source path"),
                    ("tier", "Teal runtime profile"),
                    ("permission_file", "Workspace-relative v3 permission file"),
                ],
                arguments: &[
                    RuntimeType::String,
                    RuntimeType::String,
                    RuntimeType::String,
                ],
                return_type: RuntimeType::Map,
                capability: "process.spawn",
                effect: HostToolEffect::Mutating,
                recovery: ExternalCallRecovery::Journaled,
                checkpointed: false,
                exported: true,
            },
            Self::WorkspaceDiagnosticsJson => OperationDefinition {
                module: "std.bench.workspace",
                function: "diagnostics_json",
                name: "workspace.diagnostics_json",
                description: "Read workspace diagnostics as JSON",
                parameters: &[("path", "Workspace path")],
                arguments: &[RuntimeType::String],
                return_type: RuntimeType::String,
                capability: "editor.read",
                effect: HostToolEffect::ReadOnly,
                recovery: ExternalCallRecovery::Pure,
                checkpointed: false,
                exported: false,
            },
        }
    }
}

pub(super) fn register_workspace_hosts(
    registry: &mut teal::HostRegistry,
    tools: Arc<WorkspaceTools>,
) -> Result<(), teal::HostRegistryError> {
    for &operation in WorkspaceOperation::ALL {
        let metadata = operation.definition();
        let invoked_tools = Arc::clone(&tools);
        let prepared_tools = Arc::clone(&tools);
        let mut definition = HostFunctionDefinition::blocking_with_context(
            metadata.module,
            metadata.function,
            teal::HostCallSignature::new(metadata.arguments.to_vec(), metadata.return_type)
                .expect("Static workspace Host signature is valid")
                .task(),
            metadata.capability,
            metadata.recovery,
            move |context, arguments| {
                let observation = context
                    .service::<tracing::Span>()
                    .ok()
                    .map(|span| crate::tracing_context::Context::with_span((*span).clone()));
                invoked_tools.invoke(operation, arguments, observation)
            },
        );
        if metadata.exported {
            definition = definition
                .with_preparer(move |arguments| prepared_tools.prepare(operation, arguments))
                .with_tool_export(HostToolExport {
                    name: metadata.name.to_owned(),
                    description: metadata.description.to_owned(),
                    parameters: metadata
                        .parameters
                        .iter()
                        .map(|(name, description)| HostToolParameter {
                            name: (*name).to_owned(),
                            description: (*description).to_owned(),
                        })
                        .collect(),
                    effect: metadata.effect,
                });
        }
        if metadata.checkpointed {
            definition = definition.with_tool_checkpoint(HostToolCheckpoint {
                value_field: "value".into(),
                checkpoint_field: "checkpoint".into(),
                value_type: RuntimeType::String,
            });
        }
        registry.register(definition)?;
    }
    super::media::register_image_host(registry, tools)?;
    Ok(())
}
