//! Shared agent-loop driver for the detect/verify/patch stages. Each stage
//! used to inline its own near-identical copy of this loop; the only real
//! variation across the three was the temperature setting, the stage label
//! in logs/errors, and how the final assistant text was parsed.
//!
//! Callers build an `AgentRequest`, hand it to `run_agent_loop`, and parse
//! the returned final-message text however they need to.

use std::path::Path;

use anyhow::{anyhow, Context, Result};
use tracing::{debug, info, warn};

use crate::providers::{
    CacheControl, ContentBlock, GenerationRequest, Message, Provider, Role, StopReason,
    SystemBlock, Tool,
};
use crate::scanner::util::collect_text;
use crate::tools;

/// Hard cap on tool-use round-trips per stage call. Beyond this we abort
/// rather than burn unbounded tokens on a model that won't converge.
pub(super) const MAX_TOOL_ITERATIONS: usize = 25;

/// Inputs for one agentic stage call. Construct one per file/finding and
/// pass it to [`run_agent_loop`].
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

/// Run the tool-use conversation until the model returns an assistant turn
/// with no tool calls, then return the joined text of that final turn.
/// Errors out at [`MAX_TOOL_ITERATIONS`] or on any provider failure.
pub(super) async fn run_agent_loop(req: AgentRequest<'_>) -> Result<String> {
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

    let tool_defs: Vec<Tool> = tools::tool_definitions();
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

        // Always append the assistant turn first so any tool_use blocks are
        // referenced by id in the next user turn's tool_result blocks.
        messages.push(Message {
            role: Role::Assistant,
            content: resp.content.clone(),
        });

        let tool_uses: Vec<(String, String, serde_json::Value)> = resp
            .content
            .iter()
            .filter_map(|b| match b {
                ContentBlock::ToolUse { id, name, input } => {
                    Some((id.clone(), name.clone(), input.clone()))
                }
                _ => None,
            })
            .collect();

        if tool_uses.is_empty() {
            if !matches!(resp.stop_reason, Some(StopReason::EndTurn) | None) {
                warn!(
                    stage = stage_label,
                    stop_reason = ?resp.stop_reason,
                    "no tool calls but non-end_turn stop reason"
                );
            }
            return Ok(collect_text(&resp.content));
        }

        info!(
            stage = stage_label,
            iteration,
            tool_calls = tool_uses.len(),
            "agent tool calls"
        );

        let mut tool_results: Vec<ContentBlock> = Vec::with_capacity(tool_uses.len());
        for (id, name, input) in &tool_uses {
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
        "{stage_label} hit the {MAX_TOOL_ITERATIONS}-iteration tool-use cap without a final answer"
    ))
}
