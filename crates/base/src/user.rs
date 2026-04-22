// Users "exist" but-
// - PAM sucsk
// - Need to expose more APIs

use std::{
  collections::HashMap,
  fs,
  os::unix::fs::{PermissionsExt, chown},
  path::PathBuf,
  sync::{Arc, Mutex},
};

use rind_core::prelude::*;
use rind_ipc::{
  Message, MessageType,
  payloads::{LoginPayload, LogoutPayload, Run0AuthPayload},
};
use serde_json::json;

use crate::{
  flow::{FlowRuntimePayload, StateMachine},
  permissions::PERM_RUN0,
};

pub type Run0QueueState = Arc<Mutex<HashMap<i32, bool>>>;

#[derive(Default)]
pub struct UserRuntime;

fn runtime_dir(uid: u32) -> PathBuf {
  PathBuf::from(format!("/run/user/{}", uid))
}

fn get_run0_queue(ctx: &RuntimeContext<'_>) -> Result<Run0QueueState, CoreError> {
  ctx
    .scope
    .get::<Run0QueueState>()
    .cloned()
    .ok_or_else(|| CoreError::InvalidState("run0 queue state not found in scope".into()))
}

pub fn handle_ipc_run0(
  msg: Message,
  ctx: &mut RuntimeContext<'_>,
  _dispatch: &RuntimeDispatcher,
  _log: &LogHandle,
) -> Result<Message, CoreError> {
  let pm = ctx
    .registry
    .singleton::<PermissionStore>(PermissionStore::KEY)
    .cloned()
    .unwrap_or_default();

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

  let queue = get_run0_queue(ctx)?;
  {
    let mut queue_guard = queue.lock().map_err(CoreError::custom)?;
    let needs_auth = queue_guard.entry(pid).or_insert(false);

    if !*needs_auth {
      *needs_auth = true;
      return Ok(Message::from_type(MessageType::RequestInput));
    }
  }

  let payload = msg
    .parse_payload::<Run0AuthPayload>()
    .map_err(|x| CoreError::Custom(x))?;

  let pam = ctx
    .registry
    .singleton::<Arc<PamHandle>>(PamHandle::KEY)
    .cloned()
    .ok_or_else(|| CoreError::InvalidState("pam handle not found".into()))?;

  let user = pam
    .store()
    .lookup_by_uid(uid)
    .ok_or(CoreError::Custom("user not found".into()))?;

  let password = payload.password;
  if let Err(e) = pam.pam_authenticate(&user.username, &password) {
    let mut queue_guard = queue.lock().map_err(CoreError::custom)?;
    queue_guard.remove(&pid);
    drop(queue_guard);
    return Err(CoreError::PamError(e));
  }

  let mut queue_guard = queue.lock().map_err(CoreError::custom)?;
  if queue_guard.remove(&pid).is_some() {
    return Ok(Message::from_type(MessageType::Valid));
  }

  Ok(Message::from_type(MessageType::Unknown))
}

pub fn handle_ipc_login(
  msg: Message,
  ctx: &mut RuntimeContext<'_>,
  dispatch: &RuntimeDispatcher,
  _log: &LogHandle,
) -> Result<Message, CoreError> {
  let payload = msg
    .parse_payload::<LoginPayload>()
    .map_err(|x| CoreError::Custom(x))?;

  let pam = ctx
    .registry
    .singleton::<Arc<PamHandle>>(PamHandle::KEY)
    .cloned()
    .ok_or_else(|| CoreError::InvalidState("pam handle not found".into()))?;

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
    rpayload!({
      "username": payload.username.to_ustr(),
      "tty": payload.tty.to_ustr(),
      "session_id": session.id,
    })
    .into(),
  );

  Ok(Message::ok(format!(
    "logged in successfully as {}",
    payload.username
  )))
}

pub fn handle_ipc_logout(
  msg: Message,
  ctx: &mut RuntimeContext<'_>,
  dispatch: &RuntimeDispatcher,
  _log: &LogHandle,
) -> Result<Message, CoreError> {
  let mut payload = msg
    .parse_payload::<LogoutPayload>()
    .map_err(|x| CoreError::Custom(x))?;

  if !payload.tty.starts_with("/dev/") {
    payload.tty = format!("/dev/{}", payload.tty);
  }

  let pam = ctx
    .registry
    .singleton::<Arc<PamHandle>>(PamHandle::KEY)
    .cloned()
    .ok_or_else(|| CoreError::InvalidState("pam handle not found".into()))?;

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
      rpayload!({
        "session_id": session_id,
        "username": payload.username.to_ustr(),
      })
      .into(),
    );

    return Ok(Message::ok(format!(
      "logged in successfully as {}",
      payload.username
    )));
  }

  Err(CoreError::InvalidState(format!(
    "no active session for {} on tty {}",
    payload.username, payload.tty
  )))
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
    mut payload: RuntimePayload,
    ctx: &mut RuntimeContext<'_>,
    dispatch: &RuntimeDispatcher,
    _log: &LogHandle,
  ) -> Result<Option<RuntimePayload>, CoreError> {
    let pam = ctx
      .registry
      .singleton::<Arc<PamHandle>>(PamHandle::KEY)
      .cloned()
      .ok_or_else(|| CoreError::InvalidState("pam handle not found".into()))?;

    match action {
      "login" => {
        let session_id = payload.get::<u64>("session_id")?;
        let tty = payload.get::<Ustr>("tty")?;
        let username = payload.get::<Ustr>("username")?;
        let user = pam
          .store
          .lookup_by_name(&username)
          .ok_or(PamError::UserNotFound)?;

        let _ = dispatch.dispatch(
          "flow",
          "set_state",
          FlowRuntimePayload::new("rind@user_session")
            .payload(json!({
              "session_id": session_id,
              "username": username.as_str(),
              "tty": tty.as_str(),
              "runtime_dir": runtime_dir(user.uid).to_string_lossy().to_string()
            }))
            .into(),
        );

        ctx.event_bus.emit(LoginEvent {
          action: LoginAction::Login,
          session_id: session_id,
          uid: user.uid,
        });

        self.create_runtime_dir(user)?;
      }
      "logout" => {
        let session_id = payload.get::<u64>("session_id")?;
        let username = payload.get::<Ustr>("username")?;
        let user = pam
          .store
          .lookup_by_name(&username)
          .ok_or(PamError::UserNotFound)?;

        let _ = dispatch.dispatch(
          "flow",
          "remove_state",
          FlowRuntimePayload::new("rind@user_session")
            .payload(json!({
              "session_id": session_id,
            }))
            .into(),
        );

        ctx.event_bus.emit(LoginEvent {
          action: LoginAction::Logout,
          session_id: session_id,
          uid: user.uid,
        });

        self.remove_runtime_dir(user, &pam)?;
      }
      "create_sessions" => {
        let sm = ctx
          .registry
          .singleton_mut::<StateMachine>(StateMachine::KEY)
          .ok_or_else(|| CoreError::InvalidState("state machine store not found".into()))?;
        let key = Ustr::from("rind@user_session");
        if let Some(users) = sm.states.get_mut(&key) {
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
      }
      _ => {}
    }

    // dispatch.dispatch("flow", "bootstrap", json!({}).into())?;
    // dispatch.dispatch("services", "evaluate_triggers", json!({}).into())?;

    Ok(None)
  }
}
