# YS Data Agent

[![License](https://img.shields.io/badge/license-MIT-blue)](LICENSE)
[![Website](https://img.shields.io/badge/website-ys--data-orange)](https://github.com/Yings520/ys-data-agent)
[![Docs](https://img.shields.io/badge/docs-README-green)](https://github.com/Yings520/ys-data-agent#readme)
[![Quick Start](https://img.shields.io/badge/quick%20start-%F0%9F%9A%80-brightgreen)](https://github.com/Yings520/ys-data-agent#build-and-test)
[![Release Notes](https://img.shields.io/badge/release-notes-lightgrey)](https://github.com/Yings520/ys-data-agent/releases)

**YS Data Agent** is a safe, observable Data Query Agent written in Rust under the **YS Data** personal technology brand. It inspects a SQLite schema, asks an OpenAI-compatible model for a structured SQL candidate, validates the SQL AST, executes it through a read-only connection, renders the result, and records a local trace.

This project is an independent implementation inspired by the product ideas in [Datus](https://github.com/Datus-ai/datus-agent). It does not copy or translate Datus source code.

## Why this project exists

The first release is a concrete Text-to-SQL vertical slice. The long-term goal is a Rust runtime for Data Query, Data Analysis, Data Engineering, and DataOps agents, including domain-scoped multi-Agent collaboration for data pipeline delivery.

## Architecture

```text
Question
  -> SQLite catalog
  -> LLM JSON response
  -> SQL AST policy
  -> read-only executor
  -> terminal result
  -> credential-free local trace
```

## Safety boundary

- exactly one SQL statement
- only `Statement::Query` accepted by the AST policy
- SQLite opened read-only with `query_only`
- maximum 100 returned rows
- query row values excluded from saved traces
- API key and authorization header never serialized

This is a learning project, not a production security boundary. Do not point it at sensitive databases.

## Build and test

```bash
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test
```

## Create the demo database

```bash
sqlite3 examples/demo.db < examples/demo.sql
cargo run --bin ysda -- schema --database examples/demo.db
```

## Configure an OpenAI-compatible endpoint

Set these environment variables in your shell without committing them:

```text
LLM_API_KEY
LLM_BASE_URL
LLM_MODEL
```

The base URL must be the prefix before `/chat/completions`.

## Ask a question

```bash
cargo run --bin ysda -- ask \
  --database examples/demo.db \
  "Which customers have the highest total order amount?"
```

The command prints the run ID, generated SQL, explanation, policy decision, and query result. Inspect the persisted run with:

```bash
cargo run --bin ysda -- trace <run-id>
```

Trace files live under `.ysda/traces/` and are ignored by Git.

Six repeatable manual questions are available in `examples/questions.txt`.

## Roadmap

- v0.2: one bounded SQL repair attempt and evaluation cases
- v0.3: Schema Linking and Reference SQL
- v0.4: DuckDB and PostgreSQL adapters
- v0.5: metrics and semantic context
- v0.6: YS Data Agent runtime primitives
- v1.0: Query, Analysis, Engineering, and DataOps agents with policy-controlled collaboration
