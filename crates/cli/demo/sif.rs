// Sif = Service isolation features

use std::fs;
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

  println!("demo:start pid={pid}");
  println!("demo:cgroup {cgroup}");
  println!("demo:ns mount={}", ns_link("mnt"));
  println!("demo:ns uts={}", ns_link("uts"));
  println!("demo:ns ipc={}", ns_link("ipc"));
  println!("demo:ns net={}", ns_link("net"));

  let disable_heartbeat = std::env::var("RIND_DEMO_NO_WATCHDOG")
    .ok()
    .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
    .unwrap_or(false);

  if disable_heartbeat {
    println!("demo:watchdog disabled (expect watchdog action)");
    loop {
      thread::sleep(Duration::from_secs(60));
    }
  }

  loop {
    println!(
      r#"{{"type":"signal","name":"watchdog","payload":{{"String":"alive"}},"action":"set"}}"#
    );
    thread::sleep(Duration::from_secs(1));
  }
}
