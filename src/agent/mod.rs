use crate::config::RuneConfig;
use serde_json::Value;
use anyhow::Result;

#[derive(Debug, Clone)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: Value,
}

#[derive(Debug, Clone)]
pub struct ToolResult {
    pub id: String,
    pub content: String,
    pub is_error: bool,
}

#[derive(Debug, Clone)]
pub enum Message {
    System(String),
    User(String),
    Assistant { content: Option<String>, tool_calls: Vec<ToolCall> },
    ToolResponse(ToolResult),
}

#[derive(Debug)]
pub enum StopReason {
    FinalAnswer(String),
    MaxSteps,
    TokenBudgetExhausted,
    Error(String),
    UserInterrupt,
}

pub struct Agent {
    pub config: RuneConfig,
    pub messages: Vec<Message>,
    pub step_count: u32,
    pub tokens_used: u32,
}

impl Agent {
    pub fn new(config: RuneConfig) -> Self {
        Agent { config, messages: Vec::new(), step_count: 0, tokens_used: 0 }
    }

    /// 設定系統提示
    pub fn set_system_prompt(&mut self, prompt: &str) {
        self.messages.push(Message::System(prompt.to_string()));
    }

    /// 執行 agent loop：呼叫 LLM → 取得回應 → 若有 tool calls 就執行 → 重複直到終止
    pub async fn run(&mut self, user_input: &str) -> StopReason {
        // 1. 加入 user message
        self.messages.push(Message::User(user_input.to_string()));

        // 2. Loop
        loop {
            // d. 檢查 step_count >= max_steps → StopReason::MaxSteps
            if self.step_count >= self.config.max_steps {
                return StopReason::MaxSteps;
            }
            // e. 檢查 tokens_used >= token_budget → StopReason::TokenBudgetExhausted
            if self.tokens_used >= self.config.token_budget {
                return StopReason::TokenBudgetExhausted;
            }

            // increment step count for this iteration
            self.step_count = self.step_count.saturating_add(1);

            // a. 呼叫 LLM (placeholder: call_llm)
            match self.call_llm().await {
                Ok(Message::Assistant { content, tool_calls }) => {
                    // b. 若回應是 final answer → 回傳 StopReason::FinalAnswer
                    if tool_calls.is_empty() {
                        if let Some(text) = content {
                            return StopReason::FinalAnswer(text);
                        } else {
                            return StopReason::Error("assistant returned empty content".to_string());
                        }
                    }

                    // c. 若有 tool_calls → dispatch 每個 tool → 收集結果 → 加入 messages
                    for call in tool_calls {
                        let res = self.dispatch_tool(&call).await;
                        // account some token usage for tool execution (simple heuristic)
                        self.tokens_used = self.tokens_used.saturating_add(1);
                        self.messages.push(Message::ToolResponse(res));
                    }

                    // continue the loop to call LLM again after tool results are in messages
                    continue;
                }
                Ok(other) => {
                    // push any non-assistant messages and continue
                    self.messages.push(other);
                    continue;
                }
                Err(e) => {
                    return StopReason::Error(format!("call_llm error: {}", e));
                }
            }
        }
    }

    /// Placeholder: 呼叫 LLM API（目前回傳模擬回應）
    async fn call_llm(&self) -> Result<Message> {
        // TODO: 真正的 HTTP 呼叫
        // 目前模擬 behaviour: 第一次回傳 tool call，第二次回傳 final answer
        if self.step_count == 1 {
            let call = ToolCall {
                id: "1".to_string(),
                name: "echo".to_string(),
                arguments: serde_json::json!({ "text": "Hello from tool" }),
            };

            Ok(Message::Assistant {
                content: Some("I will call a tool.".to_string()),
                tool_calls: vec![call],
            })
        } else {
            Ok(Message::Assistant {
                content: Some("I've completed the task.".to_string()),
                tool_calls: vec![],
            })
        }
    }

    /// Dispatch tool call 到對應的 handler
    async fn dispatch_tool(&self, call: &ToolCall) -> ToolResult {
        match call.name.as_str() {
            "echo" => {
                let text = call
                    .arguments
                    .get("text")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                ToolResult { id: call.id.clone(), content: format!("echo: {}", text), is_error: false }
            }
            other => ToolResult { id: call.id.clone(), content: format!("unknown tool: {}", other), is_error: true },
        }
    }
}
