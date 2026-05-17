use std::{
  fmt::{Display, Formatter},
  net::AddrParseError,
};

use nix::errno::Errno;

use crate::user::PamError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CoreError {
  ParseError(String),
  MissingField { path: String },
  TypeMismatch { path: String, expected: String },
  MissingSchema { name: String },
  DependencyCycle { cycle: Vec<String> },
  RuntimeStopped,
  PermissionDenied,
  DoubleKey,
  Unknown,
  AuthFailed(String),
  MetadataNotFound(String),
  MissingInstances(String),
  InvalidState(String),
  EventBusError(String),
  PersistenceError(String),
  DuplicatePermissions { id: u16, name: String },
  PamError(PamError),
  Custom(String),
  System(Errno),
}

impl Display for CoreError {
  fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
    match self {
      CoreError::PamError(x) => x.fmt(f),
      CoreError::ParseError(x) => write!(f, "parse error: {x}"),
      CoreError::System(x) => write!(f, "system error: {x}"),
      CoreError::DoubleKey => write!(f, "Double key"),
      CoreError::MissingField { path } => write!(f, "missing field `{path}`"),
      CoreError::TypeMismatch { path, expected } => {
        write!(f, "type mismatch for `{path}`, expected {expected}")
      }
      CoreError::DuplicatePermissions { id, name } => {
        write!(
          f,
          "duplicate permissions for `{id}`. already registered as {name}"
        )
      }
      CoreError::MissingSchema { name } => write!(f, "missing metadata schema `{name}`"),
      CoreError::DependencyCycle { cycle } => write!(f, "dependency cycle: {}", cycle.join(" -> ")),
      CoreError::RuntimeStopped => write!(f, "runtime stopped"),
      CoreError::PermissionDenied => write!(f, "Permission Denied"),
      CoreError::Unknown => write!(f, "Unknown error"),
      CoreError::AuthFailed(e) => write!(f, "authentication failed: {e}"),
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

impl From<std::io::Error> for CoreError {
  fn from(value: std::io::Error) -> Self {
    Self::Custom(value.to_string())
  }
}

impl From<PamError> for CoreError {
  fn from(value: PamError) -> Self {
    CoreError::PamError(value)
  }
}

impl From<Errno> for CoreError {
  fn from(value: Errno) -> Self {
    CoreError::System(value)
  }
}

impl From<toml::de::Error> for CoreError {
  fn from(value: toml::de::Error) -> Self {
    CoreError::ParseError(value.to_string())
  }
}

impl From<serde_json::Error> for CoreError {
  fn from(value: serde_json::Error) -> Self {
    CoreError::ParseError(value.to_string())
  }
}

impl From<AddrParseError> for CoreError {
  fn from(value: AddrParseError) -> Self {
    CoreError::Custom(value.to_string())
  }
}

impl std::error::Error for CoreError {}

pub type CoreResult<T = ()> = std::result::Result<T, CoreError>;
