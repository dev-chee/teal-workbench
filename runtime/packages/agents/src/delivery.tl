@edition("2026");

// 可复用的脚本交付契约：阶段规则由 Teal 执行，工具和文件事实来自宿主收据。
import "std/agent.tl" as app;
import "std/json.tl" as json;

pub class VerificationReference { pub call_id: string, pub claim: string }
pub class TaskDelivery {
    pub summary: string,
    pub script: string,
    pub artifacts: string[],
    pub verification: VerificationReference[],
}
pub class Artifact { pub path: string, pub sha256: string, pub size: string }
pub class Receipt {
    pub summary: string,
    pub script: Artifact,
    pub artifacts: Artifact[],
    pub program_digest: string,
    pub runtime_profile: string,
    pub verification: VerificationReference[],
}
pub enum Validation { accepted { receipt: Receipt }, rejected { reason: string } }

/// 后续动作重新核验时比较同一次交付的文件和程序身份。
pub fn same_receipt(previous: Receipt, current: Receipt) -> bool {
    if previous.script.path != current.script.path || previous.script.sha256 != current.script.sha256
        || previous.script.size != current.script.size
        || previous.program_digest != current.program_digest
        || previous.runtime_profile != current.runtime_profile
        || previous.artifacts.len() != current.artifacts.len()
        || previous.verification.len() != current.verification.len() { return false; }
    let index = 0;
    loop {
        if index >= previous.artifacts.len() { break; }
        let left = previous.artifacts[index];
        let right = current.artifacts[index];
        if left.path != right.path || left.sha256 != right.sha256 || left.size != right.size {
            return false;
        }
        index += 1;
    }
    index = 0;
    loop {
        if index >= previous.verification.len() { break; }
        if previous.verification[index].call_id != current.verification[index].call_id { return false; }
        index += 1;
    }
    return true;
}

/// 已接受收据的后续提交；调用方应在本次提交前重新执行 verify 并取得批准。
pub task commit(receipt: Receipt, session_key: string, message: string, approval_id: string) -> string {
    let paths: string[] = [];
    let hashes: string[] = [];
    let sizes: string[] = [];
    let ids: string[] = [];
    for artifact in receipt.artifacts {
        paths.push(artifact.path);
        hashes.push(artifact.sha256);
        sizes.push(artifact.size);
    }
    for reference in receipt.verification { ids.push(reference.call_id); }
    return app::commit_delivery_or_throw(message, session_key, receipt.summary,
        receipt.script.path, receipt.script.sha256, receipt.script.size,
        paths, hashes, sizes, receipt.program_digest, receipt.runtime_profile, ids, approval_id).await;
}

fn safe_relative(path: string) -> bool {
    if path == "" || path.starts_with("/") || path.starts_with(".teal/")
        || path.contains("\\") || path.contains(":")
        || path.contains("\n") || path.contains("\r") { return false; }
    for segment in path.split("/") {
        if segment == "" || segment == "." || segment == ".." { return false; }
    }
    return true;
}

fn find(receipts: app::ToolReceipt[], id: string) -> app::ToolReceipt | none {
    let matched: app::ToolReceipt | none = none;
    for receipt in receipts {
        if receipt.call_id == id {
            if matched != none { return none; }
            matched = receipt;
        }
    }
    return matched;
}

fn field(receipt: app::ToolReceipt, key: string) -> string {
    let pointer = "/" + key;
    if json::has(receipt.value_json, pointer) {
        return json::read_string(receipt.value_json, pointer);
    }
    if json::has(receipt.arguments_json, pointer) {
        return json::read_string(receipt.arguments_json, pointer);
    }
    return "";
}

fn exit_zero(receipt: app::ToolReceipt) -> bool {
    return receipt.success && json::has(receipt.value_json, "/exit_code")
        && json::read_number(receipt.value_json, "/exit_code") == "0";
}

/// 用模型本轮引用的真实调用和当前文件内容核验脚本交付。
/// 验证失败返回原因，不把模型摘要或文件存在性当作成功证据。
pub task verify(delivery: TaskDelivery, session_key: string) -> Validation {
    if delivery.summary.trim() == "" || delivery.summary.len() > 16384 {
        return Validation.rejected { reason: "Delivery summary is invalid" };
    }
    if !safe_relative(delivery.script) || !delivery.script.ends_with(".tl") {
        return Validation.rejected { reason: "Delivery script path is invalid" };
    }
    if delivery.artifacts.len() > 64 || delivery.verification.len() > 64 {
        return Validation.rejected { reason: "Delivery contains too many entries" };
    }
    let receipts = app::evidence(session_key);
    let selected: app::ToolReceipt[] = [];
    for reference in delivery.verification {
        let receipt = find(receipts, reference.call_id);
        if receipt == none {
            return Validation.rejected { reason: "Verification call ID is not in the model history" };
        }
        for previous in selected {
            if previous.call_id == reference.call_id {
                return Validation.rejected { reason: "Verification call IDs are duplicated" };
            }
        }
        selected.push(receipt);
    }
    let formatted: app::ToolReceipt | none = none;
    let checked: app::ToolReceipt | none = none;
    let executed: app::ToolReceipt | none = none;
    let has_test = false;
    for receipt in selected {
        if receipt.tool != "teal.fmt_check" && receipt.tool != "teal.check"
            && receipt.tool != "teal.test" && receipt.tool != "teal.run" {
            return Validation.rejected { reason: "Verification call is not a delivery stage" };
        }
        if receipt.tool == "teal.fmt_check" { formatted = receipt; }
        if receipt.tool == "teal.check" { checked = receipt; }
        if receipt.tool == "teal.test" { has_test = true; }
        if receipt.tool == "teal.run" { executed = receipt; }
    }
    if formatted == none { return Validation.rejected { reason: "Format receipt is required" }; }
    if checked == none { return Validation.rejected { reason: "Check receipt is required" }; }
    // 是否必须运行测试由当前编译程序的真实 `@test` 清单决定；无测试脚本允许
    // `fmt_check → check → run`，有测试脚本则必须在检查与执行之间提供 `teal.test`。
    if executed == none { return Validation.rejected { reason: "Run receipt is required" }; }
    if !exit_zero(formatted) || !exit_zero(checked) || !exit_zero(executed) {
        return Validation.rejected { reason: "A required verification command failed" };
    }
    if !(formatted.sequence < checked.sequence && checked.sequence < executed.sequence) {
        return Validation.rejected { reason: "Verification commands are out of order" };
    }
    let hash = app::file_evidence_or_throw(delivery.script).await;
    if !hash.available {
        return Validation.rejected { reason: "Delivery script evidence is unavailable: " + hash.error };
    }
    let script_path = hash.path;
    for receipt in [formatted, checked, executed] {
        let path = field(receipt, "path");
        if path == "" {
            return Validation.rejected { reason: "Verification path differs from the delivered script" };
        }
        let verified_path = app::file_evidence_or_throw(path).await;
        if !verified_path.available || verified_path.path != script_path {
            return Validation.rejected { reason: "Verification path differs from the delivered script" };
        }
    }
    if field(formatted, "source_digest") != hash.sha256 {
        return Validation.rejected { reason: "Script content changed after formatting" };
    }
    let digest = field(checked, "program_digest");
    let profile = field(checked, "runtime_profile");
    if digest == "" || digest != field(executed, "program_digest")
        || profile == "" || profile != field(executed, "runtime_profile") {
        return Validation.rejected { reason: "Program or runtime profile differs across verification" };
    }
    for receipt in selected {
        if receipt.tool != "teal.test" { continue; }
        if !exit_zero(receipt) {
            return Validation.rejected { reason: "A required verification command failed" };
        }
        if !(checked.sequence < receipt.sequence && receipt.sequence < executed.sequence) {
            return Validation.rejected { reason: "Verification commands are out of order" };
        }
        let path = field(receipt, "path");
        if path == "" {
            return Validation.rejected { reason: "Verification path differs from the delivered script" };
        }
        let verified_path = app::file_evidence_or_throw(path).await;
        if !verified_path.available || verified_path.path != script_path {
            return Validation.rejected { reason: "Verification path differs from the delivered script" };
        }
        if digest != field(receipt, "program_digest")
            || profile != field(receipt, "runtime_profile") {
            return Validation.rejected { reason: "Program or runtime profile differs across verification" };
        }
    }
    let current = app::current_program_or_throw(delivery.script, profile).await;
    if !current.available {
        return Validation.rejected { reason: "Current program verification is unavailable: " + current.error };
    }
    if current.test_count != "0" && !has_test {
        return Validation.rejected { reason: "Test receipt is required for a script with registered tests" };
    }
    if current.digest != digest {
        return Validation.rejected { reason: "Program changed after verification" };
    }
    let artifacts: Artifact[] = [];
    let paths: string[] = [delivery.script];
    for path in delivery.artifacts {
        if !safe_relative(path) || paths.contains(path) {
            return Validation.rejected { reason: "Artifact path is invalid or duplicated" };
        }
        paths.push(path);
        let file = app::file_evidence_or_throw(path).await;
        if !file.available {
            return Validation.rejected { reason: "Artifact evidence is unavailable: " + file.error };
        }
        artifacts.push(Artifact { path: path, sha256: file.sha256, size: file.size });
    }
    return Validation.accepted { receipt: Receipt {
        summary: delivery.summary,
        script: Artifact { path: delivery.script, sha256: hash.sha256, size: hash.size },
        artifacts: artifacts,
        program_digest: digest,
        runtime_profile: profile,
        verification: delivery.verification,
    } };
}
