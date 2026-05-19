use rind_core::prelude::*;
use rind_ipc::{FlowPayload, TransportMessage, TransportMessageAction, TransportMessageType};
use std::fs;
use std::io::{Write, stdout};
use std::thread;
use std::time::Duration;

fn ns_link(name: &str) -> String {
  fs::read_link(format!("/proc/self/ns/{name}"))
    .map(|p| p.display().to_string())
    .unwrap_or_else(|_| "unavailable".to_string())
}

fn main() {
  let pid = std::process::id();
  let cgroup = fs::read_to_string("/proc/self/cgroup")
    .unwrap_or_else(|_| "unavailable".to_string())
    .trim()
    .to_string();

  TransportMessage::wlog(format!("demo:start pid={pid}"));
  TransportMessage::wlog(format!("demo:cgroup {cgroup}"));
  TransportMessage::wlog(format!("demo:ns mount={}", ns_link("mnt")));
  TransportMessage::wlog(format!("demo:ns uts={}", ns_link("uts")));
  TransportMessage::wlog(format!("demo:ns ipc={}", ns_link("ipc")));
  TransportMessage::wlog(format!("demo:ns net={}", ns_link("net")));

  let disable_heartbeat = std::env::var("RIND_DEMO_NO_WATCHDOG")
    .ok()
    .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
    .unwrap_or(false);

  if disable_heartbeat {
    TransportMessage::log(format!("demo:watchdog disabled (expect watchdog action)"));
    loop {
      thread::sleep(Duration::from_secs(60));
    }
  }

  loop {
    let msg = TransportMessage {
      r#type: TransportMessageType::Impulse,
      name: Some("watchdog".to_ustr()),
      payload: Some(FlowPayload::String("alive".to_string())),
      action: TransportMessageAction::Set,
      branch: None,
    };

    let mut out = stdout();
    if let Err(e) = msg.write_signed(&mut out) {
      eprintln!("failed to send message: {e}");
    }
    let _ = out.flush();

    thread::sleep(Duration::from_secs(1));
  }
}
