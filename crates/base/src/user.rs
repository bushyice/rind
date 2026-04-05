// Users "exist" but-
// - PAM sucsk
// - Need to expose more APIs

use std::{
  fs,
  os::unix::fs::{PermissionsExt, chown},
  path::PathBuf,
  sync::Arc,
};

use rind_core::prelude::*;

use crate::flow::StateMachineShared;

#[derive(Default)]
pub struct UserRuntime;

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
  ) -> Result<(), CoreError> {
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
      _ => {}
    }

    // dispatch.dispatch("flow", "bootstrap", json!({}).into())?;
    // dispatch.dispatch("services", "evaluate_triggers", json!({}).into())?;

    Ok(())
  }
}
