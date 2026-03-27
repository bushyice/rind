use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;

use crate::context::{RuntimeContext, RuntimeScopes, RuntimeSpace};
use crate::error::CoreError;
use crate::logging::{LogHandle, LogLevel};
use crate::registry::{InstanceMap, InstanceRegistry, MetadataRegistry};

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

  pub fn r#as<T: serde::de::DeserializeOwned + 'static>(&self) -> Result<T, CoreError> {
    serde_json::from_value(self.0.clone()).map_err(CoreError::custom)
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
    ctx: &mut RuntimeContext<'_>,
    dispatch: &RuntimeDispatcher,
    log: &LogHandle,
  ) -> Result<(), CoreError>;
}

#[derive(Clone)]
pub struct RuntimeDispatcher {
  handle: RuntimeHandle,
  context_id: usize,
}

impl RuntimeDispatcher {
  fn new(handle: RuntimeHandle, context_id: usize) -> Self {
    Self { handle, context_id }
  }

  pub fn dispatch(
    &self,
    runtime_id: impl Into<String>,
    action: impl Into<String>,
    payload: RuntimePayload,
  ) -> Result<(), CoreError> {
    self.handle.send(RuntimeCommand::Dispatch {
      runtime_id: runtime_id.into(),
      action: action.into(),
      payload,
      context_id: self.context_id,
    })
  }
}

#[derive(Clone)]
pub struct RuntimeHandle {
  inner: Rc<RefCell<RuntimeEngine>>,
}

struct RuntimeEngine {
  log: LogHandle,
  runtimes: HashMap<String, Box<dyn Runtime>>,
  contexts: HashMap<usize, RuntimeScopes>,
  queue: VecDeque<RuntimeCommand>,
  instances: InstanceMap,
  stopped: bool,
}

impl RuntimeHandle {
  pub fn send(&self, command: RuntimeCommand) -> Result<(), CoreError> {
    let mut inner = self.inner.borrow_mut();
    if inner.stopped {
      return Err(CoreError::RuntimeStopped);
    }

    match command {
      RuntimeCommand::RegisterScopes { context_id, scopes } => {
        inner.contexts.insert(context_id, scopes);
      }
      RuntimeCommand::Stop => {
        inner.stopped = true;
        inner.queue.clear();
      }
      other => inner.queue.push_back(other),
    }

    Ok(())
  }

  pub fn dispatch(
    &self,
    target: &str,
    action: &str,
    payload: RuntimePayload,
    context_id: usize,
  ) -> Result<(), CoreError> {
    self.send(RuntimeCommand::Dispatch {
      runtime_id: target.to_string(),
      action: action.to_string(),
      payload,
      context_id,
    })
  }

  pub fn register_scopes(&self, context_id: usize, scopes: RuntimeScopes) -> Result<(), CoreError> {
    self.send(RuntimeCommand::RegisterScopes { context_id, scopes })
  }

  pub fn flush_context(
    &self,
    context_id: usize,
    metadata: &MetadataRegistry,
    space: RuntimeSpace,
  ) -> Result<(), CoreError> {
    loop {
      let command = {
        let mut inner = self.inner.borrow_mut();
        let idx = inner
          .queue
          .iter()
          .position(|cmd| matches!(cmd, RuntimeCommand::Dispatch { context_id: cid, .. } if *cid == context_id));

        match idx {
          Some(i) => inner.queue.remove(i),
          None => None,
        }
      };

      let Some(RuntimeCommand::Dispatch {
        runtime_id,
        action,
        payload,
        context_id: cid,
      }) = command
      else {
        break;
      };

      let (mut runtime, mut scope, mut instances, log) = {
        let mut inner = self.inner.borrow_mut();
        if inner.stopped {
          return Err(CoreError::RuntimeStopped);
        }

        let runtime = match inner.runtimes.remove(&runtime_id) {
          Some(runtime) => runtime,
          None => {
            let mut fields = HashMap::new();
            fields.insert("runtime_id".to_string(), runtime_id.clone());
            inner.log.log(
              LogLevel::Warn,
              "runtime",
              "runtime id not found".to_string(),
              fields,
            );
            continue;
          }
        };

        let scope = inner
          .contexts
          .get_mut(&cid)
          .map(|scopes| scopes.take_or_build_scope(runtime_id.as_str()))
          .unwrap_or_default();

        // CHECK
        let instances = std::mem::take(&mut inner.instances);
        let log = inner.log.clone();
        (runtime, scope, instances, log)
      };

      let registry = InstanceRegistry::new(metadata, &mut instances);
      let mut ctx = RuntimeContext::new(runtime_id.as_str(), &mut scope, registry);
      ctx.space = space.clone();
      let dispatch = RuntimeDispatcher::new(self.clone(), cid);

      if let Err(err) = runtime.handle(action.as_str(), payload, &mut ctx, &dispatch, &log) {
        let mut fields = HashMap::new();
        fields.insert("runtime_id".to_string(), runtime_id.clone());
        fields.insert("action".to_string(), action.clone());
        fields.insert("context_id".to_string(), cid.to_string());
        log.log(
          LogLevel::Error,
          "runtime",
          format!("runtime dispatch failed: {err}"),
          fields,
        );
      }

      {
        let mut inner = self.inner.borrow_mut();
        inner.runtimes.insert(runtime_id.clone(), runtime);
        inner.instances = instances;
        if let Some(scopes) = inner.contexts.get_mut(&cid) {
          scopes.put_scope(runtime_id, scope);
        }
      }
    }

    Ok(())
  }
}

pub fn start_runtime(log: LogHandle, runtimes: Vec<Box<dyn Runtime>>) -> RuntimeHandle {
  let mut map = HashMap::<String, Box<dyn Runtime>>::new();
  for runtime in runtimes {
    map.insert(runtime.id().to_string(), runtime);
  }

  RuntimeHandle {
    inner: Rc::new(RefCell::new(RuntimeEngine {
      log,
      runtimes: map,
      contexts: HashMap::new(),
      queue: VecDeque::new(),
      instances: InstanceMap::default(),
      stopped: false,
    })),
  }
}
