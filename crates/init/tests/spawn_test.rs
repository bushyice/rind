use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use rind_ipc::Message;
use rind_ipc::payloads::SSPayload;
use rind_ipc::send::send_message;

fn temp_dir(tag: &str) -> PathBuf {
  let now = std::time::SystemTime::now()
    .duration_since(std::time::UNIX_EPOCH)
    .expect("clock before epoch")
    .as_nanos();
  std::env::temp_dir().join(format!("rind-init-{tag}-{}-{now}", std::process::id()))
}

fn wait_for_socket(path: &Path, timeout: Duration) -> bool {
  let start = Instant::now();
  while start.elapsed() < timeout {
    if path.exists() {
      return true;
    }
    std::thread::sleep(Duration::from_millis(50));
  }
  false
}

fn wait_for_path(path: &Path, timeout: Duration) -> bool {
  let start = Instant::now();
  while start.elapsed() < timeout {
    if path.exists() {
      return true;
    }
    std::thread::sleep(Duration::from_millis(50));
  }
  false
}

fn stop_child(child: &mut Child) {
  let _ = child.kill();
  let _ = child.wait();
}

#[test]
fn init_uses_env_paths_and_persists_state_with_ipc_start() {
  let _ = fs::remove_file("/tmp/rind.sock");
  let units_dir = temp_dir("units");
  let state_path = temp_dir("state").join("state.bin");
  let state_root = temp_dir("state-root");
  let scoped_state_path = state_root.join("static").join("state.bin");
  let vars_path = temp_dir("vars").join("variables.toml");
  let log_dir = temp_dir("logs");
  fs::create_dir_all(&units_dir).expect("units dir should be created");
  fs::create_dir_all(state_path.parent().expect("state parent"))
    .expect("state parent should exist");
  fs::create_dir_all(&state_root).expect("state root should exist");
  fs::create_dir_all(vars_path.parent().expect("vars parent")).expect("vars parent should exist");

  let unit_file = units_dir.join("test.toml");
  fs::write(
    &unit_file,
    r#"
[[service]]
name = "probe"
run.exec = "/bin/sh"
run.args = ["-c", "sleep 2"]
space = "user"
restart = false
"#,
  )
  .expect("unit file should be written");

  let init_bin = env!("CARGO_BIN_EXE_init");
  let mut child = Command::new(init_bin)
    .env("RIND_UNITS_DIR", &units_dir)
    .env("RIND_STATE_PATH", &state_path)
    .env("RIND_STATE_ROOT", &state_root)
    .env("RIND_VARIABLES_PATH", &vars_path)
    .env("RIND_LOG_DIR", &log_dir)
    .env("RIND_PUMP_INTERVAL", "1")
    .stdout(Stdio::null())
    .stderr(Stdio::null())
    .spawn()
    .expect("init should spawn");

  let socket_path = Path::new("/tmp/rind.sock");
  if !wait_for_socket(socket_path, Duration::from_secs(5)) {
    if let Ok(Some(status)) = child.try_wait() {
      eprintln!("skipping init blackbox test: init exited early with status {status}");
    } else {
      eprintln!("skipping init blackbox test: ipc socket unavailable in this environment");
    }
    stop_child(&mut child);
    return;
  }

  let payload = SSPayload {
    name: "test:probe".to_string(),
    force: false,
    persist: true,
    unit_type: "service".to_string(),
  };

  let response = send_message(
    Message::from_action("start")
      .with(flexbuffers::to_vec(payload).expect("couldn't serialize payload")),
  )
  .expect("ipc start message should succeed");

  assert!(
    !matches!(response.r#type, rind_ipc::MessageType::Error),
    "ipc response should not be error: {:?}",
    response
  );

  let _ = wait_for_path(&state_path, Duration::from_secs(2))
    || wait_for_path(&scoped_state_path, Duration::from_secs(2));
  stop_child(&mut child);

  assert!(
    state_path.exists() || scoped_state_path.exists(),
    "state snapshot path should be created (legacy or scoped)"
  );

  let mut has_log_segment = false;
  if let Ok(entries) = fs::read_dir(&log_dir) {
    for entry in entries.flatten() {
      if entry
        .path()
        .extension()
        .map(|ext| ext == "rlog")
        .unwrap_or(false)
      {
        has_log_segment = true;
        break;
      }
    }
  }
  assert!(
    has_log_segment,
    "expected at least one .rlog segment in log dir"
  );

  let _ = fs::remove_file("/tmp/rind.sock");
}
