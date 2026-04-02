// Users "exist" but-
// - PAM sucsk
// - Need to expose more APIs

use std::sync::Arc;

use rind_core::prelude::*;

use crate::flow::StateMachineShared;

#[derive(Default)]
pub struct UserRuntime;

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
    match action {
      "login" => {
        let session_id = payload.get::<u64>("session_id")?;
        let tty = payload.get::<String>("tty")?;
        let username = payload.get::<String>("username")?;

        let _ = dispatch.dispatch(
          "flow",
          "set_state",
          serde_json::json!({
            "name": "rind@user_session",
            "payload": {
              "session_id": session_id,
              "username": username,
              "tty": tty
            }
          })
          .into(),
        );
      }
      "logout" => {
        let session_id = payload.get::<u64>("session_id")?;

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
      }
      "create_sessions" => {
        let sm_shared = ctx
          .scope
          .get::<StateMachineShared>()
          .cloned()
          .ok_or_else(|| CoreError::InvalidState("state machine not found in scope".into()))?;

        let pam = ctx
          .scope
          .get::<Arc<PamHandle>>()
          .expect("PamHandle not in scope");

        let sm = sm_shared.write().map_err(CoreError::custom)?;
        if let Some(users) = sm.states.get("rind@user_session") {
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

            let session = pam.pam_open_session(&username, &tty)?;

            let _ = dispatch.dispatch(
              "flow",
              "set_state",
              serde_json::json!({
                "name": "rind@user_session",
                "payload": {
                  "username": username,
                  "tty": tty,
                  "session_id": session.id,
                }
              })
              .into(),
            );
          }
        }
        drop(sm);
      }
      _ => {}
    }
    Ok(())
  }
}
