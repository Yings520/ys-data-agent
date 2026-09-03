pub mod discovery;
mod fake;
pub mod liter;
pub mod liter_chat;
pub mod liter_responses;
mod replay;

use ys_agent_core::ModelCapabilities;

pub use fake::FakeModelProvider;
pub use replay::ReplayModelProvider;

pub(super) fn required_capabilities(context_window_tokens: u64) -> ModelCapabilities {
    ModelCapabilities {
        tool_calling: true,
        structured_outputs: true,
        max_context_tokens: context_window_tokens as u32,
        parallel_tool_calls: false,
        streaming: false,
    }
}
