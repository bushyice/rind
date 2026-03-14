use std::collections::HashMap;
use std::sync::mpsc::{self, Sender};
use std::thread;

use crate::context::{RuntimeContext, RuntimeScope, RuntimeScopes};
use crate::error::CoreError;
use crate::logging::{LogHandle, LogLevel};

pub enum RuntimeCommand {
  RegisterScopes {
    context_id: usize,
    scopes: RuntimeScopes,
  },
  Dispatch {
    runtime_id: String,
    action: String,
    payload: RuntimePayload,
    context_id: usize,
  },
  Stop,
}

pub struct RuntimePayload<T: serde::de::DeserializeOwned + 'static = serde_json::Value>(pub T);

impl RuntimePayload {
  pub fn get<T: serde::de::DeserializeOwned + 'static>(
    &self,
    field: impl Into<String>,
  ) -> Result<T, CoreError> {
    let field = field.into();
    self
      .0
      .get(field.clone())
      .and_then(|v| serde_json::from_value(v.clone()).ok()?)
      .ok_or_else(|| {
        CoreError::InvalidState(format!("Missing required field \"{field}\" in dispatch"))
      })
  }
}

impl From<serde_json::Value> for RuntimePayload {
  fn from(value: serde_json::Value) -> Self {
    Self(value)
  }
}

pub trait Runtime: Send {
  fn id(&self) -> &str;
  fn handle(
    &mut self,
    action: &str,
    payload: RuntimePayload,
    ctx: &RuntimeContext<'_>,
    dispatch: &RuntimeDispatcher,
    log: &LogHandle,
  ) -> Result<(), CoreError>;
}

#[derive(Clone)]
pub struct RuntimeDispatcher {
  tx: Sender<RuntimeCommand>,
  context_id: usize,
}

impl RuntimeDispatcher {
  fn new(tx: Sender<RuntimeCommand>, context_id: usize) -> Self {
    Self { tx, context_id }
  }

  pub fn dispatch(
    &self,
    runtime_id: impl Into<String>,
    action: impl Into<String>,
    payload: RuntimePayload,
  ) -> Result<(), CoreError> {
    self
      .tx
      .send(RuntimeCommand::Dispatch {
        runtime_id: runtime_id.into(),
        action: action.into(),
        payload,
        context_id: self.context_id,
      })
      .map_err(|_| CoreError::RuntimeStopped)
  }
}

#[derive(Clone)]
pub struct RuntimeHandle {
  tx: Sender<RuntimeCommand>,
}

impl RuntimeHandle {
  pub fn send(&self, command: RuntimeCommand) -> Result<(), CoreError> {
    self.tx.send(command).map_err(|_| CoreError::RuntimeStopped)
  }

  pub fn register_scopes(&self, context_id: usize, scopes: RuntimeScopes) -> Result<(), CoreError> {
    self.send(RuntimeCommand::RegisterScopes { context_id, scopes })
  }
}

pub fn start_runtime(log: LogHandle, runtimes: Vec<Box<dyn Runtime>>) -> RuntimeHandle {
  let mut map = HashMap::<String, Box<dyn Runtime>>::new();
  for runtime in runtimes {
    map.insert(runtime.id().to_string(), runtime);
  }

  let (tx, rx) = mpsc::channel::<RuntimeCommand>();
  let worker_tx = tx.clone();
  thread::spawn(move || {
    let mut runtimes = map;
    let mut contexts = HashMap::<usize, RuntimeScopes>::new();
    while let Ok(command) = rx.recv() {
      match command {
        RuntimeCommand::RegisterScopes { context_id, scopes } => {
          contexts.insert(context_id, scopes);
        }
        RuntimeCommand::Dispatch {
          runtime_id,
          action,
          payload,
          context_id,
        } => {
          if let Some(runtime) = runtimes.get_mut(&runtime_id) {
            let fallback_scope = RuntimeScope::default();
            let scope = contexts
              .get(&context_id)
              .and_then(|scopes| scopes.scope(runtime_id.as_str()))
              .unwrap_or(&fallback_scope);
            let runtime_ctx = RuntimeContext::new(runtime_id.as_str(), scope);
            let dispatch = RuntimeDispatcher::new(worker_tx.clone(), context_id);

            if let Err(err) =
              runtime.handle(action.as_str(), payload, &runtime_ctx, &dispatch, &log)
            {
              let mut fields = HashMap::new();
              fields.insert("runtime_id".to_string(), runtime_id.clone());
              fields.insert("action".to_string(), action.clone());
              fields.insert("context_id".to_string(), context_id.to_string());
              log.log(
                LogLevel::Error,
                "runtime",
                format!("runtime dispatch failed: {err}"),
                fields,
              );
            }
          } else {
            let mut fields = HashMap::new();
            fields.insert("runtime_id".to_string(), runtime_id);
            log.log(
              LogLevel::Warn,
              "runtime",
              "runtime id not found".to_string(),
              fields,
            );
          }
        }
        RuntimeCommand::Stop => break,
      }
    }
  });

  RuntimeHandle { tx }
}
