use std::{env, fs, process::ExitCode, time::Duration};

use chrono::Utc;
use ys_agent_runtime::provider::{
    catalog::GovernedProviderCatalog,
    evidence::{GOVERNED_CODEC_DIGEST, GOVERNED_LITER_LLM_VERSION},
    evidence_collector::EvidenceCollectionBaseline,
    evidence_gate::{ProviderEvidenceGate, ProviderEvidenceManifest},
    validation::COMPATIBILITY_PROBE_SCHEMA_VERSION,
};

fn main() -> ExitCode {
    match run(env::args().skip(1)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(code) => {
            eprintln!("{code}");
            ExitCode::from(2)
        }
    }
}

fn run(arguments: impl Iterator<Item = String>) -> Result<(), &'static str> {
    let mut manifest_path = None;
    let mut require_nine_of_nine = false;
    let mut arguments = arguments.peekable();
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--manifest" => {
                let Some(path) = arguments.next() else {
                    return Err("provider.evidence.invalid_arguments");
                };
                manifest_path = Some(path);
            }
            "--require-nine-of-nine" => require_nine_of_nine = true,
            _ => return Err("provider.evidence.invalid_arguments"),
        }
    }

    let catalog = GovernedProviderCatalog::default();
    let baseline = EvidenceCollectionBaseline {
        catalog_digest: catalog.digest().to_owned(),
        probe_schema_version: COMPATIBILITY_PROBE_SCHEMA_VERSION.to_owned(),
        codec_digest: GOVERNED_CODEC_DIGEST.to_owned(),
        liter_llm_version: GOVERNED_LITER_LLM_VERSION.to_owned(),
    };
    let gate = ProviderEvidenceGate::new(catalog, baseline, Duration::from_secs(60 * 60))
        .map_err(|error| error.code())?;
    let manifest = match manifest_path {
        Some(path) => {
            let source =
                fs::read_to_string(path).map_err(|_| "provider.evidence.manifest_unavailable")?;
            ProviderEvidenceManifest::from_json(&source).map_err(|error| error.code())?
        }
        None => ProviderEvidenceManifest::empty(),
    };
    let verdict = gate.evaluate(&manifest, Utc::now());
    println!(
        "{}",
        serde_json::to_string(&verdict).map_err(|_| "provider.evidence.invalid_manifest")?
    );
    if require_nine_of_nine {
        verdict
            .require_nine_of_nine()
            .map_err(|error| error.code())?;
    }
    Ok(())
}
