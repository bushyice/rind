use std::any::Any;
use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;
use std::sync::mpsc::Sender;

use crate::context::{RuntimeContext, RuntimeScopes};
use crate::error::CoreError;
use crate::events::EventBus;
use crate::lifecycle::{LifecycleAction, LifecycleQueue};
use crate::logging::{LogHandle, LogLevel};
use crate::notifier::Notifier;
use crate::prelude::Resources;
use crate::registry::{InstanceMap, InstanceRegistry, MetadataRegistry};
use crate::types::Ustr;

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
    reply: Option<Sender<Result<RuntimePayload, CoreError>>>,
  },
  Stop,
}

#[derive(Default)]
pub struct RuntimePayload {
  items: HashMap<Ustr, Box<dyn Any + Send + Sync>>,
}

impl RuntimePayload {
  pub fn insert<T: Send + Sync + 'static>(mut self, name: impl Into<Ustr>, item: T) -> Self {
    self.items.insert(name.into(), Box::new(item));
    self
  }

  pub fn get<T: Send + Sync + 'static>(&mut self, name: impl Into<Ustr>) -> Result<T, CoreError> {
    let name = name.into();
    self
      .items
      .remove(&name)
      .and_then(|v| match v.downcast::<T>() {
        Ok(v) => Some(*v),
        Err(e) => {
          self.items.insert(name.clone(), e);
          None
        }
      })
      .ok_or_else(|| {
        CoreError::InvalidState(format!(
          "Missing required field {name} in dispatch. keys: {:?}",
          self.items.keys()
        ))
      })
  }
}

#[macro_export]
macro_rules! rpayload {
  ({ $($key:literal : $value:expr),* $(,)? }) => {
    RuntimePayload::default()
    $(
        .insert($key, $value)
    )*
  };
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
  ) -> Result<Option<RuntimePayload>, CoreError>;
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
      reply: None,
    })
  }

  #[deprecated]
  pub fn call(
    &self,
    runtime_id: impl Into<String>,
    action: impl Into<String>,
    payload: RuntimePayload,
  ) -> Result<Option<RuntimePayload>, CoreError> {
    let (tx, rx) = std::sync::mpsc::channel();

    self.handle.send(RuntimeCommand::Dispatch {
      runtime_id: runtime_id.into(),
      action: action.into(),
      payload,
      context_id: self.context_id,
      reply: Some(tx),
    })?;

    rx.recv()
      .map_err(|_| CoreError::InvalidState("Runtime failed to reply (deadlock?)".into()))?
      .map(Some)
  }
}

#[derive(Clone)]
pub struct RuntimeHandle {
  inner: Rc<RefCell<RuntimeEngine>>,
}

struct RuntimeEngine {
  log: LogHandle,
  runtimes: HashMap<String, Box<dyn Runtime>>,
  contexts: HashMap<usize, RuntimeContextState>,
  queue: VecDeque<RuntimeCommand>,
  instances: InstanceMap,
  pub notifier: Option<Notifier>,
  stopped: bool,
}

struct RuntimeContextState {
  scopes: RuntimeScopes,
  event_bus: EventBus,
  lifecycle: LifecycleQueue,
}

impl RuntimeHandle {
  pub fn send(&self, command: RuntimeCommand) -> Result<(), CoreError> {
    let mut inner = self.inner.borrow_mut();
    if inner.stopped {
      return Err(CoreError::RuntimeStopped);
    }

    match command {
      RuntimeCommand::RegisterScopes { context_id, scopes } => {
        let notifier = inner.notifier.clone();
        inner.contexts.insert(
          context_id,
          RuntimeContextState {
            scopes,
            event_bus: EventBus::new(notifier),
            lifecycle: LifecycleQueue::default(),
          },
        );
      }
      RuntimeCommand::Stop => {
        inner.stopped = true;
        inner.queue.clear();
      }
      other => inner.queue.push_back(other),
    }

    Ok(())
  }

  pub fn log(
    &self,
    level: LogLevel,
    target: &str,
    message: &str,
    fields: HashMap<String, String>,
  ) -> Result<(), CoreError> {
    let inner = self.inner.borrow();
    if inner.stopped {
      return Err(CoreError::RuntimeStopped);
    }

    inner.log.log(level, target, message, fields);

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
      reply: None,
    })
  }

  #[deprecated]
  pub fn call(
    &self,
    runtime_id: impl Into<String>,
    action: impl Into<String>,
    payload: RuntimePayload,
    context_id: usize,
  ) -> Result<Option<RuntimePayload>, CoreError> {
    let (tx, rx) = std::sync::mpsc::channel();

    self.send(RuntimeCommand::Dispatch {
      runtime_id: runtime_id.into(),
      action: action.into(),
      payload,
      context_id,
      reply: Some(tx),
    })?;

    rx.recv()
      .map_err(|_| CoreError::InvalidState("Runtime failed to reply (deadlock?)".into()))?
      .map(Some)
  }

  pub fn register_scopes(&self, context_id: usize, scopes: RuntimeScopes) -> Result<(), CoreError> {
    self.send(RuntimeCommand::RegisterScopes { context_id, scopes })
  }

  pub fn next_lifecycle_action(&self, context_id: usize) -> Option<LifecycleAction> {
    let mut inner = self.inner.borrow_mut();
    inner
      .contexts
      .get_mut(&context_id)
      .and_then(|ctx| ctx.lifecycle.next())
  }

  pub fn with_instances<R>(&self, f: impl FnOnce(&mut InstanceMap) -> R) -> Result<R, CoreError> {
    let mut inner = self.inner.borrow_mut();
    if inner.stopped {
      return Err(CoreError::RuntimeStopped);
    }
    Ok(f(&mut inner.instances))
  }

  pub fn flush_context(
    &self,
    context_id: usize,
    metadata: &MetadataRegistry,
    resources: &mut Resources,
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
        reply,
      }) = command
      else {
        break;
      };
      // println!("Gotten {action} for {runtime_id}");

      let (mut runtime, mut scope, mut event_bus, mut lifecycle, mut instances, log) = {
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

        let context = inner.contexts.get_mut(&cid).ok_or_else(|| {
          CoreError::InvalidState(format!("runtime context {cid} not registered"))
        })?;

        let scope = context.scopes.take_or_build_scope(runtime_id.as_str());
        let event_bus = std::mem::take(&mut context.event_bus);
        let lifecycle = std::mem::take(&mut context.lifecycle);

        // CHECK
        let instances = std::mem::take(&mut inner.instances);
        let log = inner.log.clone();
        (runtime, scope, event_bus, lifecycle, instances, log)
      };

      let registry = InstanceRegistry::new(metadata, &mut instances);
      let mut ctx = RuntimeContext::new(
        runtime_id.as_str(),
        &mut scope,
        registry,
        resources,
        &mut event_bus,
        &mut lifecycle,
        self.inner.borrow().notifier.clone(),
      );
      let dispatch = RuntimeDispatcher::new(self.clone(), cid);

      // println!("Calling runtime: {action}");
      let result = runtime.handle(action.as_str(), payload, &mut ctx, &dispatch, &log);
      // println!("Called runtime: {action}");

      if let Some(reply_tx) = reply {
        let _ = reply_tx.send(match result {
          Ok(Some(msg)) => Ok(msg),
          Ok(None) => Err(CoreError::InvalidState("No response".into())),
          Err(e) => Err(e),
        });
      } else {
        if let Err(err) = result {
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
      }

      {
        let mut inner = self.inner.borrow_mut();
        inner.runtimes.insert(runtime_id.clone(), runtime);
        inner.instances = instances;
        if let Some(context) = inner.contexts.get_mut(&cid) {
          context.scopes.put_scope(runtime_id, scope);
          context.event_bus = event_bus;
          context.lifecycle = lifecycle;
        }
      }
    }

    Ok(())
  }
}

pub fn start_runtime(
  log: LogHandle,
  runtimes: Vec<Box<dyn Runtime>>,
  notifier: Option<Notifier>,
) -> RuntimeHandle {
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
      notifier,
      stopped: false,
    })),
  }
}
