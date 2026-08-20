use std::{collections::VecDeque, sync::Arc};

use async_trait::async_trait;
use tokio::sync::Mutex;
use ys_agent_core::{
    CoreError, CoreResult, ModelCapabilities, ModelProvider, ModelRequest, ModelResponse,
};

use super::required_capabilities;

#[derive(Clone)]
pub struct ReplayModelProvider {
    responses: Arc<Mutex<VecDeque<ModelResponse>>>,
    capabilities: ModelCapabilities,
}

impl ReplayModelProvider {
    pub fn from_responses(responses: Vec<ModelResponse>) -> Self {
        Self {
            responses: Arc::new(Mutex::new(responses.into())),
            capabilities: required_capabilities(128_000),
        }
    }
}

#[async_trait]
impl ModelProvider for ReplayModelProvider {
    fn capabilities(&self) -> ModelCapabilities {
        self.capabilities.clone()
    }

    async fn complete(&self, _request: ModelRequest) -> CoreResult<ModelResponse> {
        self.responses
            .lock()
            .await
            .pop_front()
            .ok_or(CoreError::ReplayExhausted)
    }
}
