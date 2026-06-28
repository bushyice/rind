use clap::Parser;
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};

use rind_ipc::{
  Message, MessageType, payloads::Run0AuthPayload, send::send_message, ser::ser_to_vec,
};

use crate::{handle_message, report_error};

#[derive(Parser)]
#[command(name = "run0")]
#[command(version = concat!(env!("CARGO_PKG_VERSION"), "-", env!("GIT_HASH"), "-", env!("BUILD_HASH")))]
pub struct Cli {
  #[arg(short = 'n', long = "non-interactive")]
  pub non_interactive: bool,

  #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
  args: Vec<String>,
}

fn handle_run0_message(args: Vec<String>, message: Message) {
  match &message.r#type {
    MessageType::RequestInput => {
      print!("[run0] password for user: ");
      use std::io::Write;
      std::io::stdout().flush().unwrap();
      let password = rpassword::read_password().unwrap();
      print!("\r\x1b[2K");
      std::io::stdout().flush().unwrap();
      let payload = Run0AuthPayload {
        password: password.trim().to_string(),
      };
      handle_run0_message(
        args,
        match send_message(Message::from("run0").with(ser_to_vec(&payload, false))) {
          Ok(m) => m,
          Err(e) => {
            report_error("run0 request failed", e);
            return;
          }
        },
      );
    }
    MessageType::Valid => {
      if args.is_empty() {
        return;
      }

      let mut args = args.into_iter();
      let program = args.next().unwrap();

      let mut command = Command::new(program);

      command
        .args(args)
        .gid(0)
        .uid(0)
        .envs(std::env::vars())
        .current_dir(std::env::current_dir().unwrap())
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());

      let _ = command.status();
    }
    _ => handle_message(message),
  }
}

pub fn main() {
  let cli = Cli::parse();
  let mut msg = Message::from("run0");

  if cli.non_interactive {
    msg.r#type = MessageType::RequestInput;
  }

  let output = match send_message(msg) {
    Ok(m) => m,
    Err(e) => {
      report_error("run0 request failed", e);
      return;
    }
  };
  handle_run0_message(cli.args, output);
}
