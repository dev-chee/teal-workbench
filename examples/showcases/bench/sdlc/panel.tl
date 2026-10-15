@edition("2026");

// Cross-tier Showcase：面板渲染状态事实并把交互事件送回工作流。
import "bench/extension.tl" as ext;
import "bench/ui.tl" as ui;
import "flow.tl" as flow;
fn application() -> none | Error {
    if ext::event("action") ? != "" {
        flow::dispatch(ext::event("action") ?, ext::event("input") ?) ?;
    }
    match ui::event() ? {
        ui::UiEvent.change { id, value } => {
            if id == "input" {
                ext::set("draft_input", value) ?;
            }
        }
        ui::UiEvent.activate { id } => {
            if id == "send" {
                flow::dispatch("input", ext::get("draft_input") ?) ?;
            }
        } _ => {}
    }
    ui::render(ui::node("sdlc", ui::Component.column {
        children: [ui::node("heading", ui::Component.text { text: "AI-native SDLC", style: ui::Style.heading, wrap: true }), ui::node("stage", ui::Component.text { text: "Stage: " + ext::get("stage") ?, style: ui::Style.accent, wrap: true }), ui::node("status", ui::Component.text { text: ext::get("status") ?, style: ui::Style.normal, wrap: true }), ui::node("artifact", ui::Component.text { text: "Artifact: " + ext::get("artifact") ?, style: ui::Style.code, wrap: true }), ui::node("activity", ui::Component.text { text: ext::event("activity") ?, style: ui::Style.muted, wrap: true }), ui::node("input", ui::Component.input { value: ext::get("draft_input") ?, placeholder: "Goal, revision instructions, or import-alert evidence" }), ui::node("send", ui::Component.button { label: "Send", enabled: ext::get("busy") ? != "true" })],
        gap: 1
    })) ?;
}

/// 入口明确处理失败；未知外部效果仍由 VM 终止。
fn main() -> int {
    let result = application();
    match result {
        Error { .. } => {
            print(result.message);
            return 1;
        } _ => {
            return 0;
        }
    }
}
