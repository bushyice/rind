use rind_core::events::{EventBus, FlowAction, FlowEvent, FlowEventType, ServiceEvent, ServiceEventKind};
use rind_core::prelude::rslvns;

#[test]
fn subscribe_and_emit() {
  let bus = EventBus::new(None);
  let sub = bus.subscribe::<FlowEvent>();

  bus.emit(FlowEvent {
    name: rslvns!("test", "state").into(),
    payload: serde_json::json!({"id": 1}),
    action: FlowAction::Apply,
    flow_type: FlowEventType::Facet,
  });

  let event: FlowEvent = sub.try_recv().expect("should receive event");
  assert_eq!(event.name.as_str(), rslvns!("test", "state"));
  assert_eq!(event.action, FlowAction::Apply);
}

#[test]
fn multiple_subscribers() {
  let bus = EventBus::new(None);
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
  let bus = EventBus::new(None);
  let flow_sub = bus.subscribe::<FlowEvent>();
  let svc_sub = bus.subscribe::<ServiceEvent>();

  bus.emit(FlowEvent {
    name: "x".into(),
    payload: serde_json::Value::Null,
    action: FlowAction::Revert,
    flow_type: FlowEventType::Impulse,
  });

  assert!(flow_sub.try_recv().is_some());
  assert!(svc_sub.try_recv().is_none());
}

#[test]
fn dead_subscriber_is_pruned() {
  let bus = EventBus::new(None);
  let sub = bus.subscribe::<ServiceEvent>();
  drop(sub);

  bus.emit(ServiceEvent {
    name: "gone".into(),
    state: ServiceEventKind::Stopped,
  });
}

#[test]
fn drain_collects_all() {
  let bus = EventBus::new(None);
  let sub = bus.subscribe::<ServiceEvent>();

  for i in 0..5 {
    bus.emit(ServiceEvent {
      name: format!("svc{i}").into(),
      state: ServiceEventKind::Started,
    });
  }

  let events = sub.drain();
  assert_eq!(events.len(), 5);
}
