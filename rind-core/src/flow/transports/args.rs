use crate::services::Service;

use super::*;

#[derive(Default)]
pub struct ArgsTransportProtocol;

impl TransportProtocol for ArgsTransportProtocol {
  fn init(
    &mut self,
    _options: Vec<String>,
    ctx: &TransportInitContext,
    service: Option<&mut Service>,
  ) {
    if !matches!(ctx.stage, TransportInitStage::ServicePreStart) {
      return;
    }
    let _ = service;
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
