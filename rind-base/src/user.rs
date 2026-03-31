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
    _payload: RuntimePayload,
    ctx: &mut RuntimeContext<'_>,
    dispatch: &RuntimeDispatcher,
    _log: &LogHandle,
  ) -> Result<(), CoreError> {
    match action {
      "create_sessions" => {
        println!("Running Session");
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
        if let Some(users) = sm.states.get("rind@user_auto_login") {
          for user in users {
            println!("User {user:?}");
            let username = user.payload.get_json_field_as::<String>("username").ok_or(
              CoreError::MissingField {
                path: "username".into(),
              },
            )?;
            println!("Username {username}");
            let tty = user
              .payload
              .get_json_field_as::<String>("tty")
              .ok_or(CoreError::MissingField { path: "tty".into() })?;
            println!("TTY {tty}");

            let session = pam.pam_open_session(&username, &tty)?;

            let _ = dispatch.dispatch(
              "flow",
              "set_state",
              serde_json::json!({
                "name": "rind@_user_session",
                "payload": {
                  "username": username,
                  "tty": tty,
                  "session_id": session.id,
                }
              })
              .into(),
            );
            println!("Dispatched");
          }
        }
        drop(sm);
      }
      _ => {}
    }
    Ok(())
  }
}
