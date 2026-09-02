use std::collections::BTreeMap;

use ys_agent_core::{
    ActiveProviderSlot, CompatibilityEvidence, CredentialGeneration, CredentialKind,
    CredentialMutationIntent, CredentialMutationOperation, CredentialMutationPhase,
    CredentialMutationRecord, CredentialPointerCommit, OperationId, ParameterApplicability,
    PersistedCompatibilityEvidence, PersistedCredentialMutationRecord, PersistedProfileRevision,
    ProfileHistory, ProfileId, ProfileName, ProfileRevision, ProfileState, ProviderErrorCode,
    ProviderFingerprint, ProviderId, ProviderModelId, ProviderParameterKey, ProviderParameters,
    RunId, RunProviderBinding, ValidationVersions,
};

fn profile_name(value: &str) -> ProfileName {
    ProfileName::new(value).expect("valid profile name")
}

fn validation_versions() -> ValidationVersions {
    ValidationVersions::new("catalog-v1", "probe-v1", "liter-v1", "codec-v1")
}

fn draft(
    profile_id: ProfileId,
    revision: u64,
    provider: ProviderId,
    parameters: ProviderParameters,
) -> ProfileRevision {
    let credential = CredentialGeneration::new(profile_id, 1, CredentialKind::ApiKey)
        .expect("valid credential generation");
    ProfileRevision::draft(
        profile_id,
        revision,
        provider,
        ProviderModelId::new(provider, format!("{}model-a", provider.model_prefix()))
            .expect("model uses provider prefix"),
        parameters,
        Some(credential),
    )
    .expect("valid draft")
}

fn ready(mut revision: ProfileRevision) -> ProfileRevision {
    let evidence =
        CompatibilityEvidence::passing(revision.validation_inputs(validation_versions()));
    revision
        .accept_validation(evidence, validation_versions())
        .expect("matching evidence");
    revision
}

#[test]
fn persisted_ready_revision_hydrates_only_matching_passing_evidence() {
    let profile_id = ProfileId::new();
    let original = draft(
        profile_id,
        1,
        ProviderId::DeepSeek,
        ProviderParameters::default(),
    );
    let evidence =
        CompatibilityEvidence::passing(original.validation_inputs(validation_versions()));
    let persisted_evidence = PersistedCompatibilityEvidence::from_evidence(&evidence);

    let restored = ProfileRevision::hydrate(PersistedProfileRevision {
        profile_id,
        revision: 1,
        provider: ProviderId::DeepSeek,
        model: ProviderModelId::new(ProviderId::DeepSeek, "deepseek/model-a").expect("valid model"),
        parameters: ProviderParameters::default(),
        credential_generation: original.credential_generation(),
        state: ProfileState::Ready,
        validation: Some(persisted_evidence),
    })
    .expect("matching persisted evidence restores a ready revision");

    assert_eq!(restored.state(), ProfileState::Ready);
    assert_eq!(restored.validation(), Some(&evidence));
}

#[test]
fn credential_journal_contract_preserves_only_valid_non_secret_recovery_state() {
    let profile_id = ProfileId::new();
    let old_generation =
        CredentialGeneration::new(profile_id, 1, CredentialKind::ApiKey).expect("old generation");
    let new_generation =
        CredentialGeneration::new(profile_id, 2, CredentialKind::ApiKey).expect("new generation");
    let intent = CredentialMutationIntent::replace(
        OperationId::new(),
        profile_id,
        3,
        old_generation,
        new_generation,
    )
    .expect("a replacement records both profile-scoped generations");
    let wrong_kind = CredentialGeneration::new(profile_id, 3, CredentialKind::OAuthConnection)
        .expect("different credential kind");
    let error = CredentialMutationIntent::replace(
        OperationId::new(),
        profile_id,
        3,
        old_generation,
        wrong_kind,
    )
    .expect_err("one mutation cannot cross authentication kinds");
    assert_eq!(error.code(), "credential_kind_mismatch");
    let error = CredentialMutationIntent::replace(
        OperationId::new(),
        profile_id,
        3,
        new_generation,
        old_generation,
    )
    .expect_err("generation numbers never move backwards");
    assert_eq!(error.code(), "invalid_credential_mutation_shape");

    let record = CredentialMutationRecord::intent_recorded(intent);
    assert_eq!(record.operation(), CredentialMutationOperation::Replace);
    assert_eq!(record.phase(), CredentialMutationPhase::IntentRecorded);
    assert_eq!(record.old_generation(), Some(old_generation));
    assert_eq!(record.new_generation(), Some(new_generation));
    assert_eq!(record.error_code(), None);

    let record = record
        .transition(CredentialMutationPhase::VaultWritten, None)
        .expect("a staged protected generation can be recorded");
    let error = record
        .transition(CredentialMutationPhase::Completed, None)
        .expect_err("the pointer and cleanup phases cannot be skipped");
    assert_eq!(error.code(), "invalid_credential_mutation_transition");

    let restored = CredentialMutationRecord::hydrate(PersistedCredentialMutationRecord {
        operation_id: OperationId::new(),
        profile_id,
        expected_revision: 3,
        operation: CredentialMutationOperation::Replace,
        old_generation: Some(old_generation),
        new_generation: Some(new_generation),
        rollback_generation: None,
        phase: CredentialMutationPhase::Blocked,
        error_code: Some(ProviderErrorCode::StorageConflict),
    })
    .expect("a persisted fail-closed record restores without inventing state");
    assert!(restored.blocks_profile_use());
    assert_eq!(
        restored.error_code(),
        Some(ProviderErrorCode::StorageConflict)
    );

    let replacement = ProfileRevision::draft(
        profile_id,
        4,
        ProviderId::DeepSeek,
        ProviderModelId::new(ProviderId::DeepSeek, "deepseek/model-a").expect("valid model"),
        ProviderParameters::default(),
        Some(new_generation),
    )
    .expect("replacement revision");
    let pointer = CredentialPointerCommit::new(restored.operation_id(), profile_id, 3, replacement)
        .expect("pointer commit appends the next Draft revision");
    assert_eq!(pointer.new_generation(), Some(new_generation));
}

#[test]
fn provider_catalog_is_exact_and_model_prefixes_are_fail_closed() {
    assert_eq!(ProviderId::ALL.len(), 9);
    assert_eq!(ProviderId::ChatGptSubscription.model_prefix(), "chatgpt/");
    assert_eq!(ProviderId::OpenCodeGo.model_prefix(), "opencode-go/");
    assert_eq!(ProviderId::OpenCodeZen.model_prefix(), "opencode/");
    assert_eq!(ProviderId::DeepSeek.model_prefix(), "deepseek/");
    assert_eq!(ProviderId::Xai.model_prefix(), "xai/");
    assert_eq!(ProviderId::Zai.model_prefix(), "zai/");
    assert_eq!(ProviderId::OpenRouter.model_prefix(), "openrouter/");
    assert_eq!(ProviderId::MiniMax.model_prefix(), "minimax/");
    assert_eq!(ProviderId::Anthropic.model_prefix(), "anthropic/");

    assert!(ProviderModelId::new(ProviderId::DeepSeek, "deepseek/reasoner").is_ok());
    let error = ProviderModelId::new(ProviderId::DeepSeek, "openai/gpt-5")
        .expect_err("directory-external prefix must be rejected");
    assert_eq!(error.code(), "provider_model_prefix_mismatch");

    let model_owned_by_deepseek = ProviderModelId::new(ProviderId::DeepSeek, "deepseek/reasoner")
        .expect("valid DeepSeek model");
    let error = ProfileRevision::draft(
        ProfileId::new(),
        1,
        ProviderId::Anthropic,
        model_owned_by_deepseek,
        ProviderParameters::default(),
        None,
    )
    .expect_err("a model validated for one provider cannot be attached to another");
    assert_eq!(error.code(), "provider_model_owner_mismatch");
}

#[test]
fn revisions_are_append_only_and_credential_generations_are_profile_scoped() {
    let profile_id = ProfileId::new();
    let revision = draft(
        profile_id,
        1,
        ProviderId::DeepSeek,
        ProviderParameters::default(),
    );
    let mut history = ProfileHistory::new(profile_id, profile_name("primary"));
    history
        .append(revision.clone())
        .expect("first revision appends");
    let error = history
        .append(revision)
        .expect_err("a revision cannot be overwritten");
    assert_eq!(error.code(), "provider_revision_overwrite_rejected");

    let other_profile = ProfileId::new();
    let other_generation = CredentialGeneration::new(other_profile, 1, CredentialKind::ApiKey)
        .expect("valid generation");
    let error = ProfileRevision::draft(
        profile_id,
        2,
        ProviderId::DeepSeek,
        ProviderModelId::new(ProviderId::DeepSeek, "deepseek/model-b").unwrap(),
        ProviderParameters::default(),
        Some(other_generation),
    )
    .expect_err("credential generations cannot be shared by profiles");
    assert_eq!(error.code(), "credential_profile_mismatch");

    let api_key = CredentialGeneration::new(profile_id, 2, CredentialKind::ApiKey).unwrap();
    let error = ProfileRevision::draft(
        profile_id,
        2,
        ProviderId::ChatGptSubscription,
        ProviderModelId::new(ProviderId::ChatGptSubscription, "chatgpt/model-b").unwrap(),
        ProviderParameters::default(),
        Some(api_key),
    )
    .expect_err("ChatGPT subscription rejects API-key credentials");
    assert_eq!(error.code(), "credential_kind_mismatch");
}

#[test]
fn validation_digest_is_invalidated_by_critical_input_changes() {
    let profile_id = ProfileId::new();
    let mut parameters = ProviderParameters::default();
    parameters.set_temperature(Some(0.2)).unwrap();
    let revision = draft(profile_id, 1, ProviderId::Anthropic, parameters.clone());
    let versions = validation_versions();
    let evidence = CompatibilityEvidence::passing(revision.validation_inputs(versions.clone()));
    assert!(evidence.matches(&revision.validation_inputs(versions.clone())));
    assert!(
        !evidence.matches(&revision.validation_inputs(ValidationVersions::new(
            "catalog-v1",
            "probe-v1",
            "liter-v1",
            "codec-v2",
        )))
    );

    parameters.set_temperature(Some(0.8)).unwrap();
    let changed_parameters = parameters.clone();
    let mut changed = draft(profile_id, 2, ProviderId::Anthropic, parameters);
    assert!(!evidence.matches(&changed.validation_inputs(validation_versions())));
    let error = changed
        .accept_validation(evidence, validation_versions())
        .expect_err("a stale validation digest cannot make a changed revision ready");
    assert_eq!(error.code(), "validation_digest_stale");

    let mut rules = BTreeMap::new();
    rules.insert(
        ProviderParameterKey::Temperature,
        ParameterApplicability::Unsupported,
    );
    let error = changed_parameters
        .validate_applicability(&rules)
        .expect_err("unsupported parameters are rejected rather than silently dropped");
    assert_eq!(error.code(), "provider_parameter_unsupported");

    let error = ProviderParameters::default()
        .validate_applicability(&BTreeMap::new())
        .expect_err("parameters without an explicit catalog rule fail closed");
    assert_eq!(error.code(), "provider_parameter_unclassified");

    let mut conditional_rules = BTreeMap::new();
    conditional_rules.insert(
        ProviderParameterKey::Timeout,
        ParameterApplicability::Conditional,
    );
    conditional_rules.insert(
        ProviderParameterKey::Retry,
        ParameterApplicability::Supported,
    );
    let error = ProviderParameters::default()
        .validate_applicability(&conditional_rules)
        .expect_err("conditional parameters require model-level evidence before use");
    assert_eq!(error.code(), "provider_parameter_conditional");
}

#[test]
fn active_slot_has_one_ready_snapshot_and_fingerprint_is_canonical_and_non_sensitive() {
    let first_profile = ProfileId::new();
    let first = ready(draft(
        first_profile,
        1,
        ProviderId::OpenRouter,
        ProviderParameters::default(),
    ));
    assert_eq!(first.state(), ProfileState::Ready);

    let mut active = ActiveProviderSlot::empty();
    active
        .activate(&first)
        .expect("ready revision can activate");
    let first_validation_id = active.current().expect("active snapshot").validation_id();
    let first_binding = RunProviderBinding::from_active(
        RunId::new(),
        active.current().expect("active snapshot").clone(),
    )
    .expect("run receives an immutable binding");

    let second_profile = ProfileId::new();
    let second = ready(draft(
        second_profile,
        1,
        ProviderId::MiniMax,
        ProviderParameters::default(),
    ));
    active
        .activate(&second)
        .expect("new active atomically replaces old one");
    assert_eq!(active.current().unwrap().profile_id(), second_profile);
    assert_eq!(active.current().unwrap().profile_revision(), 1);
    assert_eq!(first_binding.profile_id(), first_profile);
    assert_eq!(first_binding.profile_revision(), 1);
    assert_eq!(first_binding.validation_id(), first_validation_id);

    let unready_revision = draft(
        ProfileId::new(),
        1,
        ProviderId::Zai,
        ProviderParameters::default(),
    );
    let error = active
        .activate(&unready_revision)
        .expect_err("a draft revision cannot replace the active snapshot");
    assert_eq!(error.code(), "profile_revision_not_ready");

    let mut left_specific = BTreeMap::new();
    left_specific.insert("beta".to_owned(), 2_i64.into());
    left_specific.insert("alpha".to_owned(), true.into());
    let mut right_specific = BTreeMap::new();
    right_specific.insert("alpha".to_owned(), true.into());
    right_specific.insert("beta".to_owned(), 2_i64.into());
    let left = ready(draft(
        ProfileId::new(),
        1,
        ProviderId::Xai,
        ProviderParameters::with_provider_specific(left_specific),
    ));
    let right = ready(draft(
        left.profile_id(),
        1,
        ProviderId::Xai,
        ProviderParameters::with_provider_specific(right_specific),
    ));
    let left_fingerprint = ProviderFingerprint::from_revision(&left).unwrap();
    let right_fingerprint = ProviderFingerprint::from_revision(&right).unwrap();
    assert_eq!(left_fingerprint.digest(), right_fingerprint.digest());

    let mut out_of_whitelist = BTreeMap::new();
    out_of_whitelist.insert("arbitrary_business_marker".to_owned(), 99_i64.into());
    let outside = ready(draft(
        left.profile_id(),
        1,
        ProviderId::Xai,
        ProviderParameters::with_provider_specific(out_of_whitelist),
    ));
    let outside_fingerprint = ProviderFingerprint::from_revision(&outside).unwrap();
    assert_eq!(left_fingerprint.digest(), outside_fingerprint.digest());
    assert!(!left_fingerprint.canonical_json().contains("credential"));
    assert!(!left_fingerprint.canonical_json().contains("locator"));
    assert!(
        !left_fingerprint
            .canonical_json()
            .contains("provider_specific")
    );
}
