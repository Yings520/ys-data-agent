use async_trait::async_trait;
use std::sync::Arc;
use ys_agent_adapters::data::{
    BuiltinConnectorCatalog, ConnectorRegistration, SqliteConnectorFactory, builtin_descriptor,
};
use ys_agent_core::*;

// Explicit test-only factories establish that metadata listing never opens a database.
struct NeverOpen;
#[async_trait]
impl ConnectorFactory for NeverOpen {
    fn validate_config(&self, _: &DatasourceRevision) -> Vec<FieldIssue> {
        vec![]
    }
    async fn open(&self, _: ConnectorOpenInput) -> DsResult<Arc<dyn ManagedConnector>> {
        panic!("metadata must never open a connection")
    }
}

#[test]
fn catalog_has_versioned_form_metadata_and_real_factory_routes_without_io() {
    let catalog = BuiltinConnectorCatalog::new(Arc::new(NeverOpen), Arc::new(NeverOpen)).unwrap();
    let descriptors = catalog.descriptors().unwrap();
    assert_eq!(descriptors.len(), 3);
    for descriptor in descriptors {
        assert_eq!(descriptor.support, ConnectorSupport::Supported);
        assert!(descriptor.release_evidence.is_some());
        assert!(descriptor.capability.supports_governed_query());
        assert!(
            catalog
                .factory(&descriptor.adapter_id, &descriptor.adapter_version)
                .is_ok()
        );
        assert!(
            catalog
                .factory(
                    &descriptor.adapter_id,
                    &"unknown".to_owned().try_into().unwrap()
                )
                .is_err()
        );
        assert!(
            descriptor
                .fields
                .iter()
                .all(|field| !matches!(field.input, FieldInput::Secret) || field.default.is_none())
        );
    }
    let postgres = builtin_descriptor("postgres").unwrap();
    assert!(
        postgres
            .fields
            .iter()
            .any(|field| field.id.as_str() == "password" && field.input == FieldInput::Secret)
    );
    assert!(
        !postgres
            .fields
            .iter()
            .any(|field| field.id.as_str().contains("ssh"))
    );
}

#[test]
fn duplicate_registration_and_unproven_supported_claim_fail_closed() {
    let descriptor = builtin_descriptor("sqlite").unwrap();
    let registration = || ConnectorRegistration {
        descriptor: descriptor.clone(),
        factory: Arc::new(SqliteConnectorFactory),
    };
    assert!(
        BuiltinConnectorCatalog::from_registrations(vec![registration(), registration()]).is_err()
    );
    let mut unproven = registration();
    unproven.descriptor.support = ConnectorSupport::Supported;
    unproven.descriptor.release_evidence = None;
    assert!(BuiltinConnectorCatalog::from_registrations(vec![unproven]).is_err());
}
