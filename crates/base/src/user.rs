// Users "exist" but-
// - PAM sucsk
// - Need to expose more APIs

use std::{
  collections::HashMap,
  fs,
  os::unix::fs::{PermissionsExt, chown},
  path::PathBuf,
  sync::Arc,
};

use rind_core::prelude::*;
use rind_ipc::{
  Message, MessageType,
  payloads::{LoginPayload, LogoutPayload, Run0AuthPayload},
};

use crate::{
  flow::StateMachineShared,
  ipc::{payload_msg, payload_to},
  permissions::PERM_RUN0,
};

#[derive(Debug, Default)]
enum Run0State {
  #[default]
  Inactive,
  RequireAuth,
}

#[derive(Default)]
pub struct UserRuntime {
  run0_queue: HashMap<i32, Run0State>,
}

fn runtime_dir(uid: u32) -> PathBuf {
  PathBuf::from(format!("/run/user/{}", uid))
}

impl UserRuntime {
  fn create_runtime_dir(&self, user: &UserRecord) -> std::io::Result<()> {
    let dir = runtime_dir(user.uid);

    if !dir.exists() {
      fs::create_dir_all(&dir)?;
      fs::set_permissions(&dir, fs::Permissions::from_mode(0o700))?;

      chown(&dir, Some(user.uid), Some(user.gid))?;
    }

    Ok(())
  }

  fn remove_runtime_dir(&self, user: &UserRecord, pam: &Arc<PamHandle>) -> std::io::Result<()> {
    if !pam.has_active_session(&user.username) {
      let dir = runtime_dir(user.uid);
      if dir.exists() {
        fs::remove_dir_all(dir)?;
      }
    }

    Ok(())
  }
}

impl Runtime for UserRuntime {
  fn id(&self) -> &str {
    "user"
  }

  fn handle(
    &mut self,
    action: &str,
    payload: RuntimePayload,
    ctx: &mut RuntimeContext<'_>,
    dispatch: &RuntimeDispatcher,
    _log: &LogHandle,
  ) -> Result<Option<serde_json::Value>, CoreError> {
    let pam = ctx
      .scope
      .get::<Arc<PamHandle>>()
      .expect("PamHandle not in scope");

    let event_bus = ctx.scope.get::<EventBus>().cloned().unwrap_or_default();

    match action {
      "login" => {
        let session_id = payload.get::<u64>("session_id")?;
        let tty = payload.get::<String>("tty")?;
        let username = payload.get::<String>("username")?;
        let user = pam
          .store
          .lookup_by_name(&username)
          .ok_or(PamError::UserNotFound)?;

        let _ = dispatch.dispatch(
          "flow",
          "set_state",
          serde_json::json!({
            "name": "rind@user_session",
            "payload": {
              "session_id": session_id,
              "username": username,
              "tty": tty,
              "runtime_dir": runtime_dir(user.uid).to_string_lossy().to_string()
            }
          })
          .into(),
        );

        event_bus.emit(LoginEvent {
          action: LoginAction::Login,
          session_id: session_id,
          uid: user.uid,
        });

        self.create_runtime_dir(user)?;
      }
      "logout" => {
        let session_id = payload.get::<u64>("session_id")?;
        let username = payload.get::<String>("username")?;
        let user = pam
          .store
          .lookup_by_name(&username)
          .ok_or(PamError::UserNotFound)?;

        let _ = dispatch.dispatch(
          "flow",
          "remove_state",
          serde_json::json!({
            "name": "rind@user_session",
            "payload": {
              "session_id": session_id,
            }
          })
          .into(),
        );

        event_bus.emit(LoginEvent {
          action: LoginAction::Logout,
          session_id: session_id,
          uid: user.uid,
        });

        self.remove_runtime_dir(user, pam)?;
      }
      "create_sessions" => {
        let sm_shared = ctx
          .scope
          .get::<StateMachineShared>()
          .cloned()
          .ok_or_else(|| CoreError::InvalidState("state machine not found in scope".into()))?;

        let mut sm = sm_shared.write().map_err(CoreError::custom)?;
        if let Some(users) = sm.states.get_mut("rind@user_session") {
          for user in users {
            let username = user.payload.get_json_field_as::<String>("username").ok_or(
              CoreError::MissingField {
                path: "username".into(),
              },
            )?;
            let tty = user
              .payload
              .get_json_field_as::<String>("tty")
              .ok_or(CoreError::MissingField { path: "tty".into() })?;
            // let session_id = user.payload.get_json_field_as::<u64>("session_id").ok_or(
            //   CoreError::MissingField {
            //     path: "session_id".into(),
            //   },
            // )?;

            let session = pam.pam_open_session(&username, &tty)?;

            // ILLEGAL OPERATION:
            // - modify state silently
            // - modify primary key
            // - direct wrtie to sm with impermanence
            user
              .payload
              .set_json("session_id".into(), session.id.into());

            let user = pam
              .store
              .lookup_by_name(&username)
              .ok_or(PamError::UserNotFound)?;

            self.create_runtime_dir(user)?;
          }
        }
        drop(sm);
      }

      // IPC Control
      "ipc:run0" => {
        let pm = ctx
          .scope
          .get::<PermissionStore>()
          .cloned()
          .unwrap_or_default();

        let msg = payload_msg(payload)?;

        let Some(pid) = msg.from_pid else {
          return Err(CoreError::PermissionDenied);
        };
        let Some(uid) = msg.from_uid else {
          return Err(CoreError::PermissionDenied);
        };

        if !pm.user_has(msg.from_uid.unwrap(), PERM_RUN0)
          && !pm.users.user_in_group(
            pm.users.lookup_by_uid(msg.from_uid.unwrap()).unwrap(),
            "wheel",
          )
        {
          return Err(CoreError::PermissionDenied);
        }

        let state = self.run0_queue.entry(pid).or_insert(Run0State::default());

        if matches!(state, Run0State::Inactive) {
          *state = Run0State::RequireAuth;
          return Ok(Some(Message::from_type(MessageType::RequestInput).into()));
        }

        let payload = msg
          .parse_payload::<Run0AuthPayload>()
          .map_err(|x| CoreError::Custom(x))?;

        let pam = ctx
          .scope
          .get::<Arc<rind_core::user::PamHandle>>()
          .expect("PamHandle not in scope");

        let user = pam
          .store()
          .lookup_by_uid(uid)
          .ok_or(CoreError::Custom("user not found".into()))?;

        let password = payload.password;
        if let Err(e) = pam.pam_authenticate(&user.username, &password) {
          self.run0_queue.remove(&pid);
          return Err(CoreError::PamError(e));
        }

        return if matches!(state, Run0State::RequireAuth) {
          // let env = std::fs::read(format!("/proc/{}/environ", pid))
          //   .map(|bytes| {
          //     bytes
          //       .split(|b| *b == 0) // split on '\0'
          //       .filter_map(|entry| {
          //         let s = std::str::from_utf8(entry).ok()?;
          //         s.split_once('=')
          //           .map(|(k, v)| (k.to_string(), v.to_string()))
          //       })
          //       .collect::<HashMap<String, String>>()
          //   })
          //   .unwrap_or_default();

          // let cwd = std::fs::read_link(format!("/proc/{}/cwd", pid)).unwrap_or(PathBuf::from("/"));

          // use std::fs::File;

          // let Ok(stdin) = File::open(format!("/proc/{}/fd/0", pid)) else {
          //   return Message::nack(format!("failed to read stdin for parent process"));
          // };
          // let Ok(stdout) = File::open(format!("/proc/{}/fd/1", pid)) else {
          //   return Message::nack(format!("failed to read stdout for parent process"));
          // };
          // let Ok(stderr) = File::open(format!("/proc/{}/fd/2", pid)) else {
          //   return Message::nack(format!("failed to read stderr for parent process"));
          // };

          // let args = args.clone();
          self.run0_queue.remove(&pid);

          // std::thread::spawn(move || {
          //   let mut args = args.into_iter();
          //   let program = args.next().unwrap();

          //   let mut command = Command::new(program);

          //   command
          //     .args(args)
          //     .gid(0)
          //     .uid(0)
          //     .envs(env)
          //     .current_dir(cwd)
          //     .stdin(stdin)
          //     .stdout(stdout)
          //     .stderr(stderr);

          //   let _ = command.spawn();
          // });

          Ok(Some(Message::from_type(MessageType::Valid).into()))
        } else {
          Ok(Some(Message::from_type(MessageType::Unknown).into()))
        };
      }
      "ipc:login" => {
        let payload = payload_to::<LoginPayload>(payload)?;

        let pam = ctx
          .scope
          .get::<Arc<rind_core::user::PamHandle>>()
          .expect("PamHandle not in scope");

        let Some(_) = pam.store().lookup_by_name(&payload.username) else {
          return Err(CoreError::PermissionDenied);
        };

        let password = payload.password.as_deref().unwrap_or("");
        if let Err(e) = pam.pam_authenticate(&payload.username, password) {
          return Err(CoreError::PamError(e));
        }

        if let Err(e) = pam.pam_acct_mgmt(&payload.username) {
          return Err(CoreError::PamError(e));
        }

        let session = match pam.pam_open_session(&payload.username, &payload.tty) {
          Ok(s) => s,
          Err(e) => return Err(CoreError::PamError(e)),
        };

        let _ = dispatch.dispatch(
          "user",
          "login",
          serde_json::json!({
            "username": payload.username.clone(),
            "tty": payload.tty.clone(),
            "session_id": session.id,
          })
          .into(),
        );

        return Ok(Some(
          Message::ok(format!("logged in successfully as {}", payload.username)).into(),
        ));
      }
      "ipc:logout" => {
        let msg = payload.r#as::<Message>()?;
        let mut payload = payload_to::<LogoutPayload>(payload)?;

        if !payload.tty.starts_with("/dev/") {
          payload.tty = format!("/dev/{}", payload.tty);
        }

        let pam = ctx
          .scope
          .get::<Arc<rind_core::user::PamHandle>>()
          .expect("PamHandle not in scope");

        let Some(user) = pam.store().lookup_by_name(&payload.username) else {
          return Err(CoreError::PermissionDenied);
        };

        if msg.from_uid.is_none() || msg.from_uid.unwrap() != user.uid {
          return Err(CoreError::PermissionDenied);
        }

        let sessions = pam.sessions_for(&payload.username);

        let mut closed = false;
        let mut session_id = 0;
        for session in sessions {
          if session.tty == payload.tty {
            session_id = session.id;
            let _ = pam.pam_close_session(session.id);
            closed = true;
          }
        }

        if closed {
          let _ = dispatch.dispatch(
            "user",
            "logout",
            serde_json::json!({
              "session_id": session_id,
              "username": payload.username,
            })
            .into(),
          );

          return Ok(Some(
            Message::ok(format!("logged in successfully as {}", payload.username)).into(),
          ));
        } else {
          return Err(CoreError::InvalidState(format!(
            "no active session for {} on tty {}",
            payload.username, payload.tty
          )));
        }
      }
      _ => {}
    }

    // dispatch.dispatch("flow", "bootstrap", json!({}).into())?;
    // dispatch.dispatch("services", "evaluate_triggers", json!({}).into())?;

    Ok(None)
  }
}
