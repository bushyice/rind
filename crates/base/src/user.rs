// Users "exist" but-
// - PAM sucsk
// - Need to expose more APIs

use std::{
  collections::{HashMap, HashSet},
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
  flow::{FacetGraph, FlowRuntimePayload},
  permissions::PERM_RUN0,
  scopes::ScopeStore,
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

  let mut tty = payload.tty.clone();
  if !tty.starts_with("/dev/") {
    tty = format!("/dev/{}", tty);
  }

  let session = match pam.pam_open_session(&payload.username, &tty) {
    Ok(s) => s,
    Err(e) => return Err(CoreError::PamError(e)),
  };

  let _ = dispatch.dispatch(
    "user",
    "login",
    rpayload!({
      "username": payload.username.to_ustr(),
      "tty": tty.to_ustr(),
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
  let payload = msg
    .parse_payload::<LogoutPayload>()
    .map_err(|x| CoreError::Custom(x))?;

  // if !payload.tty.starts_with("/dev/") {
  //   payload.tty = format!("/dev/{}", payload.tty);
  // }

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

  pam.pam_close_session(payload.session_id)?;

  let _ = dispatch.dispatch(
    "user",
    "logout",
    rpayload!({
      "session_id": payload.session_id,
      "username": payload.username.to_ustr(),
      "tty": payload.tty.map(|x| x.to_ustr()),
    })
    .into(),
  );

  return Ok(Message::ok(format!(
    "logged out successfully as {}",
    payload.username
  )));
}

impl UserRuntime {
  fn user_scope_name(&self, username: &str) -> String {
    format!("dyn-user-{username}")
  }

  fn user_units_dir(&self, user: &UserRecord) -> PathBuf {
    PathBuf::from(&user.home).join(".local/share/rind/units")
  }

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
          "set_facet",
          FlowRuntimePayload::new("rind:user_session")
            .payload(json!({
              "session_id": session_id,
              "username": username.as_str(),
              "tty": tty.trim_start_matches("/dev/"),
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

        let scope_name = self.user_scope_name(username.as_str());
        let units_dir = self.user_units_dir(user);
        if units_dir.exists() && !ScopeStore::has_global(&scope_name) {
          let mut attrs = HashMap::new();
          attrs.insert(Ustr::from("user"), username.to_string());
          attrs.insert(
            Ustr::from("units_dir"),
            units_dir.to_string_lossy().to_string(),
          );
          ScopeStore::desired_scope_upsert(scope_name.clone(), attrs.clone(), None);
          ScopeStore::upsert_global(scope_name.clone(), attrs.clone(), None);
          if let Some(store) = ctx.registry.singleton_mut::<ScopeStore>(ScopeStore::KEY) {
            store.upsert(scope_name.clone(), attrs, None);
          }
          ctx.lifecycle.request(LifecycleAction::ReloadUnits);
        }

        if let Some(ref notifier) = ctx.notifier {
          notifier.notify()?;
        }
      }
      "logout" => {
        let session_id = payload.get::<u64>("session_id")?;
        let username = payload.get::<Ustr>("username")?;
        let tty = payload.get::<Ustr>("tty").ok();
        let user = pam
          .store
          .lookup_by_name(&username)
          .ok_or(PamError::UserNotFound)?;

        let mut filter = serde_json::Map::new();
        filter.insert("session_id".into(), session_id.into());
        if let Some(tty) = tty {
          filter.insert("tty".into(), tty.as_str().into());
        }

        let _ = dispatch.dispatch(
          "flow",
          "remove_facet",
          FlowRuntimePayload::new("rind:user_session")
            .payload(serde_json::Value::Object(filter))
            .into(),
        );

        ctx.event_bus.emit(LoginEvent {
          action: LoginAction::Logout,
          session_id: session_id,
          uid: user.uid,
        });

        self.remove_runtime_dir(user, &pam)?;

        if !pam.has_active_session(username.as_str()) {
          let scope_name = self.user_scope_name(username.as_str());
          ScopeStore::desired_scope_remove(scope_name.as_str());
          let _ = ScopeStore::remove_scope_global(scope_name.as_str());
          if let Some(store) = ctx.registry.singleton_mut::<ScopeStore>(ScopeStore::KEY) {
            let _ = store.remove_scope(scope_name.as_str());
          }
          if let Some(sm) = ctx.registry.singleton_mut::<FacetGraph>(FacetGraph::KEY) {
            let _ = sm.drop_scope(scope_name.as_str());
          }
          ctx.lifecycle.request(LifecycleAction::ReloadUnits);
        }

        if let Some(ref notifier) = ctx.notifier {
          notifier.notify()?;
        }
      }
      "create_sessions" => {
        let mut pending_scopes: Vec<(String, HashMap<Ustr, String>)> = Vec::new();
        let key = Ustr::from("rind:user_session");
        let mut seen_users: HashSet<String> = HashSet::new();
        let mut queued_reload = false;
        {
          let sm = ctx
            .registry
            .singleton_mut::<FacetGraph>(FacetGraph::KEY)
            .ok_or_else(|| CoreError::InvalidState("state machine store not found".into()))?;
          if let Some(users) = sm.facets.get_mut(&key) {
            for user in users.iter_mut() {
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
              user
                .payload
                .set_json("session_id".into(), session.id.into());

              let user_record = pam
                .store
                .lookup_by_name(&username)
                .ok_or(PamError::UserNotFound)?;

              self.create_runtime_dir(user_record)?;

              if seen_users.insert(username.clone()) {
                let scope_name = self.user_scope_name(&username);
                let units_dir = self.user_units_dir(user_record);
                if units_dir.exists() {
                  let mut attrs = HashMap::new();
                  attrs.insert(Ustr::from("user"), username.clone());
                  attrs.insert(
                    Ustr::from("units_dir"),
                    units_dir.to_string_lossy().to_string(),
                  );
                  pending_scopes.push((scope_name, attrs));
                  queued_reload = true;
                }
              }
            }
            sm.save_all_scopes()?;
          }
        }
        for (scope_name, attrs) in pending_scopes {
          ScopeStore::desired_scope_upsert(scope_name.clone(), attrs.clone(), None);
          ScopeStore::upsert_global(scope_name.clone(), attrs.clone(), None);
          if let Some(store) = ctx.registry.singleton_mut::<ScopeStore>(ScopeStore::KEY) {
            store.upsert(scope_name, attrs, None);
          }
        }
        if queued_reload {
          ctx.lifecycle.request(LifecycleAction::ReloadUnits);
        }
      }
      _ => {}
    }

    // dispatch.dispatch("flow", "bootstrap", json!({}).into())?;
    // dispatch.dispatch("services", "evaluate_triggers", json!({}).into())?;

    Ok(None)
  }
}
