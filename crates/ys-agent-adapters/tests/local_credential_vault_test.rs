use std::fs;

use tempfile::TempDir;
use ys_agent_adapters::credential::local::LocalEncryptedCredentialVault;
use ys_agent_core::{
    CredentialGeneration, CredentialKind, CredentialProtectionStatus, CredentialVault,
    CredentialViewStatus, ProfileId, ProtectedCredentialWrite, ProviderCredentialReference,
    SecretValue,
};

fn reference(profile_id: ProfileId, generation: u64) -> ProviderCredentialReference {
    ProviderCredentialReference {
        profile_id,
        generation: CredentialGeneration::new(profile_id, generation, CredentialKind::ApiKey)
            .expect("valid API-key generation"),
    }
}

fn write(reference: ProviderCredentialReference, secret: &str) -> ProtectedCredentialWrite {
    ProtectedCredentialWrite {
        reference,
        secret: SecretValue::from_utf8(secret.to_owned()),
    }
}

#[tokio::test]
async fn local_vault_encrypts_private_files_and_reopens_without_an_os_credential_store() {
    let directory = TempDir::new().expect("temporary local workspace");
    let profile_id = ProfileId::new();
    let credential = reference(profile_id, 1);
    let canary = "api-key-local-encryption-canary";
    let vault = LocalEncryptedCredentialVault::new(directory.path());

    assert_eq!(
        vault
            .protection_status()
            .await
            .expect("local protection status"),
        CredentialProtectionStatus::ConfirmedLocal
    );
    vault
        .write_generation(write(credential.clone(), canary))
        .await
        .expect("write encrypted local credential");

    let encrypted_entry = directory
        .path()
        .join("provider-credentials")
        .join(format!("{profile_id}-1.credential"));
    let encrypted_bytes = fs::read(&encrypted_entry).expect("encrypted local credential file");
    assert!(
        !String::from_utf8_lossy(&encrypted_bytes).contains(canary),
        "the credential file must not contain the API key in plaintext"
    );
    assert_eq!(
        vault
            .credential_status(credential.clone())
            .await
            .expect("encrypted credential status"),
        CredentialViewStatus::Saved
    );

    let restarted = LocalEncryptedCredentialVault::new(directory.path());
    let recovered = restarted
        .read_generation(credential)
        .await
        .expect("read encrypted credential after restart")
        .with_secret(|secret| secret.with_exposed(str::to_owned));
    assert_eq!(recovered, canary);

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let key_mode = fs::metadata(directory.path().join("provider-credentials.key"))
            .expect("local encryption key metadata")
            .permissions()
            .mode()
            & 0o777;
        let credential_mode = fs::metadata(encrypted_entry)
            .expect("encrypted credential metadata")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(key_mode, 0o600);
        assert_eq!(credential_mode, 0o600);
    }
}
