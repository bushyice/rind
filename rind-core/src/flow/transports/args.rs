use crate::services::Service;
use rind_common::error::rw_read;

use super::*;

#[derive(Default)]
pub struct ArgsTransportProtocol;

impl TransportProtocol for ArgsTransportProtocol {
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

    for option in options {
      if let Some(state_name) = option.strip_prefix("state:") {
        let store = rw_read(&crate::store::STORE, "store read in args transport");
        let payload = store
          .state_branches(state_name)
          .and_then(|v| v.first())
          .map(|x| x.payload.to_string())
          .unwrap_or_default();
        if !payload.is_empty() {
          service.args.push(payload);
        }
      } else {
        service.args.push(option);
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
