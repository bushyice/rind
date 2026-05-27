use std::io::{Read, Write};
use std::os::unix::net::UnixListener;
use std::sync::{Mutex, OnceLock};

use rind_ipc::IPC_MAGIC;
use rind_ipc::ser::{deser_from_vec, deser_string, ser_to_vec};
use rind_ipc::{Message, send::send_message};

fn socket_lock() -> &'static Mutex<()> {
  static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
  LOCK.get_or_init(|| Mutex::new(()))
}

#[test]
fn send_message_roundtrip_with_real_unix_socket() {
  let _guard = socket_lock()
    .lock()
    .expect("socket lock should be available");
  let socket_path = "/tmp/rind.sock";
  let _ = std::fs::remove_file(socket_path);

  let listener = match UnixListener::bind(socket_path) {
    Ok(listener) => listener,
    Err(err) if err.kind() == std::io::ErrorKind::PermissionDenied => {
      eprintln!("skipping socket integration test due to sandbox restriction: {err}");
      return;
    }
    Err(err) => panic!("listener should bind: {err}"),
  };
  let server = std::thread::spawn(move || {
    let (mut stream, _) = listener.accept().expect("client should connect");

    let mut magic = [0u8; 4];
    stream.read_exact(&mut magic).expect("magic must be read");
    let mut len_buf = [0u8; 4];
    stream
      .read_exact(&mut len_buf)
      .expect("length should be readable");
    let len = u32::from_be_bytes(len_buf) as usize;

    let mut buf = vec![0u8; len];
    stream
      .read_exact(&mut buf)
      .expect("payload should be readable");
    let msg: Message = deser_from_vec(&buf, false).expect("request message should parse");

    let response = Message::ok(format!("ack:{}", msg.action));
    let out = ser_to_vec(&response, false);
    stream.write_all(&IPC_MAGIC).expect("magic should write");
    stream
      .write_all(&(out.len() as u32).to_be_bytes())
      .expect("response length should write");
    stream
      .write_all(&out)
      .expect("response payload should write");
  });

  let response =
    send_message(Message::from_action("health.check")).expect("send_message should complete");
  let root = deser_string(response.payload.unwrap());

  assert_eq!(root, "ack:health.check");

  server.join().expect("server thread should finish");
  let _ = std::fs::remove_file(socket_path);
}
