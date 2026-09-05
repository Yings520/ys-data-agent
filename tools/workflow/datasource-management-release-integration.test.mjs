import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

test("v0.2 release gate requires datasource engine evidence and every real three-driver path", async () => {
  const script = await readFile("scripts/v0.2-release-gate.sh", "utf8");
  const composeUp = script.indexOf("up --detach --wait");
  const versionEvidence = script.indexOf("datasource_release_evidence_test");
  const postgresVersion = script.indexOf("SHOW server_version");
  const managedSqlite = script.indexOf("--test managed_sqlite_test");
  const managedDuckdb = script.indexOf("--test managed_duckdb_test");
  const managedPostgres = script.indexOf("--test managed_postgres_test");
  const runtimePostgres = script.indexOf("service_runs_real_postgres_save_validate_and_select");
  const tuiThreeDriver = script.indexOf(
    "real_three_driver_forms_save_validate_select_and_set_default",
  );
  const queryArtifact = script.indexOf("--test query_eval_test");

  for (const [name, position] of Object.entries({
    versionEvidence,
    postgresVersion,
    managedSqlite,
    managedDuckdb,
    managedPostgres,
    runtimePostgres,
    tuiThreeDriver,
    queryArtifact,
  })) {
    assert.ok(position > composeUp, `${name} must run after PostgreSQL is healthy`);
  }
  assert.match(script, /YSDA_TEST_POSTGRES_PASSWORD=ysda-reader-test/);
  assert.ok(
    script.indexOf("--test datasource_tui_test") < tuiThreeDriver,
    "the real TUI test must name its integration-test target",
  );
  assert.ok(
    script.slice(tuiThreeDriver).includes("-- --ignored --nocapture"),
    "the ignored-by-default Docker test must be explicitly executed with evidence output",
  );
});

test("datasource release steps are fail-fast and are not converted to optional probes", async () => {
  const script = await readFile("scripts/v0.2-release-gate.sh", "utf8");
  const datasourceSection = script.slice(script.indexOf("datasource_release_evidence_test"));

  assert.doesNotMatch(datasourceSection, /\|\|\s*true/);
  assert.doesNotMatch(datasourceSection, /command\s+-v/);
  assert.doesNotMatch(datasourceSection, /if\s+.*(?:postgres|duckdb)/i);
});
