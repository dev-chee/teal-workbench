//! RuntimeStore 持久交互的工作台回答桥。

use teal_agent::{UserAnswer, UserQuestion};
use crate::workbench::ui::{Overlay, OverlayAction, PickerOverlay, MenuItem};
use teal_workbench_core::{InputEvent, Key};
use super::{App, PendingOverlay};

impl App {
    /// 仅提交信息，不恢复程序或打开模型；检查连接不得协调正在执行的操作。
    pub(super) fn open_durable_question(&mut self, arguments: &[String]) {
        let result = (|| -> Result<(), String> {
            if arguments.is_empty() || arguments.len() > 2 {
                return Err("Usage: :agent-answer <run-id> [request-id]".into());
            }
            if self.pending_overlay.is_some() {
                return Err(
                    "Close the current dialog before answering a persistent request".into(),
                );
            }
            let store = std::sync::Arc::new(
                teal_agent::RuntimeStore::connect(&self.workspace)
                    .map_err(|error| error.to_string())?,
            );
            let requests = store
                .interactions(&arguments[0])
                .map_err(|error| error.to_string())?
                .into_iter()
                .filter(|request| {
                    request.status == teal_agent::InteractionStatus::Pending
                        && arguments.get(1).is_none_or(|id| id == &request.id)
                })
                .collect::<Vec<_>>();
            if requests.len() != 1 {
                return Err(format!(
                    "Select exactly one pending request: {}",
                    requests
                        .iter()
                        .map(|request| request.id.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
            let request = requests.into_iter().next().expect("one request");
            request.question.validate()?;
            let title = format!("Run {} — Esc skip / Ctrl+C close", request.context.run_id);
            match &request.question {
                UserQuestion::AgentInput { prompt } | UserQuestion::Text { prompt } => {
                    self.overlays.push(QuestionText {
                        title,
                        prompt: prompt.clone(),
                        value: String::new(),
                    })
                }
                UserQuestion::Choice { prompt, choices } => {
                    self.overlays.push(PickerOverlay::with_items(
                        format!("{title}: {prompt}"),
                        choices
                            .iter()
                            .map(|choice| {
                                MenuItem::enabled(choice.id.clone(), choice.label.clone())
                            })
                            .collect(),
                    ))
                }
            }
            self.pending_overlay = Some(PendingOverlay::DurableQuestion { store, request });
            Ok(())
        })();
        if let Err(error) = result {
            self.message(super::Severity::Error, "agent", error);
        }
    }

    pub(super) fn answer_durable_question(
        &mut self,
        store: std::sync::Arc<teal_agent::RuntimeStore>,
        request: teal_agent::InteractionRequest,
        value: Option<String>,
    ) {
        let answer = match (value, &request.question) {
            (None, _) => UserAnswer::Skipped,
            (Some(value), UserQuestion::AgentInput { .. }) => UserAnswer::AgentInput {
                fields: std::collections::BTreeMap::from([
                    ("kind".into(), "text".into()),
                    ("value".into(), value),
                ]),
            },
            (Some(value), UserQuestion::Text { .. }) => UserAnswer::Text { value },
            (Some(id), UserQuestion::Choice { .. }) => UserAnswer::Selected { id },
        };
        match store.answer_interaction(
            &request.context.run_id,
            &request.id,
            request.context.generation,
            answer,
        ) {
            Ok(_) => self.message(
                super::Severity::Info,
                "agent",
                format!(
                    "Answer saved. Explicitly resume run {} with the required execution grants.",
                    request.context.run_id
                ),
            ),
            Err(error) => self.message(super::Severity::Error, "agent", error.to_string()),
        }
    }
}

/// 普通信息输入明确告知会发给模型；不借用秘密认证输入通道。
struct QuestionText {
    title: String,
    prompt: String,
    value: String,
}

impl Overlay for QuestionText {
    fn preferred_size(&self) -> (u16, u16) {
        (80, 12)
    }

    fn handle_input(&mut self, event: InputEvent) -> OverlayAction {
        match event.key {
            Key::Enter if !self.value.trim().is_empty() => {
                OverlayAction::Accepted(std::mem::take(&mut self.value))
            }
            Key::Escape => OverlayAction::Cancelled,
            Key::Backspace => {
                self.value.pop();
                OverlayAction::Consumed
            }
            Key::Char(character)
                if !event.modifiers.control
                    && !event.modifiers.alt
                    && !character.is_control()
                    && self.value.len() + character.len_utf8() <= 65536 =>
            {
                self.value.push(character);
                OverlayAction::Consumed
            }
            _ => OverlayAction::Consumed,
        }
    }

    fn render(&mut self, frame: &mut ratatui::Frame<'_>, area: ratatui::layout::Rect) {
        use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
        let width = area.width.min(80);
        let height = area.height.min(12);
        let popup = ratatui::layout::Rect::new(
            area.x + (area.width - width) / 2,
            area.y + (area.height - height) / 2,
            width,
            height,
        );
        frame.render_widget(Clear, popup);
        frame.render_widget(
            Paragraph::new(format!(
                "{}\nAnswer will be sent to the model.\n\n{}",
                self.prompt, self.value
            ))
            .wrap(Wrap { trim: false })
            .block(
                Block::default()
                    .title(self.title.as_str())
                    .borders(Borders::ALL),
            ),
            popup,
        );
    }
}
