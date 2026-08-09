use comfy_table::Table;

use crate::domain::{AgentRun, QueryResult, SchemaSnapshot};

pub fn render_schema(schema: &SchemaSnapshot) -> String {
    let mut table = Table::new();
    table.set_header(["table", "column", "type", "not null", "pk position"]);
    for table_schema in &schema.tables {
        for column in &table_schema.columns {
            table.add_row([
                table_schema.name.clone(),
                column.name.clone(),
                column.data_type.clone(),
                column.not_null.to_string(),
                column.primary_key_position.to_string(),
            ]);
        }
    }
    table.to_string()
}

pub fn render_result(result: &QueryResult) -> String {
    if result.rows.is_empty() && result.row_count > 0 {
        return format!(
            "{} row(s); row values omitted from persisted trace",
            result.row_count
        );
    }
    let mut table = Table::new();
    table.set_header(result.columns.clone());
    for row in &result.rows {
        table.add_row(row.iter().map(ToString::to_string));
    }
    let suffix = if result.truncated {
        format!("\n{} row(s) shown (truncated)", result.row_count)
    } else {
        format!("\n{} row(s)", result.row_count)
    };
    format!("{table}{suffix}")
}

pub fn render_run(run: &AgentRun) -> String {
    let mut sections = vec![format!("Run ID: {}", run.run_id)];
    if let Some(query) = &run.generated_query {
        sections.push(format!(
            "SQL:\n{}\n\nExplanation:\n{}",
            query.sql, query.explanation
        ));
    }

    if let Some(decision) = &run.policy_decision {
        sections.push(format!(
            "Policy: {}{}",
            if decision.allowed {
                "allowed"
            } else {
                "denied"
            },
            if decision.reasons.is_empty() {
                String::new()
            } else {
                format!(" ({})", decision.reasons.join("; "))
            }
        ));
    }

    if let Some(result) = &run.result {
        sections.push(render_result(result));
    }

    if !run.events.is_empty() {
        let events = run
            .events
            .iter()
            .map(|event| {
                format!(
                    "- {} at {} ms: {}",
                    event.stage, event.elapsed_ms, event.message
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        sections.push(format!("Events:\n{events}"));
    }

    if let Some(error) = &run.error {
        sections.push(format!("Error:\n{}\n{}", error.category, error.message));
    }

    sections.join("\n\n")
}
