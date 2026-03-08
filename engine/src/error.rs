use thiserror::Error;

#[derive(Debug, Error)]
pub enum EngineError {
    #[error("Parse error: {0}")]
    Parse(String),

    #[error("Invalid mapping: {0}")]
    Validation(String),

    #[error("Render error: {0}")]
    Render(String),

    #[error("Target '{0}' not found")]
    TargetNotFound(String),

    #[error("Cycle detected in view dependencies")]
    CycleDetected,
}
