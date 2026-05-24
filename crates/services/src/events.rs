use rind_core::prelude::*;
use rind_flow::{EmitTrigger, FlowType};
use rind_ipc::FlowPayload;
use rind_primitives::scopes::ScopeStore;

use crate::{ServiceRuntime, SocketRuntime};

#[derive(Default)]
pub struct EventsRuntime {
  event_rx: Option<rind_core::events::Subscription<rind_core::prelude::FlowEvent>>,
}

#[runtime("events")]
impl EventsRuntime {
  fn watch_events(&mut self) {
    self.event_rx = Some(ctx.event_bus.subscribe::<rind_core::prelude::FlowEvent>());
  }

  fn reload_scopes(&mut self) {
    let ss = ctx
      .registry
      .singleton_mut::<ScopeStore>(ScopeStore::KEY)
      .ok_or_else(|| CoreError::InvalidState("scope store not found".into()))?;

    if !ss.pending_scopes.is_empty() {
      ServiceRuntime::actions.bootstrap().dispatch(dispatch)?;
      SocketRuntime::actions.bootstrap().dispatch(dispatch)?;
      ServiceRuntime::actions.start_all().dispatch(dispatch)?;
      SocketRuntime::actions.setup_all().dispatch(dispatch)?;

      for scope in ss.pending_scopes.drain() {
        EventsRuntime::actions
          .evaluate_triggers()
          .scope(scope)
          .dispatch(dispatch)?;
      }
    }
  }

  fn evaluate_triggers(&mut self, #[default] trigger: EmitTrigger, #[optional] scope: Ustr) {
    ServiceRuntime::actions
      .evaluate_triggers()
      .trigger(trigger.clone())
      .scope(scope.clone().unwrap_or("static".to_ustr()))
      .dispatch(dispatch)?;

    SocketRuntime::actions
      .evaluate_triggers()
      .trigger(trigger.clone())
      .scope(scope.unwrap_or("static".to_ustr()))
      .dispatch(dispatch)?;
  }

  fn drain_events(&mut self) {
    let mut trigger = true;
    if let Some(rx) = &self.event_rx {
      while let Some(w) = rx.try_recv() {
        if w.name.as_str() == "rind:terminate_scope" {
          let scope = w.payload.as_str().unwrap_or_default().to_string();

          ServiceRuntime::actions
            .stop_for_scope(scope.clone())
            .dispatch(dispatch)?;

          SocketRuntime::actions
            .stop_for_scope(scope)
            .dispatch(dispatch)?;
        } else {
          trigger = false;

          ServiceRuntime::actions
            .drain_events()
            .event(w.clone())
            .dispatch(dispatch)?;

          let mut trig = EmitTrigger::default();

          trig.name = Some(w.name);
          trig.payload = Some(FlowPayload::from_json(Some(w.payload)));
          trig.flow_type = Some(match w.flow_type {
            rind_core::prelude::FlowEventType::Facet => FlowType::Facet,
            rind_core::prelude::FlowEventType::Impulse => FlowType::Impulse,
          });
          trig.action = w.action;

          EventsRuntime::actions
            .evaluate_triggers()
            .trigger(trig)
            .dispatch(dispatch)?;
        }
      }
    }

    if trigger {
      ServiceRuntime::actions.drain_events().dispatch(dispatch)?;
    }
  }
}
