use std::sync::Arc;

use async_trait::async_trait;
use tempfile::TempDir;
use tokio::sync::Mutex;
use ys_agent_core::{
    CommandId, CoreResult, EventActor, PendingRunEvent, Principal, RunEventKind, RunId,
    RuntimeStore, StepId, TaskStatus, WorkspaceId,
};
use ys_agent_runtime::{
    AgentServiceApi, CoordinationDecision, Coordinator, CreateTaskRequest, InProcessAgentService,
    RuleBasedCoordinator, RunScheduler, SendMessageRequest, ServiceReply,
};
use ys_agent_store::{LocalArtifactStore, SqliteRuntimeStore};

#[derive(Default)]
struct CountingScheduler {
    scheduled: Mutex<Vec<RunId>>,
}

impl CountingScheduler {
    async fn count(&self) -> usize {
        self.scheduled.lock().await.len()
    }
}

#[async_trait]
impl RunScheduler for CountingScheduler {
    async fn schedule(&self, run_id: RunId) -> CoreResult<()> {
        let mut scheduled = self.scheduled.lock().await;
        if !scheduled.contains(&run_id) {
            scheduled.push(run_id);
        }
        Ok(())
    }
}

struct ServiceFixture {
    _directory: TempDir,
    store: Arc<SqliteRuntimeStore>,
    service: Arc<InProcessAgentService>,
    coordinator: RuleBasedCoordinator,
    scheduler: Arc<CountingScheduler>,
    principal: Principal,
    session_id: ys_agent_core::SessionId,
}

impl ServiceFixture {
    async fn new() -> Self {
        let directory = tempfile::tempdir().expect("temporary directory");
        let store = Arc::new(
            SqliteRuntimeStore::open(directory.path().join("runtime.db"))
                .await
                .expect("runtime store"),
        );
        let artifacts =
            Arc::new(LocalArtifactStore::new(directory.path()).expect("local artifact store"));
        let scheduler = Arc::new(CountingScheduler::default());
        let workspace_id = WorkspaceId::new();
        let service = Arc::new(InProcessAgentService::with_event_capacity(
            workspace_id,
            store.clone(),
            artifacts,
            scheduler.clone(),
            2,
        ));
        let principal = Principal::local_operator("Data Engineer");
        let session = service
            .create_session(CommandId::new(), principal.clone())
            .await
            .expect("default session");
        Self {
            _directory: directory,
            store,
            service,
            coordinator: RuleBasedCoordinator,
            scheduler,
            principal,
            session_id: session.id,
        }
    }

    fn principal(&self) -> Principal {
        self.principal.clone()
    }

    fn session_id(&self) -> ys_agent_core::SessionId {
        self.session_id
    }

    async fn created_run_count(&self) -> u64 {
        self.store.run_count().await.expect("count runs")
    }
}

#[tokio::test]
async fn new_session_does_not_cancel_existing_tasks() {
    let fixture = ServiceFixture::new().await;
    let session_one = fixture
        .service
        .create_session(CommandId::new(), fixture.principal())
        .await
        .expect("first session");
    let task = fixture
        .service
        .create_task(CreateTaskRequest {
            command_id: CommandId::new(),
            session_id: session_one.id,
            goal: "Query GMV".to_owned(),
            acceptance_criteria: vec![],
        })
        .await
        .expect("create task");

    let session_two = fixture
        .service
        .create_session(CommandId::new(), fixture.principal())
        .await
        .expect("second session");

    assert_ne!(session_one.id, session_two.id);
    assert_eq!(
        fixture
            .service
            .get_task(&task.id)
            .await
            .expect("load task")
            .status,
        TaskStatus::Open
    );
}

#[tokio::test]
async fn unrelated_input_creates_a_new_task() {
    let fixture = ServiceFixture::new().await;
    let session = fixture
        .service
        .create_session(CommandId::new(), fixture.principal())
        .await
        .expect("session");
    let gmv = fixture
        .service
        .create_task(CreateTaskRequest {
            command_id: CommandId::new(),
            session_id: session.id,
            goal: "Query GMV".to_owned(),
            acceptance_criteria: vec![],
        })
        .await
        .expect("task");

    let decision = fixture
        .coordinator
        .route(&session, Some(&gmv), "Query yesterday's DAU")
        .await
        .expect("route");

    assert!(matches!(
        decision,
        CoordinationDecision::CreateNewTask { .. }
    ));
}

#[tokio::test]
async fn unsupported_workflow_is_explicit_and_creates_no_run() {
    let fixture = ServiceFixture::new().await;
    let reply = fixture
        .service
        .send_message(SendMessageRequest {
            command_id: CommandId::new(),
            session_id: fixture.session_id(),
            focused_task_id: None,
            text: "Explain why GMV fell and change the dbt model".to_owned(),
        })
        .await
        .expect("unsupported reply");

    assert!(matches!(reply, ServiceReply::UnsupportedCapability { .. }));
    assert_eq!(fixture.created_run_count().await, 0);
}

#[tokio::test]
async fn repeated_send_message_command_creates_only_one_run() {
    let fixture = ServiceFixture::new().await;
    let command_id = CommandId::new();
    let request = SendMessageRequest::new(
        command_id,
        fixture.session_id(),
        "GMV for the last seven complete days",
    );

    let first = fixture
        .service
        .send_message(request.clone())
        .await
        .expect("first message");
    let second = fixture
        .service
        .send_message(request)
        .await
        .expect("replayed message");

    assert_eq!(first.run_id(), second.run_id());
    assert_eq!(fixture.created_run_count().await, 1);
    assert_eq!(fixture.scheduler.count().await, 1);
}

#[tokio::test]
async fn subscription_reads_durable_events_before_live_notifications() {
    let fixture = ServiceFixture::new().await;
    let reply = fixture
        .service
        .send_message(SendMessageRequest::new(
            CommandId::new(),
            fixture.session_id(),
            "Query GMV",
        ))
        .await
        .expect("send message");
    let run_id = reply.run_id().expect("scheduled run");

    let mut subscription = fixture
        .service
        .subscribe_events(&run_id, 0)
        .await
        .expect("subscribe");
    let event = subscription.next().await.expect("durable event");

    assert_eq!(event.sequence, 1);
    assert!(matches!(event.event.kind, RunEventKind::RunStarted));
}

#[tokio::test]
async fn lagged_subscription_reloads_from_the_durable_sequence() {
    let fixture = ServiceFixture::new().await;
    let reply = fixture
        .service
        .send_message(SendMessageRequest::new(
            CommandId::new(),
            fixture.session_id(),
            "Query GMV",
        ))
        .await
        .expect("send message");
    let run_id = reply.run_id().expect("scheduled run");
    let mut subscription = fixture
        .service
        .subscribe_events(&run_id, 1)
        .await
        .expect("subscribe after RunStarted");

    let current = fixture.store.load_run(&run_id).await.expect("load run");
    let mut next = current.clone();
    next.version += 1;
    fixture
        .store
        .append(
            &run_id,
            current.version,
            vec![
                PendingRunEvent {
                    actor: EventActor::System,
                    kind: RunEventKind::StepStarted {
                        step_id: StepId::new(),
                        label: "plan".to_owned(),
                    },
                },
                PendingRunEvent {
                    actor: EventActor::System,
                    kind: RunEventKind::StepStarted {
                        step_id: StepId::new(),
                        label: "verify".to_owned(),
                    },
                },
            ],
            &next,
        )
        .await
        .expect("append durable events");

    let publisher = fixture.service.event_publisher();
    for sequence in 2..10 {
        publisher.notify(run_id, sequence);
    }
    let recovered = subscription.next().await.expect("reloaded event");

    assert_eq!(recovered.sequence, 2);
    assert_eq!(subscription.last_sequence(), 2);
}
