@edition("2026");

// 持久源码 Agent 演示：同一入口可由 CLI、嵌入宿主与 Bench 驱动。
import "std/agent.tl" as Agent;
import "std/approval.tl" as approval;
import "agents/delivery.tl" as delivery;

agent Sdlc {
    phase: string,
    goal: string,
    session: string,
    revision: int,
    candidate: delivery::TaskDelivery | none,
    receipt: delivery::Receipt | none,
}

impl Sdlc {
    pub task handle_input(input: string) -> string | Error {
        if input == "status" {
            return self.phase + ": " + self.goal;
        }
        if input == "stop" {
            self.phase = "stopped";
            return "Workflow stopped";
        }
        if input.starts_with("commit ") && self.phase == "delivered" {
            if !Agent::can_commit() {
                return "Verified commit service is unavailable";
            }
            let candidate = self.candidate;
            let previous = self.receipt;
            if candidate == none {
                return "No verified delivery is available";
            }
            if previous == none {
                return "No verified delivery is available";
            }
            let message = input.replace("commit ", "").trim();
            if message == "" {
                return "Commit message is required";
            }
            let checked = delivery::verify(candidate, self.session + "-task").await ?;
            match checked {
                delivery::Validation.rejected { reason } => {
                    self.phase = "needs_revision";
                    return "Delivery changed: " + reason;
                }
                delivery::Validation.accepted { receipt } => {
                    if !delivery::same_receipt(previous, receipt) {
                        self.phase = "needs_revision";
                        return "Delivery changed after verification";
                    }
                    let resource = receipt.script.sha256 + ":" + receipt.program_digest + ":" + message;
                    let consent = approval::request("Commit the verified delivery?", "git.commit", resource).await;
                    if !approval::valid(consent) {
                        return "Commit was not approved";
                    }
                    let current = delivery::verify(candidate, self.session + "-task").await ?;
                    match current {
                        delivery::Validation.rejected { reason } => {
                            return "Delivery changed: " + reason;
                        }
                        delivery::Validation.accepted { receipt: final_receipt } => {
                            if !delivery::same_receipt(receipt, final_receipt) {
                                return "Delivery changed after approval";
                            }
                            let committed = delivery::commit(final_receipt, self.session + "-task", message, consent.id).await;
                            match committed {
                                Error { .. } => {
                                    return "Commit failed: " + committed.message;
                                } _ => {
                                    self.phase = "committed";
                                    Agent::emit("commit", committed);
                                    return "Verified commit: " + committed;
                                }
                            }
                        }
                    }
                }
            }
        }
        if input.starts_with("external-deployment ") && self.phase == "committed" {
            let record = input.replace("external-deployment ", "").trim();
            if record == "" {
                return "Deployment record is required";
            }
            let consent = approval::request("Confirm this external deployment result?", "sdlc.deploy", record).await;
            if !approval::valid(consent) {
                return "Deployment result was not approved";
            }
            self.phase = "deployed";
            Agent::emit("deployment", record);
            return "Deployment recorded; maintenance input is available";
        }
        let next_input = input;
        if input.starts_with("import-alert ") && self.phase == "deployed" {
            self.phase = "idle";
            next_input = "Implement the approved incident response: " + input.replace("import-alert ", "");
        }
        if self.phase != "idle" && self.phase != "deployed" && self.phase != "stopped" {
            return "Workflow is waiting at " + self.phase;
        }
        self.goal = next_input.trim();
        if self.goal == "" {
            return "Enter a development goal";
        }
        self.revision += 1;
        self.session = "sdlc-" + string(self.revision);
        self.candidate = none;
        self.receipt = none;
        let options = self.session_options(self.session(self.session));
        options.instructions = "Inspect real sources and keep implementation and verification evidence separate.";

        self.phase = "intent";
        let intent = self.chat("Write a concise implementation intent for: " + self.goal, options).await;
        Agent::emit("intent", intent);
        self.phase = "specification";
        let specification = self.chat("Write acceptance criteria for: " + self.goal, options).await;
        Agent::emit("specification", specification);
        self.phase = "plan";
        let plan = self.chat("Write an ordered implementation plan for: " + self.goal, options).await;
        Agent::emit("plan", plan);

        self.phase = "approval";
        let consent = approval::request("Approve this plan before implementation: " + plan, "sdlc.implement", self.goal,).await;
        if !approval::valid(consent) {
            self.phase = "stopped";
            return "Plan was not approved";
        }

        self.phase = "implementation";
        let task_options = self.session_options(self.session(self.session + "-task"));
        task_options.instructions = "Implement the approved plan and submit TaskDelivery through __teal_complete. Run format, check, test and run in order.";
        let candidate: delivery::TaskDelivery = self.think("Implement " + self.goal + ". Approved intent: " + intent + ". Acceptance: " + specification + ". Plan: " + plan, task_options,).await;
        self.phase = "verification";
        let checked = delivery::verify(candidate, self.session + "-task").await ?;
        match checked {
            delivery::Validation.accepted { receipt } => {
                self.phase = "delivered";
                self.candidate = candidate;
                self.receipt = receipt;
                Agent::emit("delivery", receipt.summary + " (" + receipt.script.path + ")");
                return "Verified delivery: " + receipt.script.path;
            }
            delivery::Validation.rejected { reason } => {
                self.phase = "needs_revision";
                return "Delivery needs revision: " + reason;
            }
        }
    }
}

task application() -> none | Error {
    let worker = Sdlc {
        phase: "idle",
        goal: "",
        session: "",
        revision: 0,
        candidate: none,
        receipt: none
    };
    loop {
        let input = Agent::receive().await ?;
        match input {
            Agent::Input.text { value } => {
                if value == ":quit" {
                    break;
                }
                Agent::emit("text", worker.handle_input(value).await ?);
            }
            Agent::Input.command { value } => {
                if value == "quit" {
                    break;
                }
                Agent::emit("text", worker.handle_input(value).await ?);
            }
            Agent::Input.ui { event } => {
                Agent::emit("status", "UI event received");
            }
            Agent::Input.skipped => {
                Agent::emit("status", "Input skipped");
            }
        }
    }
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
