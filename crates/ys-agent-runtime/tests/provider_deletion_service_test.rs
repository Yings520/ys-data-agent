use std::sync::Arc;

use tempfile::TempDir;
use ys_agent_adapters::credential::keyring::{InMemoryCredentialVault, InMemoryVaultOperation};
use ys_agent_core::{
    ActiveRevisionPrecondition, CommandId, CommandReceipt, CommandResultKind, CreateRunCommand,
    CredentialVault, DeleteProfileRequest, EventActor, OperationId, PendingRunEvent,
    ProtectedCredentialWrite, ProviderCredentialReference, ProviderErrorCode, Run, RunEventKind,
    RunProviderBinding, RunProviderBindingSource, RuntimeCommandBatch, RuntimeStore, SecretValue,
    Task, WorkflowKind, WorkspaceId,
};
use ys_agent_runtime::{
    UnavailableRunProviderBindingSource, provider::service::ProviderManagementService,
};
use ys_agent_store::SqliteRuntimeStore;

#[path = "support/provider_fixture.rs"]
mod provider_fixture;

fn delete_request(
    active: &ys_agent_core::ActiveProviderSnapshot,
    enter_no_active_provider: bool,
) -> DeleteProfileRequest {
    DeleteProfileRequest {
        operation_id: OperationId::new(),
        profile_id: active.profile_id(),
        expected_revision: active.profile_revision(),
        expected_active: Some(ActiveRevisionPrecondition {
            profile_id: active.profile_id(),
            revision: active.profile_revision(),
            activation_revision: active.activation_revision(),
        }),
        enter_no_active_provider,
    }
}

async fn seeded_service() -> (
    SqliteRuntimeStore,
    ys_agent_core::ActiveProviderSnapshot,
    Arc<InMemoryCredentialVault>,
    ProviderManagementService,
) {
    let directory = TempDir::new().expect("temporary database directory");
    let database = directory.keep().join("runtime.db");
    let store = SqliteRuntimeStore::open(database)
        .await
        .expect("open database");
    let active = provider_fixture::persisted_test_active_provider(&store).await;
    let current = store
        .provider_repository()
        .load_current_revision(active.profile_id())
        .await
        .expect("load active revision");
    let generation = current
        .credential_generation()
        .expect("active revision has credential");
    let vault = Arc::new(InMemoryCredentialVault::new());
    vault
        .write_generation(ProtectedCredentialWrite {
            reference: ProviderCredentialReference {
                profile_id: active.profile_id(),
                generation,
            },
            secret: SecretValue::from_utf8("deletion-test-secret".to_owned()),
        })
        .await
        .expect("seed protected credential");
    let service = ProviderManagementService::new(Arc::new(store.provider_repository()));
    (store, active, vault, service)
}

async fn create_nonterminal_run(
    store: &SqliteRuntimeStore,
    active: ys_agent_core::ActiveProviderSnapshot,
) {
    let task = Task::new(
        WorkspaceId::new(),
        ys_agent_core::PrincipalId::new(),
        "Profile deletion guard",
    );
    let run = Run::new(task.id, WorkflowKind::Query);
    let binding = RunProviderBinding::from_active(run.id, active).expect("bind exact active");
    let snapshot = run.snapshot(serde_json::json!({"phase": "created"}), None, None, None);
    let command_id = CommandId::new();
    let command_fingerprint = format!("profile-deletion:{command_id}");
    store
        .commit_command(RuntimeCommandBatch {
            command_id,
            command_fingerprint: command_fingerprint.clone(),
            receipt: CommandReceipt {
                command_id,
                command_fingerprint,
                result_kind: CommandResultKind::RunStarted,
                session_id: None,
                task_id: Some(task.id),
                run_id: Some(run.id),
                artifact_id: None,
                message: None,
                capability: None,
            },
            new_session: None,
            new_task: Some(task),
            create_run: Some(
                CreateRunCommand::new(
                    snapshot,
                    binding,
                    vec![PendingRunEvent {
                        actor: EventActor::System,
                        kind: RunEventKind::RunStarted,
                    }],
                )
                .expect("complete Run command"),
            ),
            new_artifact: None,
            pending_events: Vec::new(),
            snapshot_update: None,
        })
        .await
        .expect("persist nonterminal Run and immutable binding");
}

#[tokio::test]
async fn deleting_an_active_profile_requires_explicit_no_active_confirmation_and_removes_its_credential()
 {
    let (store, active, vault, service) = seeded_service().await;
    let run_bindings = store.run_binding_repository();

    let error = service
        .delete_profile(
            delete_request(&active, false),
            vault.as_ref(),
            &run_bindings,
        )
        .await
        .expect_err("active Profile deletion requires an explicit no-active confirmation");
    assert_eq!(
        error.code(),
        ProviderErrorCode::ActivationPreconditionFailed.as_str()
    );
    assert!(
        service
            .active_snapshot()
            .await
            .expect("active read")
            .is_some()
    );

    service
        .delete_profile(delete_request(&active, true), vault.as_ref(), &run_bindings)
        .await
        .expect("confirmed deletion enters no-active after removing the protected credential");
    assert!(
        service
            .active_snapshot()
            .await
            .expect("no-active read")
            .is_none()
    );
    assert!(
        service
            .list_profiles()
            .await
            .expect("deleted Profile is hidden from management")
            .is_empty()
    );
    let no_active = UnavailableRunProviderBindingSource
        .bind_new_run(ys_agent_core::RunId::new())
        .await
        .expect_err("new Query must not select another Profile when no active exists");
    assert_eq!(
        no_active.code(),
        ProviderErrorCode::NoActiveProfile.as_str()
    );
}

#[tokio::test]
async fn vault_delete_failure_preserves_the_profile_and_active_snapshot() {
    let (store, active, vault, service) = seeded_service().await;
    let run_bindings = store.run_binding_repository();
    vault.fail_next(InMemoryVaultOperation::Delete);

    let _ = service
        .delete_profile(delete_request(&active, true), vault.as_ref(), &run_bindings)
        .await
        .expect_err("Vault deletion failure leaves the durable Profile untouched");
    assert_eq!(
        service
            .active_snapshot()
            .await
            .expect("active read")
            .expect("active remains")
            .profile_revision(),
        active.profile_revision()
    );
    assert_eq!(
        service
            .list_profiles()
            .await
            .expect("Profile remains browseable")
            .len(),
        1
    );
}

#[tokio::test]
async fn nonterminal_run_reference_rejects_deletion_before_touching_the_vault() {
    let (store, active, vault, service) = seeded_service().await;
    create_nonterminal_run(&store, active.clone()).await;
    let run_bindings = store.run_binding_repository();

    let error = service
        .delete_profile(delete_request(&active, true), vault.as_ref(), &run_bindings)
        .await
        .expect_err("nonterminal Run binding retains its Profile and credential");
    assert_eq!(error.code(), ProviderErrorCode::OperationStale.as_str());
    assert_eq!(
        service
            .list_profiles()
            .await
            .expect("Profile remains browseable")
            .len(),
        1
    );
    assert!(
        service
            .active_snapshot()
            .await
            .expect("active read")
            .is_some()
    );
}
