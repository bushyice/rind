use std::fmt::{Display, Formatter};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CoreError {
  ParseToml(String),
  MissingField { path: String },
  TypeMismatch { path: String, expected: String },
  MissingSchema { name: String },
  DependencyCycle { cycle: Vec<String> },
  RuntimeStopped,
  MetadataNotFound(String),
  MissingInstances(String),
  InvalidState(String),
  EventBusError(String),
  PersistenceError(String),
  Custom(String),
}

impl Display for CoreError {
  fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
    match self {
      CoreError::ParseToml(x) => write!(f, "parse error: {x}"),
      CoreError::MissingField { path } => write!(f, "missing field `{path}`"),
      CoreError::TypeMismatch { path, expected } => {
        write!(f, "type mismatch for `{path}`, expected {expected}")
      }
      CoreError::MissingSchema { name } => write!(f, "missing metadata schema `{name}`"),
      CoreError::DependencyCycle { cycle } => write!(f, "dependency cycle: {}", cycle.join(" -> ")),
      CoreError::RuntimeStopped => write!(f, "runtime stopped"),
      CoreError::MetadataNotFound(x) => write!(f, "metadata {x} not found"),
      CoreError::MissingInstances(x) => write!(f, "metadata or instance {x} not found"),
      CoreError::InvalidState(x) => write!(f, "invalid state: {x}"),
      CoreError::EventBusError(x) => write!(f, "event bus error: {x}"),
      CoreError::PersistenceError(x) => write!(f, "persistence error: {x}"),
      CoreError::Custom(x) => write!(f, "{x}"),
    }
  }
}

impl CoreError {
  pub fn custom(thing: impl std::error::Error) -> Self {
    CoreError::Custom(thing.to_string())
  }
}

impl From<anyhow::Error> for CoreError {
  fn from(value: anyhow::Error) -> Self {
    Self::Custom(value.to_string())
  }
}

impl std::error::Error for CoreError {}
