pub mod flow;
pub mod lookup;
pub mod mount;
pub mod name;
pub mod services;
pub mod store;
pub mod units;
pub mod utils;

pub use rind_common::*;

#[cfg(test)]
mod tests {
  use super::*;

  macro_rules! load_stuff {
    ($store:ident) => {
      let mut conf = rind_common::config::CONFIG.write().unwrap();
      conf.units.path = rind_common::utils::s("../examples/units");
      let state_path = format!(
        "/tmp/rind-core-tests-{}-{}.state",
        std::process::id(),
        std::time::SystemTime::now()
          .duration_since(std::time::UNIX_EPOCH)
          .unwrap_or_default()
          .as_nanos()
      );
      let _ = std::fs::remove_file(state_path.as_str());
      conf.units.state = rind_common::utils::s(state_path.as_str());
      drop(conf);

      match units::load_units() {
        Err(e) => eprintln!("{e}"),
        Ok(_) => {}
      }

      let mut $store = store::STORE.write().unwrap();
      $store.load_state();
    };
  }

  #[test]
  fn load_units() {
    load_stuff!(store);

    assert_eq!(store.len(), 4);
  }

  #[test]
  fn lookups() {
    load_stuff!(store);

    assert_eq!(
      store
        .lookup::<flow::StateDefinition>("else@my_state")
        .map(|x| x.name.clone()),
      Some("my_state".to_string())
    );
    assert_eq!(
      store
        .lookup::<flow::SignalDefinition>("else@my_signal")
        .map(|x| x.name.clone()),
      Some("my_signal".to_string())
    );
    assert_eq!(
      store
        .lookup::<services::Service>("something@one")
        .map(|x| x.name.clone()),
      Some("one".to_string())
    );

    // this check will be removed or changed when enabled becomes state-based
    // assert_eq!(
    //   store
    //     .enabled::<services::Service>()
    //     .find(|x| x.1.name == "example-active")
    //     .map(|x| x.1.name.clone()),
    //   Some("example-active".to_string())
    // );
    assert_eq!(
      store
        .lookup::<mount::Mount>("init@/proc")
        .map(|x| x.target.clone()),
      Some("/proc".to_string())
    );
  }
}
