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
      conf.units.state = rind_common::utils::s("../examples/state");
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
}
