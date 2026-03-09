mod flow;
pub use flow::*;

mod triggers;
pub use triggers::*;

mod transport;
pub use transport::*;

pub mod transports;

#[cfg(test)]
mod tests {
  use super::*;

  macro_rules! load_stuff {
    ($store:ident) => {
      let mut conf = rind_common::config::CONFIG.write().unwrap();
      conf.units.path = rind_common::utils::s("../examples/units");
      let state_path = format!(
        "/tmp/rind-flow-tests-{}-{}.state",
        std::process::id(),
        std::time::SystemTime::now()
          .duration_since(std::time::UNIX_EPOCH)
          .unwrap_or_default()
          .as_nanos()
      );
      let _ = std::fs::remove_file(state_path.as_str());
      conf.units.state = rind_common::utils::s(state_path.as_str());
      drop(conf);

      match crate::units::load_units() {
        Err(e) => eprintln!("{e}"),
        Ok(_) => {}
      }

      let mut $store = crate::store::STORE.write().unwrap();
      $store.load_state();
    };
  }

  #[test]
  fn change_state() {
    load_stuff!(store);

    store.load_enabled();

    store
      .set_state(
        "else@my_state".to_string(),
        Some(flow::FlowPayload::Json(
          serde_json::json!({ "id": "sss" }).to_string().into(),
        )),
        None,
      )
      .unwrap();

    store
      .set_state(
        "else@my_state".to_string(),
        Some(flow::FlowPayload::Json(
          serde_json::json!({ "id": "some_id" }).to_string().into(),
        )),
        None,
      )
      .unwrap();

    store
      .set_state(
        "else@my_state".to_string(),
        Some(flow::FlowPayload::Json(
          serde_json::json!({ "id": "jsjs" }).to_string().into(),
        )),
        None,
      )
      .unwrap();

    store
      .set_state(
        "else@some_state".to_string(),
        Some(flow::FlowPayload::String("Simple".into())),
        None,
      )
      .unwrap();

    store.remove_state(
      "else@my_state",
      Some(FlowMatchOperation::Options {
        binary: None,
        contains: None,
        r#as: Some(serde_json::json!({ "id": "sss" })),
      }),
      None,
    );

    store.load_state();

    assert_eq!(store.state_branches("else@my_state").unwrap().len(), 2);
  }

  #[test]
  fn signals() {}

  #[test]
  fn state_transcendence_apply_and_revert() {
    load_stuff!(store);
    store.states.clear();
    for (_, svc) in store.items_mut::<crate::services::Service>() {
      svc.transport = None;
    }

    if let Some(def) = store.lookup_mut::<flow::StateDefinition>("else@some_state") {
      def.0.payload = flow::FlowPayloadType::Json;
      def.0.branch = Some(vec!["tty:seat".to_string()]);
      def.0.after = Some(vec![flow::FlowItem::Simple("else@my_state".to_string())]);
    } else {
      panic!("missing else@some_state");
    }

    store
      .set_state(
        "else@my_state".to_string(),
        Some(flow::FlowPayload::Json(
          serde_json::json!({ "id": "u1", "seat": "tty1", "user": "makano" })
            .to_string()
            .into(),
        )),
        None,
      )
      .unwrap_or_else(|e| panic!("failed to set base state: {e}"));

    let trans = store
      .state_branches("else@some_state")
      .cloned()
      .unwrap_or_default();
    assert_eq!(trans.len(), 1);
    assert_eq!(
      trans[0].payload.get_json_field("tty"),
      Some(serde_json::json!("tty1"))
    );
    assert_eq!(trans[0].payload.get_json_field("seat"), None);

    store.remove_state(
      "else@my_state",
      Some(flow::FlowMatchOperation::Options {
        binary: None,
        contains: None,
        r#as: Some(serde_json::json!({ "id": "u1" })),
      }),
      None,
    );

    assert!(store.state_branches("else@my_state").is_none());
    assert!(store.state_branches("else@some_state").is_none());
  }

  #[test]
  fn activate_on_none_auto_payload() {
    load_stuff!(store);
    store.states.clear();
    for (_, svc) in store.items_mut::<crate::services::Service>() {
      svc.transport = None;
    }

    if let Some(def) = store.lookup_mut::<flow::StateDefinition>("else@some_state") {
      def.0.payload = flow::FlowPayloadType::String;
      def.0.activate_on_none = Some(vec!["else@my_state".to_string()]);
      def.0.auto_payload = Some(flow::AutoPayloadConfig {
        eval: Some("/bin/echo".to_string()),
        insert: None,
        args: Some(vec!["/dev/tty1".into()]),
      });
    } else {
      panic!("missing else@some_state");
    }

    store.reconcile_activate_on_none_boot();
    let login = store
      .state_branches("else@some_state")
      .cloned()
      .unwrap_or_default();
    assert_eq!(login.len(), 1);
    assert_eq!(login[0].payload.to_string(), "/dev/tty1".to_string());

    store
      .set_state(
        "else@my_state".to_string(),
        Some(flow::FlowPayload::Json(
          serde_json::json!({ "id": "u1" }).to_string().into(),
        )),
        None,
      )
      .unwrap_or_else(|e| panic!("failed to set blocker state: {e}"));

    assert!(store.state_branches("else@some_state").is_none());
  }

  #[test]
  fn non_branching_service_starts_on_state() {
    load_stuff!(store);
    store.states.clear();
    for (_, svc) in store.items_mut::<crate::services::Service>() {
      svc.transport = None;
    }

    store
      .set_state(
        "tr_demo@user_active".to_string(),
        Some(flow::FlowPayload::Json(
          serde_json::json!({ "id": "u1", "seat": "tty1", "user": "makano" })
            .to_string()
            .into(),
        )),
        None,
      )
      .unwrap_or_else(|e| panic!("failed to set user_active: {e}"));

    let service = store.lookup::<crate::services::Service>("tr_demo@user_session");
    assert!(service.is_some());
    let service = service.unwrap_or_else(|| panic!("missing tr_demo@user_session"));
    assert_ne!(service.state, crate::services::ServiceState::Inactive);
  }

  #[test]
  fn env_transport_state_refs_resolve_without_store_relock() {
    load_stuff!(store);
    store.states.clear();

    store
      .set_state(
        "tr_demo@login_required".to_string(),
        Some(flow::FlowPayload::String("/dev/tty1".to_string())),
        None,
      )
      .unwrap_or_else(|e| panic!("failed to set login_required: {e}"));

    let service = store.lookup::<crate::services::Service>("tr_demo@user_login");
    assert!(service.is_some());
    let service = service.unwrap_or_else(|| panic!("missing tr_demo@user_login"));
    let env = service
      .env
      .as_ref()
      .and_then(|e| e.get("RIND_LOGIN_TTY"))
      .cloned();
    assert_eq!(env, Some("/dev/tty1".to_string()));
  }
}
