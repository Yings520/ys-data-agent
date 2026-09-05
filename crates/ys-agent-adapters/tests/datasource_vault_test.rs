use ys_agent_adapters::credential::datasource::LocalEncryptedDatasourceVault;
use ys_agent_core::{
    DatasourceSecretRef, DatasourceVault, DsErrorCode, ProfileId, ProtectionStatus, SecretValue,
    WorkspaceId,
};

#[tokio::test]
async fn datasource_vault_is_immutable_private_and_bound_to_the_full_identity() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap().join("vault");
    let vault = LocalEncryptedDatasourceVault::new(&root);
    assert_eq!(
        vault.protection().await.unwrap(),
        ProtectionStatus::OwnerOnlyEncryptedFile
    );
    let reference = DatasourceSecretRef::new(WorkspaceId::new(), ProfileId::new(), 1).unwrap();
    vault
        .write(
            reference,
            SecretValue::from_utf8("datasource-canary".into()),
        )
        .await
        .unwrap();
    assert_eq!(
        vault
            .write(reference, SecretValue::from_utf8("replacement".into()))
            .await
            .unwrap_err()
            .code,
        DsErrorCode::Conflict
    );
    let reopened = LocalEncryptedDatasourceVault::new(&root);
    assert_eq!(
        reopened
            .read(reference)
            .await
            .unwrap()
            .value
            .with_exposed(str::to_owned),
        "datasource-canary"
    );
    let entry = root.join("datasource-credentials").join(format!(
        "{}-{}-1.credential",
        reference.workspace_id(),
        reference.profile_id()
    ));
    let ciphertext = std::fs::read(&entry).unwrap();
    assert!(!String::from_utf8_lossy(&ciphertext).contains("datasource-canary"));
    let other = DatasourceSecretRef::new(WorkspaceId::new(), reference.profile_id(), 1).unwrap();
    let other_entry = root.join("datasource-credentials").join(format!(
        "{}-{}-1.credential",
        other.workspace_id(),
        other.profile_id()
    ));
    std::fs::copy(&entry, &other_entry).unwrap();
    assert!(
        reopened.read(other).await.is_err(),
        "AAD must reject cross-workspace substitution"
    );
    reopened.remove(reference).await.unwrap();
    reopened.remove(reference).await.unwrap();
    assert!(reopened.read(reference).await.is_err());
}

#[cfg(unix)]
#[tokio::test]
async fn datasource_vault_refuses_symlinks_and_unproven_permissions_without_repairing_them() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let directory = tempfile::tempdir().unwrap();
    let base = directory.path().canonicalize().unwrap();
    let root = base.join("vault");
    let vault = LocalEncryptedDatasourceVault::new(&root);
    assert_eq!(
        vault.protection().await.unwrap(),
        ProtectionStatus::OwnerOnlyEncryptedFile
    );
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(
        vault.protection().await.unwrap(),
        ProtectionStatus::Unavailable
    );
    assert_eq!(
        std::fs::metadata(&root).unwrap().permissions().mode() & 0o777,
        0o755
    );
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
    let link = base.join("link");
    symlink(&root, &link).unwrap();
    assert_eq!(
        LocalEncryptedDatasourceVault::new(link)
            .protection()
            .await
            .unwrap(),
        ProtectionStatus::Unavailable
    );
    let reference = DatasourceSecretRef::new(WorkspaceId::new(), ProfileId::new(), 1).unwrap();
    vault
        .write(reference, SecretValue::from_utf8("canary".into()))
        .await
        .unwrap();
    let key = root.join("datasource-credentials.key");
    std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(vault.read(reference).await.is_err());
    assert_eq!(
        std::fs::metadata(key).unwrap().permissions().mode() & 0o777,
        0o644
    );
}

#[tokio::test]
async fn every_secret_journal_crash_phase_recovers_only_a_complete_old_or_new_state() {
    use ys_agent_core::*;
    use ys_agent_store::SqliteRuntimeStore;
    for phase in [
        SecretMutationPhase::Prepared,
        SecretMutationPhase::VaultWritten,
        SecretMutationPhase::Committed,
    ] {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let database = root.join("runtime.db");
        let scope = DatasourceScope {
            workspace_id: WorkspaceId::new(),
            session_id: SessionId::new(),
        };
        let profile_id = ProfileId::new();
        let reference = DatasourceSecretRef::new(scope.workspace_id, profile_id, 1).unwrap();
        let revision = DatasourceRevision::new(DatasourceRevisionInput {
            schema_version: 1,
            workspace_id: scope.workspace_id,
            profile_id,
            revision: 1,
            adapter_id: "postgres".try_into().unwrap(),
            adapter_version: "test".try_into().unwrap(),
            config_version: 1,
            source_id: None,
            fields: Default::default(),
            context: DatabaseContext::Unconfigured,
            credential: Some(reference),
        })
        .unwrap();
        let profile = DatasourceProfile {
            schema_version: 1,
            workspace_id: scope.workspace_id,
            profile_id,
            source_id: None,
            name: DatasourceName::new("Protected").unwrap(),
            head_revision: revision.identity().revision,
            deleted_at: None,
        };
        let mutation_id = OperationId::new();
        let write = DatasourceWriteContext {
            command_id: CommandId::new(),
            scope,
            expected_version: 0,
            expected_head_revision: None,
        };
        let change = DatasourceChange::SaveRevision {
            profile,
            revision,
            mutation_id: Some(mutation_id),
        };
        let command = DatasourceCommit {
            schema_version: 1,
            write,
            command_digest: DatasourceDigest::of(&change).unwrap(),
            change,
        };
        let mut mutation = SecretMutation {
            schema_version: 1,
            mutation_id,
            write,
            profile_id,
            old: None,
            new: Some(reference),
            phase: SecretMutationPhase::Prepared,
            command_digest: command.command_digest.clone(),
        };
        let journal = |m: SecretMutation| DatasourceCommit {
            schema_version: 1,
            write: m.write,
            command_digest: m.command_digest.clone(),
            change: DatasourceChange::SecretJournal { mutation: m },
        };
        let repository = SqliteRuntimeStore::open(&database)
            .await
            .unwrap()
            .datasource_repository();
        let vault = LocalEncryptedDatasourceVault::new(root.join("vault"));
        repository.commit(journal(mutation.clone())).await.unwrap();
        // Even Prepared may have a complete/partial file when the process died before phase acknowledgement.
        vault
            .write(
                reference,
                SecretValue::from_utf8("journal-secret-canary".into()),
            )
            .await
            .unwrap();
        if phase != SecretMutationPhase::Prepared {
            mutation.phase = SecretMutationPhase::VaultWritten;
            repository.commit(journal(mutation.clone())).await.unwrap();
        }
        if phase == SecretMutationPhase::Committed {
            repository.commit(command.clone()).await.unwrap();
        }
        drop(repository);
        drop(vault);
        let repository = SqliteRuntimeStore::open(&database)
            .await
            .unwrap()
            .datasource_repository();
        let vault = LocalEncryptedDatasourceVault::new(root.join("vault"));
        let pending = repository
            .pending_secret_mutations(scope.workspace_id)
            .await
            .unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].phase, phase);
        if phase == SecretMutationPhase::Committed {
            let receipt = repository.receipt(write.command_id).await.unwrap().unwrap();
            assert_eq!(
                repository.commit(command.clone()).await.unwrap(),
                receipt,
                "lost response replays the original receipt"
            );
            assert_eq!(repository.load(scope).await.unwrap(), receipt.snapshot);
            assert_eq!(
                vault
                    .read(reference)
                    .await
                    .unwrap()
                    .value
                    .with_exposed(str::to_owned),
                "journal-secret-canary"
            );
        } else {
            repository.claim_secret_cleanup(reference).await.unwrap();
            vault.remove(reference).await.unwrap();
            repository.finish_secret_cleanup(reference).await.unwrap();
            assert!(repository.load(scope).await.unwrap().profiles.is_empty());
            assert!(vault.read(reference).await.is_err());
        }
        repository
            .finish_secret_mutation(mutation_id)
            .await
            .unwrap();
        assert!(
            repository
                .pending_secret_mutations(scope.workspace_id)
                .await
                .unwrap()
                .is_empty()
        );
        let bytes = std::fs::read(database).unwrap();
        assert!(!String::from_utf8_lossy(&bytes).contains("journal-secret-canary"));
    }
}
