use clap::{Parser, Subcommand};
use rind_ipc::{
  Message, MessageType,
  payloads::PermissionPayload,
  send::send_message,
  ser::ser_to_vec,
};

use crate::{handle_message, handle_send, handle_send_raw, print, send_msg};

#[derive(Parser)]
#[command(name = "sysperms")]
pub struct Cli {
  #[command(subcommand)]
  command: Command,
}

#[derive(Subcommand)]
enum Command {
  Grant {
    #[arg(name = "SUBJECT")]
    subject: String,

    #[arg(name = "PERMISSION")]
    permission: String,

    #[arg(short = 'g', long)]
    group: bool,
  },
  Revoke {
    #[arg(name = "SUBJECT")]
    subject: String,

    #[arg(name = "PERMISSION")]
    permission: String,

    #[arg(short = 'g', long)]
    group: bool,
  },
  Show {
    #[arg(long)]
    user: Option<String>,
    #[arg(long)]
    group: Option<String>,
  },
}

pub fn main() {
  let cli = Cli::parse();

  match cli.command {
    Command::Grant {
      subject,
      permission,
      group,
    } => {
      handle_send!(
        "grant_permission",
        &PermissionPayload {
          group,
          permission,
          subject
        }
      );
    }
    Command::Revoke {
      subject,
      permission,
      group,
    } => {
      handle_send!(
        "revoke_permission",
        &PermissionPayload {
          group,
          permission,
          subject
        }
      );
    }
    Command::Show { user, group } => {
      let result = send_msg!(
        "show_permissions",
        if let Some(user) = user {
          ser_to_vec(
            PermissionPayload {
              subject: user,
              group: false,
              permission: String::new(),
            },
            false,
          )
        } else if let Some(group) = group {
          ser_to_vec(
            PermissionPayload {
              subject: group,
              group: true,
              permission: String::new(),
            },
            false,
          )
        } else {
          Vec::new()
        }
      )
      .expect("Failed to send message");

      if let Ok(list) = result.parse_payload::<rind_ipc::ser::IpcListComponent>() {
        print::print_ipc_list(&list);
      } else {
        println!(
          "{}",
          String::from_utf8_lossy(&result.payload.unwrap_or_default())
        );
      }
    }
  }
}
