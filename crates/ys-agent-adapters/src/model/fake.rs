use std::{future::Future, pin::Pin, sync::Arc};

use async_trait::async_trait;
use ys_agent_core::{CoreResult, ModelCapabilities, ModelProvider, ModelRequest, ModelResponse};

use super::required_capabilities;

type ResponseFuture = Pin<Box<dyn Future<Output = CoreResult<ModelResponse>> + Send>>;
type Responder = dyn Fn(ModelRequest) -> ResponseFuture + Send + Sync;

#[derive(Clone)]
pub struct FakeModelProvider {
    responder: Arc<Responder>,
    capabilities: ModelCapabilities,
}

impl FakeModelProvider {
    pub fn new<F, Fut>(responder: F) -> Self
    where
        F: Fn(ModelRequest) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = CoreResult<ModelResponse>> + Send + 'static,
    {
        Self {
            responder: Arc::new(move |request| Box::pin(responder(request))),
            capabilities: required_capabilities(128_000),
        }
    }
}

#[async_trait]
impl ModelProvider for FakeModelProvider {
    fn capabilities(&self) -> ModelCapabilities {
        self.capabilities.clone()
    }

    async fn complete(&self, request: ModelRequest) -> CoreResult<ModelResponse> {
        (self.responder)(request).await
    }
}
