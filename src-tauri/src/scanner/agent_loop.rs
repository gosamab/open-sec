//! Shared agent-loop driver for detect/verify/patch. Each stage hands in a
//! "submission tool" whose JSON-schema input is the structured final answer;
//! Anthropic validates the input server-side, so malformed JSON is impossible
//! by construction. Read-only tools (read_file, grep, ...) sit alongside the
//! submission tool so the model can investigate before submitting.

use std::path::Path;

use anyhow::{anyhow, Context, Result};
use serde_json::Value;
use tracing::{debug, info, warn};

use crate::providers::{
    CacheControl, ContentBlock, GenerationRequest, Message, Provider, Role, SystemBlock, Tool,
};
use crate::tools;

/// Hard cap on tool-use round-trips per stage call. Beyond this we abort
/// rather than burn unbounded tokens on a model that won't converge.
pub(super) const MAX_TOOL_ITERATIONS: usize = 25;

pub(super) struct AgentRequest<'a> {
    pub system_prompt: String,
    pub initial_user_msg: String,
    pub model: &'a str,
    pub max_tokens: u32,
    /// `None` for verify/patch (Opus 4.7 rejects `temperature`; see CLAUDE.md).
    pub temperature: Option<f32>,
    pub canonical_root: &'a Path,
    pub provider: &'a dyn Provider,
    /// Used in tracing fields and the iteration-cap error message
    /// (e.g. "detect", "verifier", "patcher").
    pub stage_label: &'static str,
}

/// Run the agent loop until the model submits via `terminal_tool`. Returns
/// that tool call's `input` (already schema-validated by Anthropic).
///
/// If the model ends its turn without calling the terminal tool, that's a
/// stage failure — the prompt instructs the model to always submit.
pub(super) async fn run_agent_loop(req: AgentRequest<'_>, terminal_tool: Tool) -> Result<Value> {
    let AgentRequest {
        system_prompt,
        initial_user_msg,
        model,
        max_tokens,
        temperature,
        canonical_root,
        provider,
        stage_label,
    } = req;

    let terminal_name = terminal_tool.name.clone();
    let tool_defs: Vec<Tool> = tools::tool_definitions_with_terminal(terminal_tool);
    let mut messages: Vec<Message> = vec![Message {
        role: Role::User,
        content: vec![ContentBlock::Text {
            text: initial_user_msg,
        }],
    }];

    for iteration in 0..MAX_TOOL_ITERATIONS {
        let mut gen_req = GenerationRequest::new(model, max_tokens);
        gen_req.temperature = temperature;
        gen_req.system.push(
            SystemBlock::text(system_prompt.clone()).with_cache(CacheControl::ephemeral_1h()),
        );
        gen_req.tools = tool_defs.clone();
        gen_req.messages = messages.clone();

        let resp = provider
            .generate(gen_req)
            .await
            .context("anthropic generate call failed")?;

        debug!(
            stage = stage_label,
            iteration,
            stop_reason = ?resp.stop_reason,
            input_tokens = resp.usage.input_tokens,
            output_tokens = resp.usage.output_tokens,
            cache_read = resp.usage.cache_read_input_tokens,
            "agent iteration"
        );

        messages.push(Message {
            role: Role::Assistant,
            content: resp.content.clone(),
        });

        let mut terminal_input: Option<Value> = None;
        let mut read_tool_uses: Vec<(String, String, Value)> = Vec::new();
        for block in &resp.content {
            if let ContentBlock::ToolUse { id, name, input } = block {
                if name == &terminal_name {
                    terminal_input = Some(input.clone());
                } else {
                    read_tool_uses.push((id.clone(), name.clone(), input.clone()));
                }
            }
        }

        // Terminal tool short-circuits the loop; read tools called in the same
        // turn are intentionally not dispatched (the conversation is over).
        if let Some(input) = terminal_input {
            info!(stage = stage_label, iteration, "agent submitted");
            return Ok(input);
        }

        if read_tool_uses.is_empty() {
            warn!(
                stage = stage_label,
                stop_reason = ?resp.stop_reason,
                "agent ended turn without calling the submission tool"
            );
            return Err(anyhow!(
                "{stage_label} ended its turn without calling `{terminal_name}`"
            ));
        }

        info!(
            stage = stage_label,
            iteration,
            tool_calls = read_tool_uses.len(),
            "agent tool calls"
        );

        let mut tool_results: Vec<ContentBlock> = Vec::with_capacity(read_tool_uses.len());
        for (id, name, input) in &read_tool_uses {
            let (content, is_error) = match tools::dispatch(name, input, canonical_root).await {
                Ok(s) => (s, false),
                Err(e) => (format!("error: {e:#}"), true),
            };
            tool_results.push(ContentBlock::ToolResult {
                tool_use_id: id.clone(),
                content,
                is_error,
            });
        }
        messages.push(Message {
            role: Role::User,
            content: tool_results,
        });
    }

    Err(anyhow!(
        "{stage_label} hit the {MAX_TOOL_ITERATIONS}-iteration tool-use cap without calling `{terminal_name}`"
    ))
}
