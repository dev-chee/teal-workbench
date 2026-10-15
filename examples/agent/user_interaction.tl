@edition("2026");

// Tier Agent：持久文本问答与稳定 ID 选择；回答不授予权限或批准副作用。
// 终端直接回答；非交互环境使用 inspect 和 resume --reply-file 恢复。
import "std/user.tl" as user;

task main() {
    workflow().await;
}

task workflow() {
    let topic_result = user::ask("What should the report cover?").await;
    match topic_result {
        Error { .. } => {
            print(topic_result.message);
            return;
        } _ => {
            let topic = topic_result;
            match topic {
                user::Answer.text { value } => {
                    print("Topic: " + value);
                }
                user::Answer.skipped => {
                    print("Report skipped");
                    return;
                }
            }
        }
    }

    let format_result = user::choose("Choose a report format", [user::Choice { id: "brief", label: "Short summary" }, user::Choice { id: "detailed", label: "Detailed analysis" },]).await;
    match format_result {
        Error { .. } => {
            print(format_result.message);
        } _ => {
            let format = format_result;
            match format {
                user::ChoiceAnswer.selected { id } => {
                    if id == "brief" {
                        print("Preparing a short summary");
                    } else {
                        print("Preparing a detailed analysis");
                    }
                }
                user::ChoiceAnswer.skipped => {
                    print("No format selected");
                }
            }
        }
    }
}
