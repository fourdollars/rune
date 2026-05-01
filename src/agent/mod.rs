use crate::config::RuneConfig;
use crate::provider::{LlmMessage, LlmRequest, LlmResponse, LlmToolCall, ProviderRegistry};
use crate::tools::ToolRegistry;
use anyhow::Result;
use colored::Colorize;
use std::path::PathBuf;
use tracing::{debug, info};

/// Agent's stop reason.
#[derive(Debug)]
pub enum StopReason {
    FinalAnswer(String),
    MaxSteps,
    TokenBudgetExhausted,
    Error(String),
    UserInterrupt,
}

/// The AI Agent — orchestrates LLM calls and tool execution.
pub struct Agent {
    pub config: RuneConfig,
    messages: Vec<LlmMessage>,
    step_count: u32,
    tokens_used: u32,
    provider: ProviderRegistry,
    tools: ToolRegistry,
}

impl Agent {
    pub fn new(config: RuneConfig, provider: ProviderRegistry) -> Self {
        let tools = ToolRegistry::new(vec![PathBuf::from("/tmp"), PathBuf::from(".")]);
        Agent {
            config,
            messages: Vec::new(),
            step_count: 0,
            tokens_used: 0,
            provider,
            tools,
        }
    }

    /// Set the system prompt.
    pub fn set_system_prompt(&mut self, prompt: &str) {
        // Insert or replace system message at position 0
        if self.messages.first().map(|m| m.role == "system").unwrap_or(false) {
            self.messages[0].content = Some(prompt.to_string());
        } else {
            self.messages.insert(0, LlmMessage {
                role: "system".to_string(),
                content: Some(prompt.to_string()),
                tool_calls: None,
                tool_call_id: None,
            });
        }
    }

    /// Reset conversation state for a new run (keeps system prompt).
    pub fn reset(&mut self) {
        let system = self.messages.first().cloned();
        self.messages.clear();
        if let Some(sys) = system {
            self.messages.push(sys);
        }
        self.step_count = 0;
        self.tokens_used = 0;
    }

    /// Run the agent loop: send user input → LLM → tools → repeat until done.
    pub async fn run(&mut self, user_input: &str) -> StopReason {
        // Add user message
        self.messages.push(LlmMessage {
            role: "user".to_string(),
            content: Some(user_input.to_string()),
            tool_calls: None,
            tool_call_id: None,
        });

        loop {
            // Check limits
            if self.step_count >= self.config.max_steps {
                return StopReason::MaxSteps;
            }
            if self.tokens_used >= self.config.token_budget {
                return StopReason::TokenBudgetExhausted;
            }

            self.step_count += 1;
            info!(step = self.step_count, tokens = self.tokens_used, "agent loop step");

            // Call LLM
            let response = match self.call_llm().await {
                Ok(r) => r,
                Err(e) => return StopReason::Error(format!("LLM call failed: {}", e)),
            };

            // Update token usage
            self.tokens_used += response.usage.total_tokens;

            // If no tool calls, we have our final answer
            if response.tool_calls.is_empty() {
                let answer = response.content.unwrap_or_default();
                // Add assistant message to history
                self.messages.push(LlmMessage {
                    role: "assistant".to_string(),
                    content: Some(answer.clone()),
                    tool_calls: None,
                    tool_call_id: None,
                });
                return StopReason::FinalAnswer(answer);
            }

            // We have tool calls — add assistant message with tool_calls
            self.messages.push(LlmMessage {
                role: "assistant".to_string(),
                content: response.content.clone(),
                tool_calls: Some(response.tool_calls.clone()),
                tool_call_id: None,
            });

            // Execute each tool call
            for tc in &response.tool_calls {
                let result = self.execute_tool_call(tc).await;

                // Add tool result message
                self.messages.push(LlmMessage {
                    role: "tool".to_string(),
                    content: Some(result),
                    tool_calls: None,
                    tool_call_id: Some(tc.id.clone()),
                });
            }

            // Loop back to call LLM with the tool results
        }
    }

    /// Call the LLM provider.
    async fn call_llm(&self) -> Result<LlmResponse> {
        let tool_defs = self.tools.tool_definitions();

        let request = LlmRequest {
            model: self.config.model.clone(),
            messages: self.messages.clone(),
            tools: if tool_defs.is_empty() { None } else { Some(tool_defs) },
            max_tokens: None,
        };

        debug!(model = %self.config.model, messages = self.messages.len(), "calling LLM");
        self.provider.chat(request).await
    }

    /// Execute a single tool call and return the result string.
    async fn execute_tool_call(&self, tc: &LlmToolCall) -> String {
        let args: serde_json::Value = serde_json::from_str(&tc.function.arguments)
            .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));

        eprintln!("  {} {}", "⚙".dimmed(), format!("{}({})", tc.function.name, &tc.function.arguments[..tc.function.arguments.len().min(80)]).dimmed());

        let output = self.tools.execute(&tc.function.name, args).await;

        if output.is_error {
            eprintln!("  {} {}", "✗".red(), output.content[..output.content.len().min(200)].dimmed());
        } else {
            eprintln!("  {} {}", "✓".green(), format!("{}...ok", tc.function.name).dimmed());
        }

        output.content
    }
}
