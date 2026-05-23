use rind_core::prelude::*;
use rind_flow::{EmitTrigger, FlowType};
use rind_ipc::FlowPayload;

#[derive(Default)]
pub struct EventsRuntime {
  event_rx: Option<rind_core::events::Subscription<rind_core::prelude::FlowEvent>>,
}

impl Runtime for EventsRuntime {
  fn handle(
    &mut self,
    action: &str,
    mut payload: RuntimePayload,
    ctx: &mut RuntimeContext<'_>,
    dispatch: &RuntimeDispatcher,
    _log: &LogHandle,
  ) -> Result<Option<RuntimePayload>, CoreError> {
    match action {
      "watch_events" => {
        self.event_rx = Some(ctx.event_bus.subscribe::<rind_core::prelude::FlowEvent>());
      }
      "evaluate_triggers" => {
        let emit_trig = payload.get::<EmitTrigger>("trigger").unwrap_or_default();
        let scope = payload.get::<Ustr>("scope").ok();

        let _ = dispatch.dispatch(
          "services",
          "evaluate_triggers",
          RuntimePayload::default()
            .insert("trigger", emit_trig.clone())
            .insert("scope", scope.clone()),
        );

        let _ = dispatch.dispatch(
          "sockets",
          "evaluate_triggers",
          RuntimePayload::default()
            .insert("trigger", emit_trig)
            .insert("scope", scope),
        );
      }
      "drain_events" => {
        let mut trigger = true;
        if let Some(rx) = &self.event_rx {
          while let Some(w) = rx.try_recv() {
            if w.name.as_str() == "rind:terminate_scope" {
              let scope = w.payload.as_str().unwrap_or_default().to_string();
              let _ = dispatch.dispatch(
                "services",
                "stop_for_scope",
                RuntimePayload::default().insert("scope", scope.clone()),
              );

              let _ = dispatch.dispatch(
                "sockets",
                "stop_for_scope",
                RuntimePayload::default().insert("scope", scope),
              );
            } else {
              trigger = false;

              let _ = dispatch.dispatch(
                "services",
                "drain_events",
                RuntimePayload::default().insert("event", w.clone()),
              );

              let mut trig = EmitTrigger::default();

              trig.name = Some(w.name);
              trig.payload = Some(FlowPayload::from_json(Some(w.payload)));
              trig.flow_type = Some(match w.flow_type {
                rind_core::prelude::FlowEventType::Facet => FlowType::Facet,
                rind_core::prelude::FlowEventType::Impulse => FlowType::Impulse,
              });
              trig.action = w.action;

              let _ = dispatch.dispatch(
                "events",
                "evaluate_triggers",
                RuntimePayload::default().insert("trigger", trig.clone()),
              );
            }
          }
        }

        if trigger {
          let _ = dispatch.dispatch("services", "drain_events", RuntimePayload::default());
        }
      }
      _ => {}
    }
    Ok(None)
  }

  fn id(&self) -> &str {
    "events"
  }
}
