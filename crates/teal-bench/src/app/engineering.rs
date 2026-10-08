//! 分支对话和三方合并的终端工作流装配。

use std::path::PathBuf;

use similar::TextDiff;
use tokio::sync::oneshot;
use teal_workbench_services::agent_algorithms::{ThreeWayMerge};

use super::{App, AppPanel, SplitAxis, EditorPanel, PendingMergeReview, Severity};
use crate::{presentation::AgentBlock, runtime_bridge::Context};
use teal_workbench_services::tools::ChangeSetRequest;

impl App {
    pub(super) fn agent_branch_command(&mut self, arguments: &[String]) {
        let input = format!(":agent-branch {}", arguments.join(" "))
            .trim()
            .to_owned();
        self.submit_source_agent_input(input);
    }

    pub(super) fn start_three_way_merge(&mut self, arguments: &[String]) {
        let [base, ours, theirs] = arguments else {
            self.message(
                Severity::Error,
                "agent",
                "Usage: :agent-merge <base> <ours> <theirs>",
            );
            return;
        };
        let (base_path, base) = match self.read_merge_input(base) {
            Ok(value) => value,
            Err(error) => {
                self.message(Severity::Error, "agent", error);
                return;
            }
        };
        let (ours_path, ours) = match self.read_merge_input(ours) {
            Ok(value) => value,
            Err(error) => {
                self.message(Severity::Error, "agent", error);
                return;
            }
        };
        let (theirs_path, theirs) = match self.read_merge_input(theirs) {
            Ok(value) => value,
            Err(error) => {
                self.message(Severity::Error, "agent", error);
                return;
            }
        };
        let target_relative = match ours_path.strip_prefix(&self.workspace) {
            Ok(path) => path.to_path_buf(),
            Err(_) => {
                self.message(
                    Severity::Error,
                    "agent",
                    "Merge target is outside the workspace",
                );
                return;
            }
        };
        let plan = ThreeWayMerge::new(&base, &ours, &theirs);
        let mut editor = EditorPanel::new(
            teal_workbench_core::Document::from_text(&plan.merged),
            self.keymap,
        );
        if let Some(extension) = ours_path.extension().and_then(|value| value.to_str())
            && let Some(grammar) = self.grammars.get(extension)
        {
            let _ = editor.set_grammar(grammar);
        }
        let panel = self.add_panel(editor, SplitAxis::Columns);
        let buffer = self
            .panels
            .get(panel)
            .and_then(AppPanel::as_editor)
            .expect("three-way merge panel is an editor")
            .document()
            .id();
        self.pending_merge = Some(PendingMergeReview {
            base_path,
            ours_path,
            theirs_path,
            target_relative: target_relative.clone(),
            plan: plan.clone(),
            buffer,
        });
        let agent = self.ensure_agent_panel();
        if let Some(output) = self
            .panels
            .get_mut(agent)
            .and_then(|panel| panel.as_text_mut())
        {
            output.push_agent_block(AgentBlock::Merge {
                target: target_relative.display().to_string(),
                conflicts: plan.conflicts.len(),
                input_digest: plan.input_digest,
            });
        }
        self.message(
            Severity::Info,
            "agent",
            format!(
                "Three-way merge opened with {} conflict(s); edit it and run :agent-merge-verify",
                plan.conflicts.len()
            ),
        );
    }

    pub(super) fn verify_three_way_merge(&mut self) {
        let Some(pending) = self.pending_merge.take() else {
            self.message(Severity::Warning, "agent", "No three-way merge is pending");
            return;
        };
        let Some(resolution) = self
            .buffers
            .get(pending.buffer)
            .map(|buffer| buffer.to_string())
        else {
            self.message(
                Severity::Error,
                "agent",
                "Merge buffer is no longer available",
            );
            return;
        };
        let current = [
            self.current_merge_text(&pending.base_path),
            self.current_merge_text(&pending.ours_path),
            self.current_merge_text(&pending.theirs_path),
        ];
        let [Ok(base), Ok(ours), Ok(theirs)] = current else {
            self.message(
                Severity::Error,
                "agent",
                "Unable to reread three-way merge inputs",
            );
            return;
        };
        let resolution = match pending
            .plan
            .validate_resolution(&base, &ours, &theirs, resolution)
        {
            Ok(resolution) => resolution.text,
            Err(error) => {
                self.pending_merge = Some(pending);
                self.message(Severity::Error, "agent", error.to_string());
                return;
            }
        };
        if normalize(&resolution) == normalize(&ours) {
            self.message(
                Severity::Info,
                "agent",
                "Merge resolution does not change the target",
            );
            return;
        }
        let original = normalize(&ours);
        let replacement = normalize(&resolution);
        let display = pending.target_relative.to_string_lossy().replace('\\', "/");
        let diff = TextDiff::from_lines(&original, &replacement)
            .unified_diff()
            .header(&format!("a/{display}"), &format!("b/{display}"))
            .to_string();
        let file = match teal_workbench_core::ChangeSetFile::new(
            pending.target_relative,
            teal_utils::sha256_hex(original.as_bytes()),
            teal_utils::sha256_hex(replacement.as_bytes()),
            diff,
        ) {
            Ok(file) => file,
            Err(error) => {
                self.message(Severity::Error, "agent", error.to_string());
                return;
            }
        };
        let change_set =
            match teal_workbench_core::ChangeSet::new(None, self.agent_generation, vec![file]) {
                Ok(change_set) => change_set,
                Err(error) => {
                    self.message(Severity::Error, "agent", error.to_string());
                    return;
                }
            };
        let (response, _ignored) = oneshot::channel();
        self.show_agent_changeset(ChangeSetRequest {
            observation: Context::current(),
            change_set,
            response,
        });
    }

    fn read_merge_input(&self, requested: &str) -> Result<(PathBuf, String), String> {
        let path = PathBuf::from(requested);
        let path = if path.is_absolute() {
            path
        } else {
            self.workspace.join(path)
        };
        let path = path
            .canonicalize()
            .map_err(|error| format!("Unable to resolve merge input: {error}"))?;
        if !path.starts_with(&self.workspace) || !path.is_file() {
            return Err("Merge input must be a regular file inside the workspace".into());
        }
        let text = self.current_merge_text(&path)?;
        Ok((path, text))
    }

    fn current_merge_text(&self, path: &PathBuf) -> Result<String, String> {
        self.buffers.find(path).map_or_else(
            || {
                std::fs::read_to_string(path)
                    .map_err(|error| format!("Unable to read merge input: {error}"))
            },
            |buffer| Ok(buffer.to_string()),
        )
    }
}

fn normalize(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}
