@edition("2026");

// Tier Core：构造 std/ui.tl 的全部公开组件、尺寸、样式和事件变体。
// 标准库只负责类型与 JSON 编解码；实际渲染和输入由工作台等 Host 完成。
import "std/ui.tl" as ui;

task application() -> none | Error {
    let styles = [ui::Style.normal, ui::Style.muted, ui::Style.accent, ui::Style.success, ui::Style.warning, ui::Style.error, ui::Style.code, ui::Style.heading,];
    let branch = ui::TreeItem { id: "child", label: "Child", expanded: false, children: [] };
    let tree = ui::TreeItem { id: "root", label: "Root", expanded: true, children: [branch] };
    let nodes = [ui::node("row", ui::Component.row { children: [], gap: 1 }), ui::node("column", ui::Component.column { children: [], gap: 1 }), ui::node("grid", ui::Component.grid { children: [], columns: 2, gap: 1 }), ui::node("text", ui::Component.text { text: "Text", style: ui::Style.normal, wrap: true }), ui::node("button", ui::Component.button { label: "Run", enabled: true }), ui::node("input", ui::Component.input { value: "", placeholder: "Name" }), ui::node("list", ui::Component.list { items: ["a", "b"], selected: 0 }), ui::node("table", ui::Component.table { headers: ["Name"], rows: [["Teal"]], selected: 0 }), ui::node("tree", ui::Component.tree { items: [tree], selected: "child" }), ui::node("progress", ui::Component.progress { value: 1, maximum: 2, label: "Half" }), ui::node("spacer", ui::Component.spacer { cells: 1 }),];

    let sized = ui::Node {
        id: "sized",
        width: ui::Length.cells { value: 20 },
        height: ui::Length.percent { value: 50 },
        content: ui::Component.column { children: nodes, gap: 0 },
    };
    let filled = ui::Node {
        id: "filled",
        width: ui::Length.fill { weight: 2 },
        height: ui::Length.auto,
        content: ui::Component.spacer { cells: 0 },
    };

    let events = [ui::event({ "kind": "mount" }) ?, ui::event({ "kind": "focus", "id": "name" }) ?, ui::event({ "kind": "key", "id": "name", "key": "Enter", "control": "false", "alt": "false", "shift": "false" }) ?, ui::event({ "kind": "activate", "id": "run" }) ?, ui::event({ "kind": "change", "id": "name", "value": "Teal" }) ?, ui::event({ "kind": "selection", "id": "list", "index": "1", "key": "b" }) ?, ui::event({ "kind": "resize", "width": "120", "height": "40" }) ?, ui::event({ "kind": "tick", "elapsed_ms": "16" }) ?,];

    print(styles.len(), events.len(), ui::quote("line\n\t\"quoted\"") ?);
    print(ui::encode(sized) ?.contains("\"kind\":\"tree\""));
    print(ui::encode(filled) ?.contains("\"fill\":2"));
}

/// 入口明确处理失败；未知外部效果仍由 VM 终止。
task main() -> int {
    let result = application().await;
    match result {
        Error { .. } => {
            print(result.message);
            return 1;
        } _ => {
            return 0;
        }
    }
}
