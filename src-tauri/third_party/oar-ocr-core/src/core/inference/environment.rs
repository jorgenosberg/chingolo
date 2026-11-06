//! Initialization of the process-wide ONNX Runtime environment.

use ort::environment::{Environment, EnvironmentBuilder};
use ort::logging::LogLevel;
use std::sync::OnceLock;

static ENV: OnceLock<Environment> = OnceLock::new();

/// The process-wide ONNX Runtime environment, created on first use.
pub fn environment() -> ort::Result<&'static Environment> {
    if let Some(e) = ENV.get() {
        return Ok(e);
    }
    let env = ort::init().build()?;
    env.set_log_level(LogLevel::Error);
    Ok(ENV.get_or_init(|| env))
}

/// Returns `true` when this call created the environment.
pub fn initialize_ort_environment() -> ort::Result<bool> {
    let first = ENV.get().is_none();
    environment()?;
    Ok(first)
}

pub(super) fn commit_environment(builder: EnvironmentBuilder) -> ort::Result<bool> {
    if ENV.get().is_some() {
        return Ok(false);
    }
    let env = builder.build()?;
    env.set_log_level(LogLevel::Error);
    Ok(ENV.set(env).is_ok())
}

pub(super) fn lower_owned_environment_log_level(level: LogLevel) -> ort::Result<()> {
    if let Some(e) = ENV.get() {
        e.set_log_level(level);
    }
    Ok(())
}
