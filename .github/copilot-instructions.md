# Copilot Instructions

Read [CONTRIBUTING.md](../CONTRIBUTING.md) before creating or editing files — it defines where each file type belongs and the required formats.

## Agent Workflow Policy (engine-rs/src)

When working on files in `engine-rs/src`, the agent must always run the following commands before finishing any task:

- cargo fmt --check
- cargo clippy --tests -- -D warnings
- cargo test --lib

This ensures all code is formatted, lint-free, and passes unit tests before handoff.