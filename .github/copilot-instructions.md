# Copilot Instructions

Read [CONTRIBUTING.md](../CONTRIBUTING.md) before creating or editing files — it defines where each file type belongs and the required formats.

## Pre-1.0: No Backwards Compatibility

The project is pre-1.0. Do not add backwards-compatibility shims, serde aliases, or deprecation wrappers. Rename freely — old YAML that breaks is the user's problem to update. This rule will be removed at 1.0.

## Test-Driven Development (engine-rs/src)

Always use TDD: no bug fix or behaviour change without a failing test first. Write the test, confirm it fails, then write the minimal code to make it pass.

## Agent Workflow Policy (engine-rs/src)

When working on files in `engine-rs/src`, the agent must always run the following commands before finishing any task:

- cargo fmt --check
- cargo clippy --tests -- -D warnings
- cargo test

This ensures all code is formatted, lint-free, and passes unit tests before handoff.

To re-run only specific examples instead of the full integration suite, use:

```
OSI_EXAMPLES=hello-world,route cargo test execute_all_examples
```

Comma-separated substrings are matched against example directory names.