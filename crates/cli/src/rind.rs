use std::os::unix::process::CommandExt;

use clap::Parser;
use libc::seteuid;
use owo_colors::OwoColorize;
use rind_ipc::{Message, MessageType, send::send_message, ser::ser_to_vec};

mod applets;
mod macros;
mod print;

#[derive(clap::Parser)]
#[command(name = "rind")]
#[command(version = concat!(env!("CARGO_PKG_VERSION"), "-", env!("GIT_HASH"), "-", env!("BUILD_HASH")))]
#[command(about = "Rust Init Daemon")]
#[command(after_help = "\
\x1b[1;36mApplets:\x1b[0m
  \x1b[97mrun0     \x1b[0m Execute privileged operations\x1b[0m
  \x1b[97msysinvoke\x1b[0m Invoke IPC actions\x1b[0m
  \x1b[97msysunit  \x1b[0m Manage units\x1b[0m
  \x1b[97msyslogs  \x1b[0m View logs\x1b[0m
  \x1b[97msysperms \x1b[0m Permission management\x1b[0m
  \x1b[97msyssess  \x1b[0m Session management\x1b[0m
")]
struct Cli {
  #[command(subcommand)]
  command: Commands,
}

#[derive(clap::Subcommand)]
enum Commands {
  ReloadUnits {
    #[arg(short = 'a', long = "static")]
    all: bool,
  },
  SoftReboot,
  Reboot,
  Shutdown,
  #[cfg(feature = "applet-exec")]
  Applet {
    #[arg(name = "APPLET")]
    applet: String,

    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    args: Vec<String>,
  },
}

pub fn report_error(msg: &str, err: impl std::fmt::Display) {
  eprintln!("{} {}: {}", "Error".on_red().black(), msg, err);
}

pub fn handle_message(message: Message) {
  match message.r#type {
    MessageType::Ok => {
      println!(
        "{} {}",
        "Ok".on_green().black(),
        message
          .payload
          .as_ref()
          .map(|p| rind_ipc::ser::deser_string(p))
          .unwrap_or_else(|| "ok".to_string())
      );
    }
    MessageType::Error => {
      println!(
        "{} {}",
        "Error".on_red().black(),
        message
          .payload
          .as_ref()
          .map(|p| rind_ipc::ser::deser_string(p))
          .unwrap_or_else(|| "unknown error".to_string())
      )
    }
    _ => {}
  }
}

pub fn apply_scope_name(name: &str, scope: Option<&str>) -> String {
  let Some(scope) = scope else {
    return name.to_string();
  };
  if scope.is_empty() || scope == "static" || name.contains('@') {
    return name.to_string();
  }
  format!("{name}@{scope}")
}

fn main() {
  let argv0 = std::env::args().next().unwrap();
  let current = std::path::Path::new(&argv0)
    .file_stem()
    .unwrap()
    .to_str()
    .unwrap();

  if let Some(applet) = applets::APPLETS.iter().find(|a| a.name == current) {
    if current != "run0" {
      unsafe {
        seteuid(libc::getuid());
      }
    }

    (applet.entry)();
    return;
  }

  let cli = Cli::parse();

  match cli.command {
    #[cfg(feature = "applet-exec")]
    Commands::Applet {
      applet: current,
      args,
    } => {
      let Ok(exe) = std::env::current_exe() else {
        eprintln!("Failed to get current exec path");
        return;
      };

      let err = std::process::Command::new(&exe)
        .arg0(current)
        .args(args)
        .exec();

      panic!("exec failed: {err}");
    }
    Commands::ReloadUnits { all } => {
      handle_send!("reload_units", &all);
    }
    Commands::SoftReboot => {
      handle_send!("soft_reboot", &());
    }
    Commands::Reboot => {
      handle_send!("reboot", &());
    }
    Commands::Shutdown => {
      handle_send!("shutdown", &());
    }
  }
}
