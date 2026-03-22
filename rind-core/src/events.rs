use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum FlowAction {
  #[default]
  Apply,
  Revert,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FlowEvent {
  pub name: String,
  pub payload: serde_json::Value,
  pub action: FlowAction,
  pub flow_type: FlowEventType,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FlowEventType {
  State,
  Signal,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceEvent {
  pub name: String,
  pub state: ServiceEventKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ServiceEventKind {
  Started,
  Exited { code: i32 },
  Stopped,
  Failed,
}

struct ErasedChannel {
  sender: Box<dyn Any + Send + Sync>,
}

#[derive(Default, Clone)]
pub struct EventBus {
  inner: Arc<Mutex<EventBusInner>>,
}

#[derive(Default)]
struct EventBusInner {
  channels: HashMap<TypeId, Vec<ErasedChannel>>,
}

pub struct Subscription<T> {
  rx: Receiver<T>,
}

impl<T> Subscription<T> {
  pub fn try_recv(&self) -> Option<T> {
    self.rx.try_recv().ok()
  }

  pub fn drain(&self) -> Vec<T> {
    let mut out = Vec::new();
    while let Ok(item) = self.rx.try_recv() {
      out.push(item);
    }
    out
  }

  pub fn recv(&self) -> Option<T> {
    self.rx.recv().ok()
  }
}

impl EventBus {
  pub fn new() -> Self {
    Self::default()
  }

  pub fn subscribe<T: Clone + Send + 'static>(&self) -> Subscription<T> {
    let (tx, rx) = mpsc::channel();
    let mut inner = self.inner.lock().expect("event bus lock");
    let type_id = TypeId::of::<T>();
    inner
      .channels
      .entry(type_id)
      .or_default()
      .push(ErasedChannel {
        sender: Box::new(tx),
      });
    Subscription { rx }
  }

  pub fn emit<T: Clone + Send + 'static>(&self, event: T) {
    let mut inner = self.inner.lock().expect("event bus lock");
    let type_id = TypeId::of::<T>();
    if let Some(senders) = inner.channels.get_mut(&type_id) {
      senders.retain(|erased| {
        if let Some(tx) = erased.sender.downcast_ref::<Sender<T>>() {
          tx.send(event.clone()).is_ok()
        } else {
          false
        }
      });
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn subscribe_and_emit() {
    let bus = EventBus::new();
    let sub = bus.subscribe::<FlowEvent>();

    bus.emit(FlowEvent {
      name: "test@state".into(),
      payload: serde_json::json!({"id": 1}),
      action: FlowAction::Apply,
      flow_type: FlowEventType::State,
    });

    let event = sub.try_recv().expect("should receive event");
    assert_eq!(event.name, "test@state");
    assert_eq!(event.action, FlowAction::Apply);
  }

  #[test]
  fn multiple_subscribers() {
    let bus = EventBus::new();
    let sub1 = bus.subscribe::<ServiceEvent>();
    let sub2 = bus.subscribe::<ServiceEvent>();

    bus.emit(ServiceEvent {
      name: "web".into(),
      state: ServiceEventKind::Started,
    });

    assert!(sub1.try_recv().is_some());
    assert!(sub2.try_recv().is_some());
  }

  #[test]
  fn different_types_are_independent() {
    let bus = EventBus::new();
    let flow_sub = bus.subscribe::<FlowEvent>();
    let svc_sub = bus.subscribe::<ServiceEvent>();

    bus.emit(FlowEvent {
      name: "x".into(),
      payload: serde_json::Value::Null,
      action: FlowAction::Revert,
      flow_type: FlowEventType::Signal,
    });

    assert!(flow_sub.try_recv().is_some());
    assert!(svc_sub.try_recv().is_none());
  }

  #[test]
  fn dead_subscriber_is_pruned() {
    let bus = EventBus::new();
    let sub = bus.subscribe::<ServiceEvent>();
    drop(sub);

    bus.emit(ServiceEvent {
      name: "gone".into(),
      state: ServiceEventKind::Stopped,
    });
  }

  #[test]
  fn drain_collects_all() {
    let bus = EventBus::new();
    let sub = bus.subscribe::<ServiceEvent>();

    for i in 0..5 {
      bus.emit(ServiceEvent {
        name: format!("svc{i}"),
        state: ServiceEventKind::Started,
      });
    }

    let events = sub.drain();
    assert_eq!(events.len(), 5);
  }
}
