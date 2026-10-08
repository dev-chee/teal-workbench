//! 通用制品、结构化命令与精确 Git 操作。界面审批和 Runtime Host 账本控制实际执行。
use super::*;
use teal::{
    HostRegistry, HostCallSignature, HostFunctionDefinition, HostValue, RuntimeType,
    ExternalCallRecovery, HostError,
};
use teal_agent::{
    ActionContext, ActionResourceAccess, ActionSpec, Decision, PermissionProvider, ResourceVersion,
};
use serde::{Serialize, Deserialize};
use std::collections::BTreeSet;

#[derive(Clone)]
struct Context {
    workspace: PathBuf,
    instance: ExtensionInstance,
    config: BTreeMap<String, String>,
    permissions: Arc<teal_workbench_services::tools::ChannelApproval>,
    run: String,
}

pub(super) fn register_preflight(
    registry: &mut HostRegistry,
) -> Result<(), teal::HostRegistryError> {
    for (name, count, capability, task) in definitions() {
        let signature = signature(count);
        let definition = if task {
            HostFunctionDefinition::asynchronous(
                "std.bench.evidence",
                name,
                signature,
                capability,
                ExternalCallRecovery::Journaled,
                |_| {
                    Box::pin(async {
                        Err(host_error(
                            "Evidence operations require a prepared background task",
                        ))
                    })
                },
            )
        } else {
            HostFunctionDefinition::new(
                "std.bench.evidence",
                name,
                signature,
                capability,
                ExternalCallRecovery::Pure,
                |_| Err(host_error("Evidence reads require a task workspace")),
            )
        };
        registry.register(definition)?;
    }
    Ok(())
}

pub(super) fn register(
    registry: &mut HostRegistry,
    workspace: &Path,
    instance: &ExtensionInstance,
    config: &BTreeMap<String, String>,
    permissions: &Arc<teal_workbench_services::tools::ChannelApproval>,
    run: &str,
) -> Result<(), teal::HostRegistryError> {
    let context = Context {
        workspace: workspace.into(),
        instance: instance.clone(),
        config: config.clone(),
        permissions: permissions.clone(),
        run: run.into(),
    };
    for (name, count, capability, task) in definitions() {
        let context = context.clone();
        let definition = if task {
            HostFunctionDefinition::asynchronous(
                "std.bench.evidence",
                name,
                signature(count),
                capability,
                ExternalCallRecovery::Journaled,
                move |arguments| {
                    let c = context.clone();
                    Box::pin(async move {
                        c.invoke(name, arguments)
                            .await
                            .map(HostValue::String)
                            .map_err(|e| host_error(&e))
                    })
                },
            )
        } else {
            HostFunctionDefinition::new(
                "std.bench.evidence",
                name,
                signature(count),
                capability,
                ExternalCallRecovery::Pure,
                move |arguments| {
                    let paths: Vec<String> = serde_json::from_str(&string(arguments, 0)?)
                        .map_err(|e| host_error(&e.to_string()))?;
                    let result = paths
                        .iter()
                        .map(|p| artifact_receipt(&context.workspace, p))
                        .collect::<Result<Vec<_>, _>>()
                        .map_err(|e| host_error(&e))?;
                    serde_json::to_string(&result)
                        .map(HostValue::String)
                        .map_err(|e| host_error(&e.to_string()))
                },
            )
        };
        registry.register(definition)?;
    }
    Ok(())
}

fn definitions() -> [(&'static str, usize, &'static str, bool); 3] {
    teal_workbench_automation::extension::EVIDENCE_SIGNATURES
}
fn signature(count: usize) -> HostCallSignature {
    HostCallSignature::new(vec![RuntimeType::String; count], RuntimeType::String)
        .expect("Host strings")
}
fn string(values: &[HostValue], index: usize) -> Result<String, HostError> {
    match values.get(index) {
        Some(HostValue::String(v)) if v.len() <= 1024 * 1024 => Ok(v.clone()),
        _ => Err(host_error("expected bounded string")),
    }
}
fn host_error(message: &str) -> HostError {
    HostError::new(std::io::ErrorKind::PermissionDenied, message)
}

pub(super) fn validate_write_scope(
    workspace: &Path,
    instance: &ExtensionInstance,
    paths: &[String],
) -> Result<(), String> {
    let file = workspace
        .join(".teal/runtime")
        .join(format!("extension-git-{}.json", instance.id));
    let baseline: Vec<String> = if file.exists() {
        let saved: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(file).map_err(err)?).map_err(err)?;
        serde_json::from_value(saved["baseline"].clone()).map_err(err)?
    } else {
        vec![]
    };
    for path in paths {
        let normalized = artifact_receipt(workspace, path)?.path;
        if baseline.contains(&normalized) {
            return Err("Planned writes overlap pre-existing user changes".into());
        }
    }
    Ok(())
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Command {
    program: String,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    cwd: String,
    #[serde(default = "timeout")]
    timeout_ms: u64,
    #[serde(default)]
    inputs: Vec<String>,
    #[serde(default)]
    operation: String,
    #[serde(default)]
    environment: String,
}
fn timeout() -> u64 {
    600_000
}

impl Context {
    async fn authorize(&self, tool: &str, arguments: serde_json::Value) -> Result<(), String> {
        self.authorize_with_resources(tool, arguments, Vec::new())
            .await
    }

    async fn authorize_with_resources(
        &self,
        tool: &str,
        arguments: serde_json::Value,
        mut resources: Vec<ResourceVersion>,
    ) -> Result<(), String> {
        let id = format!("{}:{}", self.run, sha256(arguments.to_string().as_bytes()));
        resources.extend(permission_resources(tool, &arguments, &self.workspace)?);
        match self
            .permissions
            .authorize(
                ActionContext {
                    run_id: self.run.clone(),
                    task_id: self.instance.id.clone(),
                    generation: self.instance.revision,
                    policy_version: self.permissions.version().into(),
                },
                ActionSpec {
                    id,
                    tool: tool.into(),
                    arguments,
                    resources,
                },
            )
            .await
            .map_err(err)?
        {
            Decision::Allow => Ok(()),
            _ => Err("User did not authorize the operation".into()),
        }
    }
    fn approval(
        &self,
        id: &str,
        operation: &str,
        environment: &str,
    ) -> Result<&teal_workbench_automation::extension::ApprovalReceipt, String> {
        let r = self
            .instance
            .approvals
            .get(id)
            .ok_or("Approval receipt is missing")?;
        let operation_matches =
            r.operation == operation || (operation == "commit" && r.operation == "plan");
        if r.approved != Some(true) || !operation_matches || r.environment != environment {
            return Err("Approval does not match the operation".into());
        }
        for a in &r.artifacts {
            if artifact_receipt(&self.workspace, &a.path)? != *a {
                return Err("Approved artifacts changed".into());
            }
        }
        Ok(r)
    }
    async fn invoke(&self, name: &str, args: Vec<HostValue>) -> Result<String, String> {
        match name {
            "command" => {
                self.command(
                    &string(&args, 0).map_err(err)?,
                    &string(&args, 1).map_err(err)?,
                )
                .await
            }
            "git" => {
                self.git(
                    &string(&args, 0).map_err(err)?,
                    &string(&args, 1).map_err(err)?,
                    &string(&args, 2).map_err(err)?,
                    &string(&args, 3).map_err(err)?,
                )
                .await
            }
            _ => Err("Unknown evidence operation".into()),
        }
    }
    async fn command(&self, name: &str, approval: &str) -> Result<String, String> {
        let encoded = self
            .config
            .get(&format!("command.{name}"))
            .ok_or("Named command is not configured")?;
        let command: Command = serde_json::from_str(encoded).map_err(err)?;
        if command.program.is_empty() || command.timeout_ms == 0 || command.timeout_ms > 3_600_000 {
            return Err("Invalid command program or timeout".into());
        }
        if !command.operation.is_empty() {
            self.approval(approval, &command.operation, &command.environment)?;
        }
        let cwd = self
            .workspace
            .join(&command.cwd)
            .canonicalize()
            .map_err(err)?;
        if !cwd.starts_with(self.workspace.canonicalize().map_err(err)?) {
            return Err("Command working directory escapes the workspace".into());
        }
        let mut input_paths = if command.inputs.is_empty() {
            source_paths(&self.workspace).await?
        } else {
            command.inputs.clone()
        };
        if command.inputs.is_empty() {
            // 已审批制品所在目录保存流程报告，不属于被测试的业务源码；报告更新不使代码测试失效。
            let artifact_roots = self
                .instance
                .approvals
                .values()
                .filter(|r| r.approved == Some(true))
                .flat_map(|r| r.artifacts.iter())
                .filter_map(|a| Path::new(&a.path).parent())
                .filter(|p| !p.as_os_str().is_empty())
                .collect::<Vec<_>>();
            input_paths.retain(|path| {
                !artifact_roots
                    .iter()
                    .any(|root| Path::new(path).starts_with(root))
            });
        }
        let before = input_paths
            .iter()
            .map(|p| artifact_receipt(&self.workspace, p))
            .collect::<Result<Vec<_>, _>>()?;
        let request = serde_json::to_value(&command).map_err(err)?;
        let input_resources = before
            .iter()
            .map(|artifact| ResourceVersion {
                access: ActionResourceAccess::Read,
                resource: format!(
                    "file://{}",
                    self.workspace
                        .join(&artifact.path)
                        .to_string_lossy()
                        .replace('\\', "/")
                ),
                version: artifact.sha256.clone(),
            })
            .collect();
        self.authorize_with_resources("extension.command", request.clone(), input_resources)
            .await?;
        if !command.operation.is_empty() {
            self.approval(approval, &command.operation, &command.environment)?;
        }
        for a in &before {
            if artifact_receipt(&self.workspace, &a.path)? != *a {
                return Err("Command input changed while awaiting approval".into());
            }
        }
        let output = run(&command.program, &command.args, &cwd, command.timeout_ms).await?;
        for a in &before {
            if artifact_receipt(&self.workspace, &a.path)? != *a {
                return Err("Command input changed during verification".into());
            }
        }
        let evidence_digest = sha256(
            serde_json::to_string(&(&request, &before, &output.output, output.status))
                .map_err(err)?
                .as_bytes(),
        );
        let receipt = teal_workbench_automation::extension::CommandReceipt {
            id: format!("{}:{name}:{evidence_digest}", self.run),
            command_digest: sha256(request.to_string().as_bytes()),
            exit_code: Some(output.status),
            output_digest: sha256(output.output.as_bytes()),
            artifacts: before,
        };
        let directory = self.workspace.join(".teal/runtime/extension-evidence");
        std::fs::create_dir_all(&directory).map_err(err)?;
        let record = serde_json::json!({"name":name,"command":command,"receipt":receipt});
        std::fs::write(
            directory.join(format!("{}.json", sha256(receipt.id.as_bytes()))),
            record.to_string(),
        )
        .map_err(err)?;
        Ok(serde_json::json!({"receipt":receipt,"output":output.output,"success":output.status==0}).to_string())
    }

    async fn git(
        &self,
        action: &str,
        paths_json: &str,
        message: &str,
        approval_id: &str,
    ) -> Result<String, String> {
        static LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
        let _lock = LOCK.lock().await;
        let request: serde_json::Value = serde_json::from_str(paths_json).map_err(err)?;
        let paths: Vec<String> = serde_json::from_value(if action == "commit_verified" {
            request["paths"].clone()
        } else {
            request.clone()
        })
        .map_err(err)?;
        let session_path = self
            .workspace
            .join(".teal/runtime")
            .join(format!("extension-git-{}.json", self.instance.id));
        match action {
            "begin" => {
                if paths.len() != 1 || !paths[0].starts_with("codex/") {
                    return Err("A codex/ branch name is required".into());
                }
                if session_path.exists() {
                    let saved = std::fs::read_to_string(&session_path).map_err(err)?;
                    let parsed: serde_json::Value = serde_json::from_str(&saved).map_err(err)?;
                    if parsed["branch"].as_str() == Some(paths[0].as_str()) {
                        return Ok(saved);
                    }
                }
                checked_git(
                    &self.workspace,
                    &["check-ref-format", "--branch", &paths[0]],
                )
                .await?;
                let baseline = changed_paths(&self.workspace).await?;
                self.authorize(
                    "extension.git",
                    serde_json::json!({"action":"begin","branch":paths[0]}),
                )
                .await?;
                checked_git(&self.workspace, &["switch", "-c", &paths[0]]).await?;
                let value = serde_json::json!({"branch":paths[0],"baseline":baseline}).to_string();
                std::fs::write(session_path, &value).map_err(err)?;
                Ok(value)
            }
            "commit" | "commit_verified" => {
                if paths.is_empty() || message.trim().is_empty() {
                    return Err("Commit requires paths and a message".into());
                }
                let r = self.approval(approval_id, "commit", "")?;
                let approved = r
                    .artifacts
                    .iter()
                    .map(|a| a.path.clone())
                    .collect::<BTreeSet<_>>();
                let actual = paths
                    .iter()
                    .map(|p| artifact_receipt(&self.workspace, p).map(|r| r.path))
                    .collect::<Result<BTreeSet<_>, _>>()?;
                if action == "commit" && actual != approved {
                    return Err("Commit paths do not match approved artifacts".into());
                }
                if action == "commit_verified" {
                    self.verify_commit_evidence(&request, &actual, r)?;
                }
                let session: serde_json::Value =
                    serde_json::from_str(&std::fs::read_to_string(&session_path).map_err(err)?)
                        .map_err(err)?;
                let baseline: Vec<String> =
                    serde_json::from_value(session["baseline"].clone()).map_err(err)?;
                if actual.iter().any(|p| baseline.contains(p)) {
                    return Err("Commit overlaps pre-existing user changes".into());
                }
                let branch = checked_git(&self.workspace, &["branch", "--show-current"]).await?;
                if Some(branch.trim()) != session["branch"].as_str() {
                    return Err("Working branch changed".into());
                }
                if !checked_git(&self.workspace, &["diff", "--cached", "--name-only"])
                    .await?
                    .trim()
                    .is_empty()
                {
                    return Err("User index is not empty; automatic commit is blocked".into());
                }
                if action == "commit" {
                    self.authorize("extension.git",serde_json::json!({"action":"commit","paths":actual,"message":message,"approval":approval_id})).await?;
                }
                self.approval(approval_id, "commit", "")?;
                if action == "commit_verified" {
                    self.verify_commit_evidence(&request, &actual, r)?;
                }
                let mut args = vec!["add".into(), "--".into()];
                args.extend(actual.iter().cloned());
                let output = run("git", &args, &self.workspace, timeout()).await?;
                if output.status != 0 {
                    return Err(output.output);
                }
                let staged =
                    checked_git(&self.workspace, &["diff", "--cached", "--name-only", "-z"])
                        .await?
                        .split('\0')
                        .filter(|p| !p.is_empty())
                        .map(str::to_owned)
                        .collect::<BTreeSet<_>>();
                if !staged.is_subset(&actual) {
                    return Err("Index contains unrelated changes; commit blocked".into());
                }
                if staged.is_empty() {
                    return Err("No task changes to commit".into());
                }
                self.approval(approval_id, "commit", "")?;
                checked_git(&self.workspace, &["diff", "--cached", "--check"]).await?;
                let artifacts = actual
                    .iter()
                    .map(|path| artifact_receipt(&self.workspace, path))
                    .collect::<Result<Vec<_>, _>>()?;
                // Git 自身也限定提交范围，避免检查后新加入索引的无关文件被顺带提交。
                let mut commit_args = vec![
                    "commit".to_owned(),
                    "--only".into(),
                    "-m".into(),
                    message.into(),
                    "--".into(),
                ];
                commit_args.extend(actual.iter().cloned());
                let committed = run("git", &commit_args, &self.workspace, timeout()).await?;
                if committed.status != 0 {
                    return Err(committed.output);
                }
                let commit = checked_git(&self.workspace, &["rev-parse", "HEAD"]).await?;
                for artifact in &artifacts {
                    if artifact_receipt(&self.workspace, &artifact.path)? != *artifact {
                        return Err(format!(
                            "Commit {} exists, but artifacts changed; inspect before continuing",
                            commit.trim()
                        ));
                    }
                }
                Ok(serde_json::json!({"commit":commit.trim(),"artifacts":artifacts,"approval":approval_id}).to_string())
            }
            "status" => {
                Ok(serde_json::to_string(&changed_paths(&self.workspace).await?).map_err(err)?)
            }
            _ => Err("Supported Git operations: begin, commit, commit_verified, status".into()),
        }
    }

    fn verify_commit_evidence(
        &self,
        request: &serde_json::Value,
        paths: &BTreeSet<String>,
        approval: &teal_workbench_automation::extension::ApprovalReceipt,
    ) -> Result<(), String> {
        let ids: Vec<String> = serde_json::from_value(request["checks"].clone()).map_err(err)?;
        let required: Vec<String> = serde_json::from_str(
            self.config
                .get("checks")
                .ok_or("Required checks are not configured")?,
        )
        .map_err(err)?;
        if required.is_empty() || ids.len() != required.len() {
            return Err("All configured checks must have receipts".into());
        }
        let roots = approval
            .artifacts
            .iter()
            .filter_map(|a| Path::new(&a.path).parent())
            .filter(|p| !p.as_os_str().is_empty())
            .collect::<Vec<_>>();
        let mut found = BTreeSet::new();
        for id in ids {
            let file = self
                .workspace
                .join(".teal/runtime/extension-evidence")
                .join(format!("{}.json", sha256(id.as_bytes())));
            let record: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(file).map_err(err)?).map_err(err)?;
            let receipt: teal_workbench_automation::extension::CommandReceipt =
                serde_json::from_value(record["receipt"].clone()).map_err(err)?;
            let name = record["name"].as_str().ok_or("Invalid check name")?;
            let configured: serde_json::Value = serde_json::from_str(
                self.config
                    .get(&format!("command.{name}"))
                    .ok_or("Check configuration changed")?,
            )
            .map_err(err)?;
            let current: Command = serde_json::from_value(configured).map_err(err)?;
            if receipt.id != id
                || receipt.exit_code != Some(0)
                || serde_json::to_value(current).map_err(err)? != record["command"]
                || !found.insert(name.to_owned())
            {
                return Err("Check receipt is invalid or stale".into());
            }
            for a in &receipt.artifacts {
                if artifact_receipt(&self.workspace, &a.path)? != *a {
                    return Err("Verified source changed".into());
                }
            }
            for path in paths {
                if !roots.iter().any(|root| Path::new(path).starts_with(root))
                    && !receipt.artifacts.iter().any(|a| &a.path == path)
                {
                    return Err("Commit contains an unverified source file".into());
                }
            }
        }
        if found != required.into_iter().collect() {
            return Err("Required checks do not match evidence".into());
        }
        Ok(())
    }
}

fn permission_resources(
    tool: &str,
    arguments: &serde_json::Value,
    workspace: &Path,
) -> Result<Vec<ResourceVersion>, String> {
    let mut resources = Vec::new();
    if tool == "extension.command" {
        let command: Command = serde_json::from_value(arguments.clone()).map_err(err)?;
        let cwd = workspace.join(&command.cwd);
        resources.push(path_resource(&cwd, ActionResourceAccess::Execute)?);
        resources.push(ResourceVersion {
            access: ActionResourceAccess::Execute,
            resource: format!("application://named-command/{}", command.program),
            version: sha256(arguments.to_string().as_bytes()),
        });
    } else if tool == "extension.git" {
        if let Some(paths) = arguments.get("paths").and_then(serde_json::Value::as_array) {
            for path in paths.iter().filter_map(serde_json::Value::as_str) {
                resources.push(path_resource(
                    &workspace.join(path),
                    ActionResourceAccess::Write,
                )?);
            }
        }
        resources.push(ResourceVersion {
            access: ActionResourceAccess::Execute,
            resource: "application://git/workspace".into(),
            version: String::new(),
        });
    }
    Ok(resources)
}

fn path_resource(path: &Path, access: ActionResourceAccess) -> Result<ResourceVersion, String> {
    let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let version = match std::fs::metadata(&path) {
        Ok(metadata) if metadata.is_file() => sha256(&std::fs::read(&path).map_err(err)?),
        Ok(metadata) if metadata.is_dir() => format!("directory:{}", metadata.len()),
        Ok(metadata) => format!("other:{}", metadata.len()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => "absent".into(),
        Err(error) => return Err(error.to_string()),
    };
    Ok(ResourceVersion {
        access,
        resource: format!("file://{}", path.to_string_lossy().replace('\\', "/")),
        version,
    })
}

async fn run(
    program: &str,
    args: &[String],
    cwd: &Path,
    timeout_ms: u64,
) -> Result<teal_workbench_services::ProcessOutput, String> {
    tokio::time::timeout(
        std::time::Duration::from_millis(timeout_ms),
        teal_workbench_services::run_program(Path::new(program), args, cwd),
    )
    .await
    .map_err(|_| "Command timed out; inspect the external result before retrying".to_owned())?
    .map_err(err)
}
async fn checked_git(root: &Path, args: &[&str]) -> Result<String, String> {
    let output = run(
        "git",
        &args.iter().map(|s| (*s).into()).collect::<Vec<_>>(),
        root,
        timeout(),
    )
    .await?;
    if output.status != 0 {
        return Err(output.output);
    }
    Ok(output.output)
}
async fn changed_paths(root: &Path) -> Result<Vec<String>, String> {
    let text = checked_git(
        root,
        &["status", "--porcelain=v1", "-z", "--untracked-files=all"],
    )
    .await?;
    let mut result = BTreeSet::new();
    let mut parts = text.split('\0');
    while let Some(record) = parts.next() {
        if record.len() < 4 {
            continue;
        }
        let path = &record[3..];
        if !path.starts_with(".teal/runtime/") {
            result.insert(path.to_owned());
        }
        if (record[..2].contains('R') || record[..2].contains('C'))
            && let Some(path) = parts.next()
        {
            result.insert(path.into());
        }
    }
    Ok(result.into_iter().collect())
}
async fn source_paths(root: &Path) -> Result<Vec<String>, String> {
    let tracked = checked_git(
        root,
        &[
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
        ],
    )
    .await?;
    let mut result = BTreeSet::new();
    for path in tracked
        .split('\0')
        .filter(|p| !p.is_empty() && !p.starts_with(".teal/runtime/"))
    {
        if !root.join(path).is_dir() {
            result.insert(path.into());
        }
    }
    if result.len() > 16384 {
        return Err("Verification input exceeds 16384 files; configure explicit inputs".into());
    }
    Ok(result.into_iter().collect())
}
