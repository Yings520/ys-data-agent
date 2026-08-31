mod context_assembler;
mod coordinator;
pub mod doctor;
pub mod export;
mod harness;
mod loop_driver;
mod recovery;
mod service;

pub mod workflow;

pub use context_assembler::{
    AssembledContext, ContextAssembler, ContextAssemblyRequest, ContextManifestArtifactWriter,
    InMemoryQueryContextProvider, PersistContextIdentity, PreparedContextManifest, PromptBuilder,
    RecentTaskSummary, RetrievalNeed, ToolViewSnapshot,
};

pub use coordinator::{CoordinationDecision, Coordinator, FutureWorkflow, RuleBasedCoordinator};
pub use harness::{Harness, HarnessConfig, HarnessDependencies};
pub use loop_driver::{
    HarnessStep, LoopBudget, LoopDriver, LoopResult, LoopUsage, StepAccounting, StepOutcome,
};
pub use recovery::{RecoveryDecision, RecoveryManager, RecoveryRequest};
pub use service::{
    AgentServiceApi, ArtifactView, CreateTaskRequest, EventSubscription, InProcessAgentService,
    NoopRunScheduler, RunScheduler, SendMessageRequest, ServiceEvent, ServiceEventPublisher,
    ServiceReply,
};
pub use workflow::query::{
    ClarificationNeed, FreshnessState, MetricReference, ParameterKind, QUERY_SYSTEM_PROMPT_VERSION,
    QueryArtifact, QueryArtifactInput, QueryVerifier, QueryWorkflow, QueryWorkflowState,
    RedactedParameter, ResultColumn, ResultSchema, VerificationCheck, VerificationInput,
    VerificationReport, WorkflowDirective, WorkflowEffect, classify_intent, material_ambiguity,
    query_system_instructions, requires_current_freshness,
};
pub mod tools;
