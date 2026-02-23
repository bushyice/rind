use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;

fn send_command(cmd: &str) -> std::io::Result<String> {
  let mut stream = UnixStream::connect("/tmp/rind.sock")?;
  stream.write_all(cmd.as_bytes())?;
  stream.write_all(b"\n")?;

  let mut reader = BufReader::new(&stream);
  let mut response = String::new();
  reader.read_line(&mut response)?;
  Ok(response.trim().to_string())
}

fn main() {
  println!("Querying daemon");
  let output = send_command("list").unwrap();
  println!("Daemon says:\n{}", output);
}
