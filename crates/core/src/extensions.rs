use std::{
  any::{Any, TypeId},
  collections::HashMap,
};

use once_cell::unsync::OnceCell;

use crate::{
  error::CoreResult,
  logging::LogHandle,
  prelude::InstanceRegistry,
  runtime::{RuntimeDispatcher, RuntimePayload},
};

thread_local! {
  pub static EXTENSIONS: OnceCell<ExtensionManager> = OnceCell::new();
}

pub type ExtensionEnquire<R> = fn(name: &str) -> CoreResult<R>;

pub type ExtensionAct<T> = fn(name: &str, input: &mut T) -> CoreResult<()>;

pub type ExtensionResolve<T> = fn(name: &str, input: T) -> CoreResult<T>;

pub enum ExtensionResponseAction {
  Dispatch {
    runtime: &'static str,
    action: &'static str,
    payload: Option<RuntimePayload>,
  },
  Function(
    Box<
      dyn Fn(
        Option<&RuntimeDispatcher>,
        Option<&LogHandle>,
        Option<&mut InstanceRegistry<'_>>,
      ) -> CoreResult<Box<dyn Any>>,
    >,
  ),
}

pub struct ExtensionExecutionCtx<T> {
  pub target: T,
  pub response: Option<ExtensionResponseAction>,
}

impl<T> ExtensionExecutionCtx<T> {
  pub fn new(target: T) -> Self {
    Self {
      target,
      response: None,
    }
  }

  pub fn with_dispatch(
    mut self,
    runtime: &'static str,
    action: &'static str,
    payload: Option<RuntimePayload>,
  ) -> Self {
    self.response = Some(ExtensionResponseAction::Dispatch {
      runtime,
      action,
      payload,
    });
    self
  }

  pub fn with_fn(
    mut self,
    f: impl Fn(
      Option<&RuntimeDispatcher>,
      Option<&LogHandle>,
      Option<&mut InstanceRegistry<'_>>,
    ) -> CoreResult<Box<dyn Any>>
    + 'static,
  ) -> Self {
    self.response = Some(ExtensionResponseAction::Function(Box::new(f)));
    self
  }

  pub fn dispatch(
    self,
    dispatch: Option<&RuntimeDispatcher>,
    log: Option<&LogHandle>,
    registry: Option<&mut InstanceRegistry<'_>>,
  ) -> CoreResult<Box<dyn Any>> {
    if let Some(action) = self.response {
      match action {
        ExtensionResponseAction::Dispatch {
          runtime,
          action,
          payload,
        } => {
          if let Some(dispatch) = dispatch {
            dispatch.dispatch(runtime, action, payload.unwrap_or_default())?
          }
        }
        ExtensionResponseAction::Function(f) => return f(dispatch, log, registry),
      }
    }

    Ok(Box::new(()))
  }
}

pub enum Extension<T> {
  Enquire(ExtensionEnquire<T>),
  Act(ExtensionAct<T>),
  Resolve(ExtensionResolve<T>),
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ExtensionType {
  Enquire,
  Act,
  Resolve,
}

#[derive(Default)]
pub struct ExtensionManager {
  enquire: HashMap<TypeId, Vec<Box<dyn Any>>>,
  act: HashMap<TypeId, Vec<Box<dyn Any>>>,
  resolve: HashMap<TypeId, Vec<Box<dyn Any>>>,
}

impl ExtensionManager {
  pub fn enquire<T: 'static>(&self, name: &str) -> CoreResult<Vec<T>> {
    let Some(exts) = self.enquire.get(&TypeId::of::<T>()) else {
      return Ok(Vec::new());
    };

    let mut results = Vec::new();

    for ext in exts.iter() {
      if let Some(ext) = ext.downcast_ref::<ExtensionEnquire<T>>() {
        results.push(ext(name)?);
      }
    }

    Ok(results)
  }

  pub fn act<T: 'static>(&self, name: &str, v: &mut T) -> CoreResult<()> {
    let Some(exts) = self.act.get(&TypeId::of::<T>()) else {
      return Ok(());
    };

    for ext in exts.iter() {
      if let Some(ext) = ext.downcast_ref::<ExtensionAct<T>>() {
        ext(name, v)?;
      }
    }

    Ok(())
  }

  pub fn resolve<T: 'static>(&self, name: &str, v: T) -> CoreResult<T> {
    let Some(exts) = self.resolve.get(&TypeId::of::<T>()) else {
      return Ok(v);
    };

    let mut result = v;

    for ext in exts.iter() {
      if let Some(ext) = ext.downcast_ref::<ExtensionResolve<T>>() {
        result = ext(name, result)?;
      }
    }

    Ok(result)
  }

  fn get_entry<T: 'static>(&mut self, t: ExtensionType) -> &mut Vec<Box<dyn Any>> {
    match t {
      ExtensionType::Act => self.act.entry(TypeId::of::<T>()).or_default(),
      ExtensionType::Enquire => self.enquire.entry(TypeId::of::<T>()).or_default(),
      ExtensionType::Resolve => self.resolve.entry(TypeId::of::<T>()).or_default(),
    }
  }

  pub fn register<T: 'static>(&mut self, ext: Extension<T>) {
    match ext {
      Extension::Act(e) => self.get_entry::<T>(ExtensionType::Act).push(Box::new(e)),
      Extension::Enquire(e) => self
        .get_entry::<T>(ExtensionType::Enquire)
        .push(Box::new(e)),
      Extension::Resolve(e) => self
        .get_entry::<T>(ExtensionType::Resolve)
        .push(Box::new(e)),
    }
  }
}

#[cfg(test)]
mod tests {
  use super::{Extension, ExtensionExecutionCtx, ExtensionManager};

  fn enquire_len(name: &str) -> crate::error::CoreResult<usize> {
    Ok(name.len())
  }

  fn act_push(_name: &str, input: &mut Vec<String>) -> crate::error::CoreResult<()> {
    input.push("acted".to_string());
    Ok(())
  }

  fn resolve_suffix(_name: &str, input: String) -> crate::error::CoreResult<String> {
    Ok(format!("{input}-resolved"))
  }

  #[test]
  fn manager_executes_enquire_act_and_resolve_extensions() {
    let mut mgr = ExtensionManager::default();
    mgr.register::<usize>(Extension::Enquire(enquire_len));
    mgr.register::<Vec<String>>(Extension::Act(act_push));
    mgr.register::<String>(Extension::Resolve(resolve_suffix));

    let results = mgr
      .enquire::<usize>("abcd")
      .expect("enquire should return result");
    assert_eq!(results, vec![4usize]);

    let mut target = Vec::<String>::new();
    mgr.act::<Vec<String>>("anything", &mut target)
      .expect("act should mutate target");
    assert_eq!(target, vec!["acted".to_string()]);

    let resolved = mgr
      .resolve::<String>("name", "value".to_string())
      .expect("resolve should transform");
    assert_eq!(resolved, "value-resolved".to_string());
  }

  #[test]
  fn execution_ctx_with_fn_dispatches_custom_response() {
    let out = ExtensionExecutionCtx::new(())
      .with_fn(|_, _, _| Ok(Box::new(11u32)))
      .dispatch(None, None, None)
      .expect("dispatch should run function response");
    let value = out.downcast_ref::<u32>().expect("result type should be u32");
    assert_eq!(*value, 11);
  }
}
