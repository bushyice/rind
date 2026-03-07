use crate::services::Service;
use rind_common::error::rw_read;

use super::*;

#[derive(Default)]
pub struct EnvTransportProtocol;

impl TransportProtocol for EnvTransportProtocol {
  fn init(
    &mut self,
    options: Vec<String>,
    ctx: &TransportInitContext,
    service: Option<&mut Service>,
  ) {
    if !matches!(ctx.stage, TransportInitStage::ServicePreStart) {
      return;
    }
    let Some(service) = service else {
      return;
    };

    let env = service
      .env
      .get_or_insert_with(std::collections::HashMap::new);
    for option in options {
      let Some((key, value)) = option.split_once('=') else {
        continue;
      };
      if let Some(state_name) = value.strip_prefix("state:") {
        let store = rw_read(&crate::store::STORE, "store read in env transport");
        if let Some(val) = store
          .state_branches(state_name)
          .and_then(|v| v.first())
          .map(|x| x.payload.to_string())
        {
          env.insert(key.to_string(), val);
        }
      } else {
        env.insert(key.to_string(), value.to_string());
      }
    }
  }

  fn recv(
    &self,
    _ctx: &mut TransportContext,
    _instance: &FlowInstance,
    _service: Option<&mut Service>,
  ) -> TransportResult {
    Ok(None)
  }
}
