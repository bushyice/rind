use std::io::{Read, Write};
use std::os::unix::net::UnixStream;

use clap::Parser;
mod messaging;

#[derive(clap::Parser)]
#[command(name = "rind")]
#[command(version = env!("CARGO_PKG_VERSION"))]
#[command(about = "Rust Init Daemon")]
struct Cli {
  #[arg(short = 'L', long)]
  list: bool,

  #[arg(short = 'S', long)]
  start: bool,

  #[arg(short = 'X', long)]
  stop: bool,

  #[arg(short = 'u', long, num_args(0..=1), default_missing_value = "")]
  unit: Option<String>,

  #[arg(short = 's', long, num_args(0..=1), default_missing_value = "")]
  service: Option<String>,
}

// #[derive(clap::Subcommand)]
// enum Commands {
//   List,
//   Start { name: String },
//   Stop { name: String },
// }

fn send_command(cmd: impl Into<String>) -> std::io::Result<String> {
  let mut stream = UnixStream::connect("/tmp/rind.sock")?;

  let payload = cmd.into().into_bytes();
  let len = (payload.len() as u32).to_be_bytes();

  stream.write_all(&len)?;
  stream.write_all(&payload)?;

  let mut len_buf = [0u8; 4];
  stream.read_exact(&mut len_buf)?;
  let len = u32::from_be_bytes(len_buf) as usize;

  let mut buf = vec![0u8; len];
  stream.read_exact(&mut buf)?;

  Ok(String::from_utf8_lossy(&buf).to_string())
}

fn main() {
  let cli = Cli::parse();

  // match cli.command {
  //   Commands::List => {
  //     let output: messaging::Message = toml::from_str(
  //       &send_command(
  //         toml::to_string(&messaging::Message::from_type(messaging::MessageType::List)).unwrap(),
  //       )
  //       .unwrap(),
  //     )
  //     .unwrap();

  //     for item in output.parse_vec_payload::<String>().unwrap() {
  //       println!("{item}");
  //     }
  //   }
  //   Commands::Start { name } => {}
  //   Commands::Stop { name } => {}
  // }

  if cli.list {
    let output: messaging::Message = toml::from_str(
      &send_command(
        toml::to_string(&messaging::Message::from_type(messaging::MessageType::List)).unwrap(),
      )
      .unwrap(),
    )
    .unwrap();

    println!("{:?}", output.payload);
  }
}
