mod context_assembler;
mod coordinator;
mod service;

pub use context_assembler::{
    AssembledContext, ContextAssembler, ContextAssemblyRequest, ContextManifestArtifactWriter,
    InMemoryQueryContextProvider, PersistContextIdentity, PreparedContextManifest, PromptBuilder,
    RecentTaskSummary, RetrievalNeed, ToolViewSnapshot,
};

pub use coordinator::{CoordinationDecision, Coordinator, FutureWorkflow, RuleBasedCoordinator};
pub use service::{
    AgentServiceApi, ArtifactView, CreateTaskRequest, EventSubscription, InProcessAgentService,
    NoopRunScheduler, RunScheduler, SendMessageRequest, ServiceEvent, ServiceEventPublisher,
    ServiceReply,
};
pub mod tools;
