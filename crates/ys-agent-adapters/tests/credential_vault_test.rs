use ys_agent_adapters::credential::keyring::{
    InMemoryCredentialVault, InMemoryVaultOperation, KEYRING_SERVICE, KeyringCredentialVault,
};
use ys_agent_core::{
    CredentialGeneration, CredentialKind, CredentialProtectionStatus, CredentialVault,
    CredentialViewStatus, ProfileId, ProtectedCredentialWrite, ProviderCredentialReference,
    SecretValue,
};

fn reference(
    profile_id: ProfileId,
    generation: u64,
    kind: CredentialKind,
) -> ProviderCredentialReference {
    ProviderCredentialReference {
        profile_id,
        generation: CredentialGeneration::new(profile_id, generation, kind)
            .expect("valid credential generation"),
    }
}

fn write(reference: ProviderCredentialReference, secret: &str) -> ProtectedCredentialWrite {
    ProtectedCredentialWrite {
        reference,
        secret: SecretValue::from_utf8(secret.to_owned()),
    }
}

async fn exposed(vault: &impl CredentialVault, reference: ProviderCredentialReference) -> String {
    vault
        .read_generation(reference)
        .await
        .expect("credential generation is readable")
        .with_secret(|secret| secret.with_exposed(str::to_owned))
}

#[tokio::test]
async fn protection_probe_is_create_read_delete_and_unconfirmed_never_falls_back() {
    assert_eq!(KEYRING_SERVICE, "io.ysda.provider");

    let confirmed = InMemoryCredentialVault::new();
    assert_eq!(
        confirmed
            .protection_status()
            .await
            .expect("probe completes"),
        CredentialProtectionStatus::ConfirmedNative
    );
    assert!(
        confirmed.stored_accounts().is_empty(),
        "the protection probe must clean its temporary credential"
    );

    let unconfirmed =
        InMemoryCredentialVault::with_protection(CredentialProtectionStatus::Unconfirmed);
    let reference = reference(ProfileId::new(), 1, CredentialKind::ApiKey);
    let error = unconfirmed
        .write_generation(write(reference, "must-not-fall-back"))
        .await
        .expect_err("unconfirmed protection rejects writes");

    assert_eq!(error.code(), "provider.credential.protection_unavailable");
    assert!(unconfirmed.stored_accounts().is_empty());
}

#[tokio::test]
async fn every_protection_probe_phase_must_succeed_before_native_is_confirmed() {
    for operation in [
        InMemoryVaultOperation::Write,
        InMemoryVaultOperation::Read,
        InMemoryVaultOperation::Delete,
    ] {
        let vault = InMemoryCredentialVault::new();
        vault.fail_next(operation);

        assert_ne!(
            vault.protection_status().await.expect("probe completes"),
            CredentialProtectionStatus::ConfirmedNative,
            "{operation:?} failure must fail the protection probe closed"
        );
    }
}

#[tokio::test]
async fn restart_reads_versioned_api_key_and_oauth_generations_in_isolation() {
    let vault = InMemoryCredentialVault::new();
    assert_eq!(
        vault.protection_status().await.expect("probe completes"),
        CredentialProtectionStatus::ConfirmedNative
    );

    let api_profile = ProfileId::new();
    let oauth_profile = ProfileId::new();
    let api_v1 = reference(api_profile, 1, CredentialKind::ApiKey);
    let api_v2 = reference(api_profile, 2, CredentialKind::ApiKey);
    let oauth_v1 = reference(oauth_profile, 1, CredentialKind::OAuthConnection);

    vault
        .write_generation(write(api_v1.clone(), "api-one"))
        .await
        .expect("write first API-key generation");
    vault
        .write_generation(write(api_v2.clone(), "api-two"))
        .await
        .expect("write second API-key generation");
    vault
        .write_generation(write(oauth_v1.clone(), "oauth-bundle"))
        .await
        .expect("write OAuth generation");

    let restarted = vault.restart();
    assert_eq!(exposed(&restarted, api_v1.clone()).await, "api-one");
    assert_eq!(exposed(&restarted, api_v2.clone()).await, "api-two");
    assert_eq!(exposed(&restarted, oauth_v1.clone()).await, "oauth-bundle");

    let mut expected_accounts = vec![
        format!("{api_profile}:1"),
        format!("{api_profile}:2"),
        format!("{oauth_profile}:1"),
    ];
    expected_accounts.sort();
    assert_eq!(restarted.stored_accounts(), expected_accounts);

    restarted
        .delete_generation(api_v1.clone())
        .await
        .expect("delete one immutable generation");
    assert_eq!(exposed(&restarted, api_v2).await, "api-two");
    assert_eq!(exposed(&restarted, oauth_v1).await, "oauth-bundle");
    assert_eq!(
        restarted
            .credential_status(api_v1)
            .await
            .expect("status is available"),
        CredentialViewStatus::Missing
    );
}

#[tokio::test]
async fn immutable_generation_and_faults_preserve_the_last_complete_state() {
    let vault = InMemoryCredentialVault::new();
    vault.protection_status().await.expect("probe completes");

    let profile_id = ProfileId::new();
    let old = reference(profile_id, 1, CredentialKind::ApiKey);
    let replacement = reference(profile_id, 2, CredentialKind::ApiKey);
    vault
        .write_generation(write(old.clone(), "old-secret"))
        .await
        .expect("write old generation");

    let overwrite = vault
        .write_generation(write(old.clone(), "overwritten"))
        .await
        .expect_err("immutable generation cannot be overwritten");
    assert_eq!(overwrite.code(), "provider.storage.conflict");
    assert_eq!(exposed(&vault, old.clone()).await, "old-secret");

    vault.fail_next(InMemoryVaultOperation::Write);
    let replacement_error = vault
        .write_generation(write(replacement.clone(), "new-secret"))
        .await
        .expect_err("injected replacement failure is visible");
    assert_eq!(
        replacement_error.code(),
        "provider.credential.protection_unavailable"
    );
    assert!(!format!("{replacement_error:?}").contains("new-secret"));
    assert_eq!(exposed(&vault, old.clone()).await, "old-secret");
    assert_eq!(
        vault
            .credential_status(replacement.clone())
            .await
            .expect("status is available"),
        CredentialViewStatus::Missing
    );

    vault.fail_next(InMemoryVaultOperation::Delete);
    vault
        .delete_generation(old.clone())
        .await
        .expect_err("injected delete failure is visible");
    assert_eq!(exposed(&vault, old.clone()).await, "old-secret");
    vault
        .delete_generation(old.clone())
        .await
        .expect("delete can be safely retried");
    vault
        .delete_generation(old.clone())
        .await
        .expect("missing delete remains idempotent");
}

#[tokio::test]
async fn malformed_ownership_fails_closed_and_debug_is_redacted() {
    let vault = InMemoryCredentialVault::new();
    vault.protection_status().await.expect("probe completes");

    let owner = ProfileId::new();
    let wrong_owner = ProfileId::new();
    let malformed = ProviderCredentialReference {
        profile_id: wrong_owner,
        generation: CredentialGeneration::new(owner, 1, CredentialKind::ApiKey)
            .expect("valid generation"),
    };
    let error = vault
        .write_generation(write(malformed, "ownership-canary"))
        .await
        .expect_err("cross-profile reference fails closed");
    assert_eq!(error.code(), "provider.storage.conflict");

    let api_reference = reference(owner, 1, CredentialKind::ApiKey);
    vault
        .write_generation(write(api_reference, "debug-canary"))
        .await
        .expect("write generation for redaction assertion");
    let wrong_tag = reference(owner, 1, CredentialKind::OAuthConnection);
    let error = vault
        .read_generation(wrong_tag)
        .await
        .err()
        .expect("envelope tag mismatch fails closed");
    assert_eq!(error.code(), "provider.storage.conflict");

    let rendered = format!("{vault:?}");
    assert!(rendered.contains("InMemoryCredentialVault"));
    assert!(!rendered.contains("ownership-canary"));
    assert!(!rendered.contains("debug-canary"));
}

#[test]
fn production_type_is_os_keyring_only_and_its_debug_surface_is_non_sensitive() {
    let vault = KeyringCredentialVault::new();
    let _: &dyn CredentialVault = &vault;
    let rendered = format!("{vault:?}");

    assert!(rendered.contains("KeyringCredentialVault"));
    assert!(rendered.contains(KEYRING_SERVICE));
    assert!(!rendered.contains("credential_value"));
}
