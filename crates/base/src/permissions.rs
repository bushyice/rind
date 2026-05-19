// Permissions have partially been impl'd BUT-
// - kinda lacks proper permission validation because each module/concept has to handle it separately

use rind_core::{
  error::{CoreError, CoreResult},
  prelude::{
    LogHandle, Model, NamedItem, PermissionId, PermissionStore, RuntimeContext, model,
    permission_path,
  },
  runtime::RuntimeDispatcher,
  types::{ToUstr, Ustr},
};
use rind_ipc::{
  Message, MessageType,
  payloads::PermissionPayload,
  ser::{IpcListComponent, IpcListPrinter, PermissionSerialized},
};

pub static PERM_SYSTEM_SERVICES: PermissionId = PermissionId(1000);
pub static PERM_LOGIN: PermissionId = PermissionId(1001);
pub static PERM_RUN0: PermissionId = PermissionId(1002);

#[model(
  meta_name = name,
  meta_fields(
    name, id
  )
)]
pub struct Permission {
  pub name: Ustr,
  pub id: u16,
  pub links: Option<Vec<u16>>,
  pub group: Option<Ustr>,
}

fn grant_ungrant_ipc(msg: Message, pm: &PermissionStore, grant: bool) -> CoreResult<Message> {
  let payload = msg
    .parse_payload::<PermissionPayload>()
    .map_err(CoreError::Custom)?;

  let Some(permid) = (if let Ok(permid) = payload.permission.parse::<u16>() {
    Some(PermissionId(permid))
  } else {
    pm.from_name(&payload.permission.to_ustr())
  }) else {
    return Err(CoreError::not_found("permission", &payload.permission));
  };

  if payload.group {
    let Some(group) = pm.users.group_by_name(&payload.subject) else {
      return Err(CoreError::not_found("group", &payload.subject));
    };
    if grant {
      pm.grant_group(group.gid, permid);
    } else {
      pm.ungrant_group(group.gid, permid);
    }
  } else {
    let Some(user) = pm.users.lookup_by_name(&payload.subject) else {
      return Err(CoreError::not_found("user", &payload.subject));
    };
    if grant {
      pm.grant_user(user.uid, permid);
    } else {
      pm.ungrant_user(user.uid, permid);
    }
  }

  pm.write_perms_with_overlay(&permission_path())?;

  Ok(Message::ok(format!(
    "Permission changed for {}",
    payload.subject
  )))
}

pub fn handle_ipc_grant_permission(
  msg: Message,
  ctx: &mut RuntimeContext<'_>,
  _dispatch: &RuntimeDispatcher,
  _log: &LogHandle,
) -> CoreResult<Message> {
  let pm = ctx
    .registry
    .singleton::<PermissionStore>(PermissionStore::KEY)
    .ok_or(CoreError::RuntimeStopped)?;

  grant_ungrant_ipc(msg, pm, true)
}

pub fn handle_ipc_revoke_permission(
  msg: Message,
  ctx: &mut RuntimeContext<'_>,
  _dispatch: &RuntimeDispatcher,
  _log: &LogHandle,
) -> CoreResult<Message> {
  let pm = ctx
    .registry
    .singleton::<PermissionStore>(PermissionStore::KEY)
    .ok_or(CoreError::RuntimeStopped)?;

  grant_ungrant_ipc(msg, pm, false)
}

pub fn handle_ipc_show_permission(
  msg: Message,
  ctx: &mut RuntimeContext<'_>,
  _dispatch: &RuntimeDispatcher,
  _log: &LogHandle,
) -> CoreResult<Message> {
  let pm = ctx
    .registry
    .singleton::<PermissionStore>(PermissionStore::KEY)
    .ok_or(CoreError::RuntimeStopped)?;
  let payload = msg.parse_payload::<PermissionPayload>().ok();

  let mut list = IpcListComponent::default().with_printer(IpcListPrinter {
    r#type: "table".to_string(),
    titles: vec!["Name".to_string(), "Id".to_string(), "Group".to_string()],
    keys: vec!["name".to_string(), "id".to_string(), "group".to_string()],
    colors: vec![
      "blue".to_string(),
      "yellow".to_string(),
      "green".to_string(),
    ],
  });

  for (id, name, group) in pm.all(
    payload.as_ref().and_then(|payload| {
      if payload.group {
        pm.users.group_by_name(&payload.subject).map(|x| x.gid)
      } else {
        pm.users.lookup_by_name(&payload.subject).map(|x| x.uid)
      }
    }),
    payload.map_or(false, |payload| payload.group),
  )? {
    list.add(PermissionSerialized { group, name, id });
  }

  Ok(Message::from_type(MessageType::Ok).with(flexbuffers::to_vec(&list).unwrap_or_default()))
}
