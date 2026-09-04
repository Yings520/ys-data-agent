//! Black-box secret canary checks for Provider-management output surfaces.

use std::{
    fs,
    path::Path,
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use tempfile::TempDir;
use ys_agent_adapters::credential::memory::InMemoryCredentialVault;
use ys_agent_core::{
    ArtifactKind, ArtifactStore, CredentialGeneration, CredentialKind, CredentialMutation,
    CredentialMutationIntent, CredentialMutationRequest, CredentialProtectionStatus, OperationId,
    ProfileId, ProfileName, ProfileRevision, ProtectedCredentialWrite, ProviderCredentialReference,
    ProviderId, ProviderModelId, ProviderParameters, PutArtifact, RevisionPrecondition, RunId,
    SaveProfileRequest, SaveProfileRevision, SecretValue, Sensitivity, TaskId, WorkspaceId,
};
use ys_agent_runtime::{
    provider::service::{CredentialService, ProviderManagementService},
    telemetry::{
        ProviderTelemetryOutcome, TelemetryDispatcher, TelemetryError, TelemetryEvent,
        TelemetrySink,
    },
};
use ys_agent_store::{LocalArtifactStore, SqliteRuntimeStore};

const CANARY: &str = "provider-cross-surface-secret-canary-must-not-leak";

#[derive(Clone, Default)]
struct RecordingTelemetrySink {
    events: Arc<Mutex<Vec<String>>>,
}

#[async_trait]
impl TelemetrySink for RecordingTelemetrySink {
    async fn emit(&self, event: TelemetryEvent) -> Result<(), TelemetryError> {
        self.events
            .lock()
            .expect("telemetry test state")
            .push(serde_json::to_string(&event).map_err(|_| TelemetryError::Encoding)?);
        Ok(())
    }
}

impl RecordingTelemetrySink {
    fn text(&self) -> String {
        self.events.lock().expect("telemetry test state").join("\n")
    }
}

fn draft(profile_id: ProfileId) -> ProfileRevision {
    ProfileRevision::draft(
        profile_id,
        1,
        ProviderId::DeepSeek,
        ProviderModelId::new(ProviderId::DeepSeek, "deepseek/canary-model")
            .expect("governed model"),
        ProviderParameters::default(),
        None,
    )
    .expect("valid Draft")
}

async fn save_draft(profiles: &ProviderManagementService, profile_id: ProfileId, name: &str) {
    profiles
        .save_profile(SaveProfileRequest {
            operation_id: OperationId::new(),
            revision: SaveProfileRevision {
                precondition: RevisionPrecondition {
                    profile_id,
                    expected_current_revision: None,
                },
                name: ProfileName::new(name).expect("Profile name"),
                revision: draft(profile_id),
            },
        })
        .await
        .expect("save Draft without a Credential");
}

fn write(profile_id: ProfileId, generation: CredentialGeneration) -> ProtectedCredentialWrite {
    ProtectedCredentialWrite {
        reference: ProviderCredentialReference {
            profile_id,
            generation,
        },
        secret: SecretValue::from_utf8(CANARY.to_owned()),
    }
}

async fn mutate_with_canary(
    service: &CredentialService,
    profile_id: ProfileId,
    generation: CredentialGeneration,
) -> ys_agent_core::ProviderResult<ys_agent_core::ProfileDetail> {
    service
        .mutate(CredentialMutationRequest {
            intent: CredentialMutationIntent::create(OperationId::new(), profile_id, 1, generation)
                .expect("valid credential creation"),
            mutation: CredentialMutation::Replace(write(profile_id, generation)),
        })
        .await
}

fn assert_directory_has_no_canary(root: &Path) {
    for entry in fs::read_dir(root).expect("read output root") {
        let entry = entry.expect("output directory entry");
        let path = entry.path();
        if entry.file_type().expect("output file type").is_dir() {
            assert_directory_has_no_canary(&path);
        } else {
            let bytes = fs::read(&path).expect("read output file");
            let text = String::from_utf8_lossy(&bytes);
            assert!(
                !text.contains(CANARY),
                "Provider secret canary escaped into {}",
                path.display()
            );
        }
    }
}

#[tokio::test]
async fn credential_canary_is_absent_from_persistence_views_errors_telemetry_and_artifacts() {
    let directory = TempDir::new().expect("temporary Provider output root");
    let database = directory.path().join("runtime.db");
    let store = SqliteRuntimeStore::open(&database)
        .await
        .expect("open SQLite persistence");
    let repository = store.provider_repository();
    let profiles = ProviderManagementService::new(Arc::new(repository.clone()));
    let vault = Arc::new(InMemoryCredentialVault::new());
    let profile_id = ProfileId::new();
    let generation =
        CredentialGeneration::new(profile_id, 1, CredentialKind::ApiKey).expect("first generation");
    save_draft(&profiles, profile_id, "Canary profile").await;

    let credentials = CredentialService::new(
        Arc::new(repository.clone()),
        Arc::new(store.run_binding_repository()),
        vault,
    );
    let detail = mutate_with_canary(&credentials, profile_id, generation)
        .await
        .expect("only the protected vault receives the canary");
    let journal = repository
        .pending_credential_mutations()
        .await
        .expect("journal");
    let rendered_views = format!(
        "{detail:?}\n{}\n{journal:?}",
        serde_json::to_string(&profiles.list_profiles().await.expect("masked list"))
            .expect("serialize masked list"),
    );
    assert!(!rendered_views.contains(CANARY));

    let blocked_profile = ProfileId::new();
    save_draft(&profiles, blocked_profile, "Blocked canary profile").await;
    let blocked_generation = CredentialGeneration::new(blocked_profile, 1, CredentialKind::ApiKey)
        .expect("blocked generation");
    let unavailable = CredentialService::new(
        Arc::new(repository),
        Arc::new(store.run_binding_repository()),
        Arc::new(InMemoryCredentialVault::with_protection(
            CredentialProtectionStatus::Unconfirmed,
        )),
    );
    let error = mutate_with_canary(&unavailable, blocked_profile, blocked_generation)
        .await
        .expect_err("protection failure must not echo a Credential");
    assert!(!error.to_string().contains(CANARY));
    assert!(!format!("{error:?}").contains(CANARY));

    let sink = RecordingTelemetrySink::default();
    TelemetryDispatcher::new(Arc::new(sink.clone()))
        .emit_after_commit(TelemetryEvent::ProviderCall {
            provider: ProviderId::DeepSeek,
            fingerprint_sha256: CANARY.to_owned(),
            milliseconds: 1,
            retry_count: 0,
            outcome: ProviderTelemetryOutcome::Failed,
        })
        .await;
    assert!(!sink.text().contains(CANARY));

    LocalArtifactStore::new(directory.path())
        .expect("create owner-only Artifact directory")
        .put(PutArtifact {
            workspace_id: WorkspaceId::new(),
            task_id: TaskId::new(),
            run_id: RunId::new(),
            kind: ArtifactKind::VerificationReport,
            media_type: "application/json".to_owned(),
            bytes: br#"{"provider":"deep_seek","credential":"[REDACTED]"}"#.to_vec(),
            sensitivity: Sensitivity::Internal,
            owner: None,
            retention_policy: None,
            expires_at: None,
            producer_step_id: None,
        })
        .await
        .expect("persist only a sanitized Artifact fixture");
    assert_directory_has_no_canary(directory.path());
}
