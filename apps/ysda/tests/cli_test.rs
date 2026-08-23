use clap::Parser;
use ysda::cli::{AgentCommand, Cli};

#[test]
fn parse_schema_command() {
    let cli = Cli::try_parse_from(["ysda", "schema", "--database", "examples/demo.db"])
        .expect("schema command should parse");
    assert!(matches!(
        cli.command,
        AgentCommand::Schema { database, .. } if database == *"examples/demo.db"
    ));
}

#[test]
fn parses_ask_question_as_one_argument() {
    let cli = Cli::try_parse_from([
        "ysda",
        "ask",
        "--database",
        "examples/demo.db",
        "top customers",
    ])
    .expect("ask command should parse");

    assert!(matches!(
        cli.command,
        AgentCommand::Ask { question, .. } if question == "top customers"
    ));
}
