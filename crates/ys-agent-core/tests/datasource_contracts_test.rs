use ys_agent_core::{DatasourceName, DatasourceSecretRef, ProfileId, SecretEdit, WorkspaceId};

#[test]
fn credential_references_round_trip_both_schemes_and_reject_inline_values() {
    use ys_agent_core::CredentialReference;
    let reference = DatasourceSecretRef::new(WorkspaceId::new(), ProfileId::new(), 2).unwrap();
    let credential = CredentialReference::from_datasource(reference);
    assert_eq!(credential.datasource_reference(), Some(reference));
    assert_eq!(credential.environment_variable_name(), None);
    let encoded = serde_json::to_string(&credential).unwrap();
    assert_eq!(
        serde_json::from_str::<CredentialReference>(&encoded).unwrap(),
        credential
    );
    let env: CredentialReference = serde_json::from_str("\"env:DB_PASSWORD\"").unwrap();
    assert_eq!(env.environment_variable_name(), Some("DB_PASSWORD"));
    assert_eq!(env.datasource_reference(), None);
    for invalid in ["postgres://inline", "env:bad/name", "env:9INVALID", "env:"] {
        assert!(CredentialReference::new(invalid).is_err());
        assert!(
            serde_json::from_str::<CredentialReference>(&serde_json::to_string(invalid).unwrap())
                .is_err()
        );
    }
}

#[test]
fn datasource_names_have_workspace_uniqueness_keys() {
    let name = DatasourceName::new("  Analytics  ").unwrap();
    assert_eq!(name.as_str(), "Analytics");
    assert_eq!(name.uniqueness_key(), "analytics");
    assert_eq!(
        DatasourceName::new("ÄNALYTICS").unwrap().uniqueness_key(),
        "Änalytics"
    );
    assert!(DatasourceName::new(" \t ").is_err());
    assert!(DatasourceName::new("bad\nname").is_err());
    assert!(serde_json::from_str::<DatasourceName>("\" \"").is_err());
}

#[test]
fn datasource_secret_references_bind_workspace_profile_and_generation() {
    let workspace = WorkspaceId::new();
    let profile = ProfileId::new();
    let reference = DatasourceSecretRef::new(workspace, profile, 1).unwrap();
    assert_eq!(reference.workspace_id(), workspace);
    assert_eq!(reference.profile_id(), profile);
    assert_eq!(reference.generation(), 1);
    assert!(DatasourceSecretRef::new(workspace, profile, 0).is_err());
    assert_ne!(
        reference,
        DatasourceSecretRef::new(WorkspaceId::new(), profile, 1).unwrap()
    );
    assert_ne!(
        reference,
        DatasourceSecretRef::new(workspace, ProfileId::new(), 1).unwrap()
    );
    assert_ne!(
        reference,
        DatasourceSecretRef::new(workspace, profile, 2).unwrap()
    );
}

#[test]
fn keeping_a_secret_preserves_its_exact_reference() {
    let reference = DatasourceSecretRef::new(WorkspaceId::new(), ProfileId::new(), 3).unwrap();
    assert_eq!(
        SecretEdit::Keep.retained_reference(Some(reference)),
        Some(reference)
    );
    assert_eq!(SecretEdit::Remove.retained_reference(Some(reference)), None);
    assert_eq!(SecretEdit::Keep.retained_reference(None), None);
}

#[test]
fn local_fields_distinguish_incomplete_drafts_from_invalid_configuration() {
    use std::collections::BTreeMap;
    use ys_agent_core::{
        DatasourceField, FieldId, FieldInput, FieldValue, validate_datasource_fields,
    };
    let port = FieldId::new("port").unwrap();
    let password = FieldId::new("password").unwrap();
    let fields = vec![
        DatasourceField {
            id: port.clone(),
            label: "Port".into(),
            required: true,
            input: FieldInput::Integer { min: 1, max: 65535 },
            default: None,
        },
        DatasourceField {
            id: password.clone(),
            label: "Password".into(),
            required: true,
            input: FieldInput::Secret,
            default: None,
        },
    ];
    assert!(validate_datasource_fields(&fields, &BTreeMap::new(), false, false).is_empty());
    assert_eq!(
        validate_datasource_fields(&fields, &BTreeMap::new(), false, true).len(),
        2
    );
    let bad_port = BTreeMap::from([(port.clone(), FieldValue::Integer(0))]);
    assert_eq!(
        validate_datasource_fields(&fields, &bad_port, false, false).len(),
        1
    );
    let ordinary_secret = BTreeMap::from([(password, FieldValue::Text("masked".into()))]);
    assert_eq!(
        validate_datasource_fields(&fields, &ordinary_secret, true, false).len(),
        1
    );
    let valid = BTreeMap::from([(port, FieldValue::Integer(5432))]);
    assert!(validate_datasource_fields(&fields, &valid, true, true).is_empty());
    let unknown = BTreeMap::from([(FieldId::new("unknown").unwrap(), FieldValue::Boolean(true))]);
    assert_eq!(
        validate_datasource_fields(&fields, &unknown, false, false).len(),
        1
    );
}

#[test]
fn read_only_capability_is_explicit_and_legacy_metadata_is_unproven() {
    use ys_agent_core::{CapabilityDescriptor, ReadOnlyMechanism, SourceId};
    let capability = CapabilityDescriptor {
        source_id: SourceId::new("local"),
        dialect: "sqlite".into(),
        catalog_reader: true,
        sql_query_executor: true,
        freshness_reader: true,
        supports_explain: true,
        supports_read_only_tx: false,
        max_concurrency: 1,
        preflight_reader: true,
        read_only_mechanism: Some(ReadOnlyMechanism::FileReadOnly),
    };
    assert!(capability.supports_governed_query());
    let mut legacy = serde_json::to_value(&capability).unwrap();
    legacy.as_object_mut().unwrap().remove("preflight_reader");
    legacy
        .as_object_mut()
        .unwrap()
        .remove("read_only_mechanism");
    let legacy: CapabilityDescriptor = serde_json::from_value(legacy).unwrap();
    assert!(!legacy.supports_governed_query());
    let mut missing_preflight = capability;
    missing_preflight.preflight_reader = false;
    assert!(!missing_preflight.supports_governed_query());
}

#[test]
fn datasource_revision_rejects_unknown_versions_and_foreign_credentials() {
    use ys_agent_core::{DatabaseContext, DatasourceRevision, DatasourceRevisionInput};
    let workspace = WorkspaceId::new();
    let profile = ProfileId::new();
    let input = DatasourceRevisionInput {
        schema_version: 1,
        workspace_id: workspace,
        profile_id: profile,
        revision: 1,
        adapter_id: "sqlite".try_into().unwrap(),
        adapter_version: "1".try_into().unwrap(),
        config_version: 1,
        source_id: None,
        fields: Default::default(),
        context: DatabaseContext::Unconfigured,
        credential: None,
    };
    let revision = DatasourceRevision::new(input.clone()).unwrap();
    assert_eq!(revision.number(), 1);
    assert!(
        revision
            .ensure_config_contract(&input.adapter_id, &input.adapter_version, 1)
            .is_ok()
    );
    assert!(
        revision
            .ensure_config_contract(&input.adapter_id, &input.adapter_version, 2)
            .is_err()
    );
    let mut unknown = input.clone();
    unknown.schema_version = 2;
    assert!(DatasourceRevision::new(unknown).is_err());
    let mut zero = input.clone();
    zero.revision = 0;
    assert!(DatasourceRevision::new(zero).is_err());
    let mut foreign = input.clone();
    foreign.credential = Some(DatasourceSecretRef::new(WorkspaceId::new(), profile, 1).unwrap());
    assert!(DatasourceRevision::new(foreign).is_err());
    let mut next = input;
    next.revision = 2;
    assert_ne!(
        revision.identity(),
        DatasourceRevision::new(next).unwrap().identity()
    );
    assert_eq!(revision.number(), 1);
}

#[test]
fn ready_evidence_is_bound_to_exact_revision_capability_policy_and_probe() {
    use ys_agent_core::*;
    let workspace = WorkspaceId::new();
    let profile = ProfileId::new();
    let source = SourceId::new("analytics");
    let revision = DatasourceRevision::new(DatasourceRevisionInput {
        schema_version: 1,
        workspace_id: workspace,
        profile_id: profile,
        revision: 1,
        adapter_id: "postgres".try_into().unwrap(),
        adapter_version: "1".try_into().unwrap(),
        config_version: 1,
        source_id: Some(source.clone()),
        fields: Default::default(),
        context: DatabaseContext::Database {
            catalog: None,
            database: "analytics".into(),
            schema: "public".into(),
        },
        credential: Some(DatasourceSecretRef::new(workspace, profile, 1).unwrap()),
    })
    .unwrap();
    let capability = CapabilityDescriptor {
        source_id: source,
        dialect: "postgres".into(),
        catalog_reader: true,
        sql_query_executor: true,
        freshness_reader: true,
        supports_explain: true,
        supports_read_only_tx: true,
        max_concurrency: 1,
        preflight_reader: true,
        read_only_mechanism: Some(ReadOnlyMechanism::TransactionReadOnly),
    };
    let policy = DatasourceDigest::of(&"policy-v1").unwrap();
    let inputs = DatasourceValidationInputs::new(&revision, &capability, policy.clone()).unwrap();
    let probe = ProbeEvidence {
        authenticated: true,
        target_verified: true,
        read_only_verified: true,
        least_privilege_verified: true,
        capabilities_verified: true,
    };
    let evidence = ValidationEvidence::new(
        inputs.clone(),
        "18".try_into().unwrap(),
        probe,
        chrono::Utc::now(),
    )
    .unwrap();
    assert!(evidence.matches(&inputs));
    let mut detail = DatasourceDetail {
        schema_version: 1,
        profile: DatasourceProfile {
            schema_version: 1,
            workspace_id: workspace,
            profile_id: profile,
            source_id: Some(capability.source_id.clone()),
            name: DatasourceName::new("analytics").unwrap(),
            head_revision: std::num::NonZeroU64::new(1).unwrap(),
            deleted_at: None,
        },
        revision: revision.clone(),
        state: RevisionState::Ready,
        validation: Some(evidence.clone()),
    };
    assert!(detail.is_ready(&inputs));
    detail.state = RevisionState::Draft;
    assert!(!detail.is_ready(&inputs));
    detail.state = RevisionState::Ready;
    let mut altered = revision.input().clone();
    altered.fields.insert(
        FieldId::new("host").unwrap(),
        FieldValue::Text("changed".into()),
    );
    detail.revision = DatasourceRevision::new(altered).unwrap();
    assert!(!detail.is_ready(&inputs));
    let run_id = RunId::new();
    let scope = DatasourceScope {
        workspace_id: workspace,
        session_id: SessionId::new(),
    };
    let binding =
        RunDatasourceBinding::from_validated(run_id, scope, 1, &revision, &evidence, &inputs)
            .unwrap();
    assert_eq!(binding.run_id(), run_id);
    assert_eq!(binding.revision(), revision.identity());
    let serialized = serde_json::to_string(&binding).unwrap();
    assert!(!serialized.contains("canonical_path"));
    assert_eq!(
        binding.digest().unwrap(),
        serde_json::from_str::<RunDatasourceBinding>(&serialized)
            .unwrap()
            .digest()
            .unwrap()
    );
    let mut file_input = revision.input().clone();
    file_input.context = DatabaseContext::File {
        canonical_path: std::path::PathBuf::from("/contract-private-location.sqlite"),
    };
    let file_revision = DatasourceRevision::new(file_input).unwrap();
    let file_inputs =
        DatasourceValidationInputs::new(&file_revision, &capability, policy.clone()).unwrap();
    let file_evidence = ValidationEvidence::new(
        file_inputs.clone(),
        "test".try_into().unwrap(),
        probe,
        chrono::Utc::now(),
    )
    .unwrap();
    let file_binding = RunDatasourceBinding::from_validated(
        run_id,
        scope,
        1,
        &file_revision,
        &file_evidence,
        &file_inputs,
    )
    .unwrap();
    let file_json = serde_json::to_string(&file_binding).unwrap();
    assert!(!file_json.contains("contract-private-location"));
    assert!(!format!("{file_binding:?}").contains("contract-private-location"));
    assert!(!evidence.matches(&file_inputs));

    let mut rotated = revision.input().clone();
    rotated.credential = Some(DatasourceSecretRef::new(workspace, profile, 2).unwrap());
    let rotated = DatasourceRevision::new(rotated).unwrap();
    assert!(
        !evidence.matches(
            &DatasourceValidationInputs::new(&rotated, &capability, policy.clone()).unwrap()
        )
    );
    let mut upgraded = revision.input().clone();
    upgraded.adapter_version = "2".try_into().unwrap();
    let upgraded = DatasourceRevision::new(upgraded).unwrap();
    assert!(!evidence.matches(
        &DatasourceValidationInputs::new(&upgraded, &capability, policy.clone()).unwrap()
    ));
    assert!(
        RunDatasourceBinding::from_validated(
            run_id,
            DatasourceScope {
                workspace_id: WorkspaceId::new(),
                ..scope
            },
            1,
            &revision,
            &evidence,
            &inputs,
        )
        .is_err()
    );
    let mut next = revision.input().clone();
    next.revision = 2;
    let next = DatasourceRevision::new(next).unwrap();
    assert!(
        !evidence
            .matches(&DatasourceValidationInputs::new(&next, &capability, policy.clone()).unwrap())
    );
    assert!(
        !evidence.matches(
            &DatasourceValidationInputs::new(
                &revision,
                &capability,
                DatasourceDigest::of(&"policy-v2").unwrap()
            )
            .unwrap()
        )
    );
    let mut weaker = capability;
    weaker.preflight_reader = false;
    assert!(DatasourceValidationInputs::new(&revision, &weaker, policy).is_err());
    let mut failed = probe;
    failed.least_privilege_verified = false;
    assert!(
        ValidationEvidence::new(inputs, "18".try_into().unwrap(), failed, chrono::Utc::now())
            .is_err()
    );
}
