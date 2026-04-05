use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};

fn handle_client(mut stream: TcpStream) {
  let mut buffer = [0; 1024];

  match stream.read(&mut buffer) {
    Ok(bytes_read) => {
      if bytes_read == 0 {
        return;
      }

      let request = String::from_utf8_lossy(&buffer[..bytes_read]);
      println!("received: {}", request);

      let response = b"hello\n";
      stream.write_all(response).unwrap();
    }
    Err(e) => {
      eprintln!("read error: {}", e);
    }
  }
}

fn main() {
  let host = std::env::var("HOST").unwrap_or("0.0.0.0".to_string());
  let port = std::env::var("PORT").unwrap_or("8080".to_string());
  let listener = TcpListener::bind(format!("{host}:{port}")).unwrap();
  println!("Server listening on port {port}...");

  for stream in listener.incoming() {
    match stream {
      Ok(stream) => {
        println!("client connected");

        handle_client(stream);
      }
      Err(e) => {
        eprintln!("connection failed: {}", e);
      }
    }
  }
}
