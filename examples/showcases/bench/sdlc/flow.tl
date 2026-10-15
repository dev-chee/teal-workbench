@edition("2026");

// Cross-tier Showcase：阶段状态机只根据已保存制品和验证收据前进。
import "bench/extension.tl" as ext;
import "std/json.tl" as json;
import "common.tl" as common;

fn request_plan_approval() -> none | Error {
    ext::set("stage", "plan") ?;
    let artifacts = [common::root() ? +"/intent.md", common::root() ? +"/spec.md", common::root() ? +"/plan.md"];
    ext::set("approved_artifacts", common::strings(artifacts) ?) ?;
    let id = ext::request_approval("Review the final intent, specification, plan, write scope and required checks. Approval starts implementation, verification and local delivery.", common::strings(artifacts) ?, "plan", "") ?;
    ext::set("pending_approval", id) ?;
    ext::set("status", "Awaiting final plan approval") ?;
}

fn input_event(input: string) -> none | Error {
    if input == "status" {
        ext::message(ext::get("status") ?) ?;
        return;
    }
    if ext::get("busy") ? == "true" {
        ext::message("A stage is running. Use :extension stop sdlc to interrupt it.") ?;
        return;
    }
    if input == "close" {
        ext::set("active", "false") ?;
        ext::set("status", "Closed by user") ?;
        return;
    }
    if input.starts_with("import-alert ") {
        if ext::get("flow_id") ? == "" {
            ext::set("flow_id", ext::event("instance") ? +"-" + ext::event("id") ?) ?;
        }
        ext::set("feedback", input) ?;
        common::launch("diagnose", input) ?;
        return;
    }
    if input.starts_with("external-deployment ") {
        imported_deployment(input) ?;
        return;
    }
    if ext::get("active") ? != "true" {
        new_flow(input) ?;
        return;
    }
    revise(input) ?;
}

fn imported_deployment(input: string) -> none | Error {
    if ext::get("stage") ? != "deploy" {
        ext::message("External deployment evidence is accepted only at the deployment gate.") ?;
        return;
    }
    ext::set("external_deployment", input) ?;
    let id = ext::request_approval("Confirm the external deployment result and its relation to the recorded commit: " + input, common::strings([common::root() ? +"/delivery.json"]) ?, "external-deployment", ext::config("environment") ?) ?;
    ext::set("pending_approval", id) ?;
}

fn new_flow(input: string) -> none | Error {
    if input == "" {
        return;
    }
    ext::set("active", "true") ?;
    ext::set("flow_id", ext::event("instance") ? +"-" + ext::event("id") ?) ?;
    ext::set("goal", input) ?;
    ext::set("feedback", "") ?;
    ext::set("revisions", "0") ?;
    for stage in ["intent", "spec", "plan"] {
        ext::set(stage + "_approval", "") ?;
    }
    ext::set("stage", "intent") ?;
    common::launch("initialize", input) ?;
}

fn revise(input: string) -> none | Error {
    ext::set("feedback", input) ?;
    if input.starts_with("revise intent") {
        ext::set("stage", "intent") ?;
    }
    if input.starts_with("revise spec") {
        ext::set("stage", "spec") ?;
    }
    if input.starts_with("revise plan") {
        ext::set("stage", "plan") ?;
    }
    let stage = ext::get("stage") ?;
    if stage == "intent" || stage == "spec" || stage == "plan" {
        if stage == "intent" {
            ext::set("intent_approval", "") ?;
            ext::set("spec_approval", "") ?;
        }
        if stage == "spec" {
            ext::set("spec_approval", "") ?;
        }
        ext::set("plan_approval", "") ?;
        common::launch(stage, input) ?;
    } else if stage == "build" || stage == "verify" {
        common::launch("build", input) ?;
    } else {
        ext::message("Use import-alert <evidence>, external-deployment <record>, or close.") ?;
    }
}

fn approval_event() -> none | Error {
    let id = ext::event("receipt") ?;
    if id != ext::get("pending_approval") ? {
        return;
    }
    if ext::approval(id) ? != "approved" {
        ext::set("status", "Approval rejected; provide revision instructions") ?;
        return;
    }
    let stage = ext::get("stage") ?;
    if stage == "deploy" {
        ext::set("deploy_approval", id) ?;
        common::launch("deploy", "") ?;
    } else if stage == "diagnose" {
        ext::set("goal", "Implement the approved incident intent: " + ext::get("incident_artifact") ? +". Include a permanent regression or Eval.") ?;
        ext::set("flow_id", ext::event("instance") ? +"-" + ext::event("id") ?) ?;
        ext::set("active", "true") ?;
        ext::set("revisions", "0") ?;
        ext::set("stage", "intent") ?;
        ext::set("plan_approval", "") ?;
        common::launch("initialize", ext::get("goal") ?) ?;
    } else if stage == "plan" {
        ext::set("plan_approval", id) ?;
        common::launch("commit", "") ?;
    }
}

fn draft_completed(task_name: string, result: string) -> none | Error {
    // 保留模型结构化文件清单，计划文档中也有相同的确定性清单供人工审查。
    if task_name == "plan" {
        let files: string[] = [];
        for item in json::items(result, "/files") {
            files.push(json::read_string(item, ""));
        }
        ext::set("planned_files", common::strings(files) ?) ?;
    }
    if task_name == "intent" {
        ext::set("stage", "spec") ?;
        common::launch("spec", "") ?;
    } else if task_name == "spec" {
        ext::set("stage", "plan") ?;
        common::launch("plan", "") ?;
    } else {
        request_plan_approval() ?;
    }
}

fn commit_completed() -> none | Error {
    ext::set("stage", "build") ?;
    common::launch("build", "") ?;
}

fn build_completed(result: string) -> none | Error {
    if json::read_boolean(result, "/plan_changed") {
        ext::set("stage", "plan") ?;
        ext::set("plan_approval", "") ?;
        ext::set("status", "Implementation requires a revised plan; provide instructions before continuing") ?;
    } else {
        ext::set("stage", "verify") ?;
        common::launch("verify", result) ?;
    }
}

fn verification_completed(result: string) -> none | Error {
    ext::set("verification", result) ?;
    if json::read_boolean(result, "/passed") {
        common::launch("deliver", result) ?;
    } else {
        let attempts = int(ext::get("revisions") ?) + 1;
        ext::set("revisions", string(attempts)) ?;
        let limit = 3;
        if ext::config("max_revisions") ?.is_int() {
            limit = int(ext::config("max_revisions") ?);
        }
        ext::set("stage", "build") ?;
        ext::set("feedback", result) ?;
        if attempts <= limit {
            common::launch("build", result) ?;
        } else {
            ext::set("status", "Revision budget exhausted; human input required") ?;
        }
    }
}

fn delivery_completed(result: string) -> none | Error {
    ext::set("delivery", result) ?;
    ext::set("stage", "deploy") ?;
    ext::set("external_deployment", "") ?;
    if ext::config("command.deploy") ? == "" {
        ext::set("status", "Awaiting external-deployment <record>; no deploy command configured") ?;
    } else {
        let id = ext::request_approval("Authorize deployment of the verified commit: " + result, common::strings([common::root() ? +"/delivery.json"]) ?, "deploy", ext::config("environment") ?) ?;
        ext::set("pending_approval", id) ?;
        ext::set("status", "Awaiting deployment approval") ?;
    }
}

fn diagnosis_completed(result: string) -> none | Error {
    ext::set("stage", "diagnose") ?;
    ext::set("incident_artifact", result) ?;
    let id = ext::request_approval("Triage the diagnostic intent. Approval accepts it for a new development cycle.", common::strings([result]) ?, "triage", "") ?;
    ext::set("pending_approval", id) ?;
    ext::set("status", "Awaiting incident triage") ?;
}

/// 事件分发只选择阶段函数；模型与工具工作均在后台 task 内执行。
pub fn dispatch(action: string, input: string) -> none | Error {
    if action == "enter" {
        ext::message("SDLC: enter a goal or import-alert evidence. Review artifacts in the Bench approval dialog.") ?;
        return;
    }
    if action == "input" || action == "command" {
        input_event(input) ?;
        return;
    }
    if action == "approval" {
        approval_event() ?;
        return;
    }
    if action == "task_failed" {
        ext::set("busy", "false") ?;
        ext::set("status", "Stage failed: " + ext::event("result") ?) ?;
        return;
    }
    if action != "task_completed" {
        return;
    }
    ext::set("busy", "false") ?;
    let name = ext::event("task") ?;
    let result = ext::event("result") ?;
    if name == "initialize" {
        common::launch("intent", ext::get("goal") ?) ?;
    } else if name == "intent" || name == "spec" || name == "plan" {
        draft_completed(name, result) ?;
    } else if name == "commit" {
        commit_completed() ?;
    } else if name == "build" {
        build_completed(result) ?;
    } else if name == "verify" {
        verification_completed(result) ?;
    } else if name == "deliver" {
        delivery_completed(result) ?;
    } else if name == "diagnose" {
        diagnosis_completed(result) ?;
    } else if name == "deploy" {
        ext::set("stage", "maintain") ?;
        ext::set("status", "Deployment completed; ready for maintenance input") ?;
    }
}
