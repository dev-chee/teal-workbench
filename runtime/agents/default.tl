@edition("2026");

// 默认 Agent 应用可由 CLI、嵌入宿主或 Bench 驱动；界面能力是可选的。

import "std/agent.tl" as Agent;
import "std/user.tl" as user;
import "std/approval.tl" as approval;
import "std/ui.tl" as ui;
import "std/json.tl" as json;
import "agents/delivery.tl" as delivery;

/// 模型调用的计划文本作为显示资料保存，不作为交付证据。
@tool("agent.update_plan", "Record the current ordered plan as text")
task update_plan(text: string) -> string {
    Agent::emit("plan", text);
    return text;
}

/// 可由主 Agent 在当前授权和根预算内创建的源码审阅角色。
@role("Review source changes and report concrete evidence")
agent Reviewer {}

impl Reviewer {
    pub task review(prompt: string) -> string {
        let options = self.session_options(self.session("review"));
        options.instructions = "Inspect actual sources and return a concise evidence-based review.";
        return self.chat(prompt, options).await;
    }
}

agent Assistant {
    session_key: string,
    generation: int,
    task_goal: string,
    task_session: string,
    task_state: string,
    task_script: string,
    last_delivery: delivery::TaskDelivery | none,
    last_receipt: delivery::Receipt | none,
    goal: string,
    goal_state: string,
    goal_stalls: int,
    last_revision: string,
    plan: string,
    branch_names: string[],
    branch_keys: { string: string },
    branch_stamps: { string: string },
}

impl Assistant {
    pub task submit(input: string) -> string | Error {
        if input == ":model" || input.starts_with(":model ") {
            return "Model bindings are frozen for this run. Finish this run with :quit before selecting a model for a new run.";
        }
        if input == ":plan" || input == ":plan show" {
            if self.plan == "" {
                return "No plan is recorded";
            }
            return self.plan;
        }
        if input == ":delivery" {
            let summary = if self.task_state == "completed" { "A verified delivery is current: " + self.task_script } else { "No verified delivery is available" };
            Agent::emit("delivery", summary);
            return summary;
        }
        if input == ":roles" {
            return "Available role: Reviewer (source review with inherited run limits)";
        }
        if input == ":agent-branch" || input == ":agent-branch list" {
            return "Branches: " + self.branch_names.join(", ");
        }
        if input.starts_with(":agent-branch create ") {
            let name = input.replace(":agent-branch create ", "").trim();
            if name == "" || self.branch_names.contains(name) {
                return "Branch name is empty or already used";
            }
            let stamp = Agent::workspace_stamp().await ?;
            if !stamp.available {
                return "Branch baseline is unavailable: " + stamp.error;
            }
            self.generation += 1;
            let key = "branch-" + string(self.generation);
            self.copy_session(self.session_key, key).await ?;
            self.branch_names.push(name);
            self.branch_keys[name] = key;
            self.branch_stamps[name] = stamp.value;
            return "Branch created: " + name;
        }
        if input.starts_with(":agent-branch switch ") {
            let name = input.replace(":agent-branch switch ", "").trim();
            if !self.branch_names.contains(name) {
                return "Unknown branch: " + name;
            }
            let stamp = Agent::workspace_stamp().await ?;
            if !stamp.available || stamp.value != self.branch_stamps[name] {
                return "Branch is stale against the current workspace";
            }
            self.session_key = self.branch_keys[name];
            return "Branch selected: " + name;
        }
        if input.starts_with(":agent-branch save ") {
            let name = input.replace(":agent-branch save ", "").trim();
            if !self.branch_names.contains(name) {
                return "Unknown branch: " + name;
            }
            let stamp = Agent::workspace_stamp().await ?;
            if !stamp.available {
                return "Branch baseline is unavailable: " + stamp.error;
            }
            self.branch_stamps[name] = stamp.value;
            return "Branch state is saved with the Agent run: " + name;
        }
        if input == ":goal" || input == ":goal status" {
            return "Goal " + self.goal_state + ": " + self.goal;
        }
        if input.starts_with(":goal stop") {
            self.goal_state = "stopped";
            return "Goal stopped";
        }
        let work = input;
        if input.starts_with(":goal start ") {
            self.goal = input.replace(":goal start ", "").trim();
            self.goal_state = "advancing";
            self.goal_stalls = 0;
            self.last_revision = "";
            work = "task " + self.goal;
        } else if input.starts_with(":goal continue") {
            if self.goal_state != "waiting" || self.task_state != "needs_revision" {
                return "No goal is ready to continue";
            }
            self.goal_state = "advancing";
            work = "task-resume";
        }
        if input == "task-status" {
            return "Task " + self.task_state + ": " + self.task_script;
        }
        if input == ":evidence" {
            let candidate = self.last_delivery;
            let previous = self.last_receipt;
            if candidate == none {
                return "No verified delivery is available";
            }
            if previous == none {
                return "No verified delivery is available";
            }
            if self.task_session == "" {
                return "No verified delivery is available";
            }
            let checked = delivery::verify(candidate, self.task_session).await ?;
            match checked {
                delivery::Validation.accepted { receipt } => {
                    if !delivery::same_receipt(previous, receipt) {
                        self.task_state = "stale";
                        return "Delivery evidence is stale";
                    }
                    return "Delivery evidence is current";
                }
                delivery::Validation.rejected { reason } => {
                    self.task_state = "stale";
                    return "Delivery evidence is stale: " + reason;
                }
            }
        }
        if input.starts_with(":task-commit ") {
            if !Agent::can_commit() {
                return "Verified commit service is unavailable";
            }
            let candidate = self.last_delivery;
            let previous = self.last_receipt;
            if candidate == none {
                return "No verified delivery is available";
            }
            if previous == none {
                return "No verified delivery is available";
            }
            if self.task_state != "completed" {
                return "Task delivery is not current";
            }
            let message = input.replace(":task-commit ", "").trim();
            if message == "" {
                return "Commit message is required";
            }
            let checked = delivery::verify(candidate, self.task_session).await ?;
            match checked {
                delivery::Validation.rejected { reason } => {
                    self.task_state = "stale";
                    return "Delivery evidence is stale: " + reason;
                }
                delivery::Validation.accepted { receipt } => {
                    if !delivery::same_receipt(previous, receipt) {
                        self.task_state = "stale";
                        return "Delivery evidence is stale";
                    }
                    let resource = receipt.script.sha256 + ":" + receipt.program_digest + ":" + message;
                    let consent = approval::request("Commit the verified delivery?", "git.commit", resource).await;
                    if !approval::valid(consent) {
                        return "Commit was not approved";
                    }
                    // 批准后再次复查文件和程序，可信服务还会核对原收据身份。
                    let current = delivery::verify(candidate, self.task_session).await ?;
                    match current {
                        delivery::Validation.rejected { reason } => {
                            self.task_state = "stale";
                            return "Delivery evidence is stale: " + reason;
                        }
                        delivery::Validation.accepted { receipt: current_receipt } => {
                            if !delivery::same_receipt(receipt, current_receipt) {
                                self.task_state = "stale";
                                return "Delivery evidence is stale";
                            }
                            let committed = delivery::commit(current_receipt, self.task_session, message, consent.id).await;
                            match committed {
                                Error { .. } => {
                                    return "Commit failed: " + committed.message;
                                } _ => {
                                    return "Committed verified delivery: " + committed;
                                }
                            }
                        }
                    }
                }
            }
        }
        if input == "task-abandon" {
            self.task_state = "abandoned";
            self.task_goal = "";
            self.task_session = "";
            self.last_delivery = none;
            self.last_receipt = none;
            return "Task abandoned";
        }
        let options = self.session_options(self.session(self.session_key));
        options.instructions = "You are a Teal Agent. Inspect actual sources before changing code and report what was verified. When a durable fact has concrete tool evidence, knowledge.propose may submit a draft for independent review; it never publishes directly.";
        let task_input = "";
        let transfer_context = "";
        if work.starts_with("task --with-chat-context ") {
            let projection = self.context_projection(self.session_key);
            if projection.text == "" {
                return "No completed Chat session is available";
            }
            let decision = user::choose("Share this Chat summary with a new Task? " + projection.text, [user::Choice { id: "allow", label: "Share summary" }, user::Choice { id: "deny", label: "Cancel Task" },]).await;
            match decision {
                Error { .. } => {
                    return "User interaction failed: " + decision.message;
                } _ => {
                    let value = decision;
                    match value {
                        user::ChoiceAnswer.selected { id } => {
                            if id != "allow" {
                                return "Task context was not shared";
                            }
                        }
                        user::ChoiceAnswer.skipped => {
                            return "Task context was not shared";
                        }
                    }
                }
            }
            let current = self.context_projection(self.session_key);
            if current.source_digest != projection.source_digest {
                return "Chat changed while context sharing was pending";
            }
            transfer_context = projection.text;
            work = "task " + work.replace("task --with-chat-context ", "");
        }
        if work.starts_with("task ") {
            self.generation += 1;
            self.task_goal = work.replace("task ", "").trim();
            self.task_session = "task-" + string(self.generation);
            self.task_state = "running";
            self.task_script = "";
            self.last_delivery = none;
            self.last_receipt = none;
            task_input = self.task_goal;
        } else if work.starts_with("task-resume") {
            if self.task_state != "needs_revision" {
                return "No task is ready for revision";
            }
            task_input = "Revise the delivery for: " + self.task_goal + ". " + work;
            self.task_state = "running";
        }
        if task_input != "" {
            let task_options = self.session_options(self.session(self.task_session));
            task_options.instructions = "Deliver one Teal script and its verified artifacts. Run format, check, test and run in order. Submit a TaskDelivery value through __teal_complete. Propose durable knowledge only with exact earlier tool call IDs as source_refs.";
            if transfer_context != "" {
                task_options.context = [transfer_context];
            }
            let attempts = if self.goal_state == "advancing" { 3 } else { 1 };
            for attempt in 0..attempts {
                Agent::emit("status", "Verification attempt " + string(attempt + 1));
                let candidate: delivery::TaskDelivery = self.think(task_input, task_options).await;
                for receipt in Agent::evidence(self.task_session) {
                    if receipt.tool == "agent.update_plan" {
                        self.plan = json::read_string(receipt.value_json, "");
                    }
                }
                let checked = delivery::verify(candidate, self.task_session).await ?;
                match checked {
                    delivery::Validation.accepted { receipt } => {
                        self.task_state = "completed";
                        self.task_script = receipt.script.path;
                        self.last_delivery = candidate;
                        self.last_receipt = receipt;
                        if self.goal_state == "advancing" {
                            self.goal_state = "completed";
                        }
                        return "Verified delivery: " + receipt.summary + " (" + receipt.script.path + ")";
                    }
                    delivery::Validation.rejected { reason } => {
                        self.task_state = "needs_revision";
                        if reason == self.last_revision {
                            self.goal_stalls += 1;
                        } else {
                            self.goal_stalls = 0;
                            self.last_revision = reason;
                        }
                        if self.goal_state != "advancing" || self.goal_stalls >= 2 {
                            if self.goal_state == "advancing" {
                                self.goal_state = "waiting";
                            }
                            return "Delivery needs revision: " + reason;
                        }
                        task_input = "Revise the same delivery using current tool evidence: " + reason;
                        self.task_state = "running";
                    }
                }
            }
            self.goal_state = "waiting";
            return "Goal paused after its revision limit";
        }
        return self.chat(input, options).await;
    }
}

fn view(answer: string, draft: string) -> ui::Node {
    return ui::node("agent", ui::Component.column {
        children: [ui::node("answer", ui::Component.text { text: answer, style: ui::Style.normal, wrap: true }), ui::node("prompt", ui::Component.input { value: draft, placeholder: "Ask the Agent" }), ui::node("send", ui::Component.button { label: "Send", enabled: true }),],
        gap: 1,
    });
}

task application() -> none | Error {
    let assistant = Assistant {
        session_key: "chat-0",
        generation: 0,
        task_goal: "",
        task_session: "",
        task_state: "idle",
        task_script: "",
        last_delivery: none,
        last_receipt: none,
        goal: "",
        goal_state: "idle",
        goal_stalls: 0,
        last_revision: "",
        plan: "",
        branch_names: [],
        branch_keys: {},
        branch_stamps: {},
    };
    let draft = "";
    if Agent::supports_ui() {
        Agent::render(view("Ready", draft)) ?;
    }
    loop {
        let input = Agent::receive().await ?;
        match input {
            Agent::Input.text { value } => {
                if value == ":quit" {
                    break;
                }
                let reply = assistant.submit(value).await ?;
                Agent::emit("text", reply);
                if Agent::supports_ui() {
                    Agent::render(view(reply, draft)) ?;
                }
            }
            Agent::Input.command { value } => {
                if value == "quit" {
                    break;
                }
                let reply = assistant.submit(":" + value).await ?;
                Agent::emit("text", reply);
                if Agent::supports_ui() {
                    Agent::render(view(reply, draft)) ?;
                }
            }
            Agent::Input.ui { event } => {
                match event {
                    ui::UiEvent.change { id, value } => {
                        if id == "prompt" {
                            draft = value;
                        }
                    }
                    ui::UiEvent.activate { id } => {
                        if id == "send" && draft.trim() != "" {
                            let reply = assistant.submit(draft).await ?;
                            draft = "";
                            Agent::emit("text", reply);
                            Agent::render(view(reply, draft)) ?;
                        }
                    } _ => {}
                }
            }
            Agent::Input.skipped => {
                Agent::emit("status", "Input skipped");
            }
        }
    }
}

/// 入口显式处理应用错误，避免把失败当作成功结束。
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
