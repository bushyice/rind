use rind_core::reexports::serde_json;
use rind_flow::triggers::trigger_events;
use rind_ipc::payloads::SSPayload;
use std::collections::{HashMap, HashSet};
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

use nix::sys::socket::{
  AddressFamily, Backlog, SockFlag, SockType, SockaddrIn, SockaddrIn6, UnixAddr, bind, listen,
  setsockopt, socket, sockopt,
};
use rind_core::prelude::*;
use serde::{Deserialize, Serialize};

use crate::ServiceRuntime;
use crate::services::SocketActivation;
use rind_flow::{
  EmitTrigger, FacetGraph, FlowInstance, FlowItem, FlowRuntime, Trigger, condition_matches,
};
use rind_ipc::Message;
use rind_primitives::permissions::PERM_SYSTEM_SERVICES;
use rind_primitives::variables::VariableHeap;

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SocketType {
  Tcp,
  Udp,
  Uds,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum SocketServiceLifecycle {
  #[default]
  Managed,
  Owned,
}

#[model(meta_name = name, meta_fields(name, listen, r#type, owner, start_on, lifecycle, trigger, stop_on, managed_by, on_start, on_stop, on_data, permissions), derive_metadata(Debug, Clone))]
pub struct Socket {
  pub name: Ustr,
  pub listen: String,
  pub r#type: SocketType,
  pub owner: Option<Ustr>,
  #[serde(rename = "start-on")]
  pub start_on: Option<Vec<FlowItem>>,
  #[serde(rename = "stop-on")]
  pub stop_on: Option<Vec<FlowItem>>,
  #[serde(rename = "on-start")]
  pub on_start: Option<Vec<Trigger>>,
  #[serde(rename = "trigger")]
  pub trigger: Option<Vec<Trigger>>,
  #[serde(rename = "on-stop")]
  pub on_stop: Option<Vec<Trigger>>,
  #[serde(default)]
  pub lifecycle: SocketServiceLifecycle,

  #[serde(rename = "managed-by")]
  pub managed_by: Option<Vec<Ustr>>,
  pub permissions: Option<Vec<Ustr>>,

  pub fd: RawFd,
  pub active: bool,
}

pub struct SocketRuntime {
  instances: HashMap<RawFd, Ustr>,
  owner: HashMap<RawFd, Ustr>,
  paused: HashMap<Ustr, Vec<RawFd>>,
  trigger_index: HashMap<Ustr, std::collections::HashSet<Ustr>>,
}

impl Default for SocketRuntime {
  fn default() -> Self {
    Self {
      instances: HashMap::new(),
      owner: HashMap::new(),
      paused: HashMap::new(),
      trigger_index: HashMap::new(),
    }
  }
}

impl SocketRuntime {
  fn get_socket_path(&self, listen: &str, create: bool) -> CoreResult<PathBuf> {
    let path = PathBuf::from("/var/sock").join(listen);
    if let (Some(p), true) = (path.parent(), create) {
      std::fs::create_dir_all(p)?;
    }
    if path.exists() {
      let _ = std::fs::remove_file(&path);
    }
    Ok(path)
  }

  fn create_socket(&self, meta: &SocketMetadata) -> CoreResult<std::os::fd::OwnedFd> {
    let fd = match meta.r#type {
      SocketType::Tcp => {
        let addr: std::net::SocketAddr = meta.listen.parse()?;
        let family = match addr {
          std::net::SocketAddr::V4(_) => AddressFamily::Inet,
          std::net::SocketAddr::V6(_) => AddressFamily::Inet6,
        };

        let fd = socket(
          family,
          SockType::Stream,
          SockFlag::SOCK_NONBLOCK | SockFlag::SOCK_CLOEXEC,
          None,
        )?;
        setsockopt(&fd, sockopt::ReuseAddr, &true)?;

        match addr {
          std::net::SocketAddr::V4(a) => bind(fd.as_raw_fd(), &SockaddrIn::from(a))?,
          std::net::SocketAddr::V6(a) => bind(fd.as_raw_fd(), &SockaddrIn6::from(a))?,
        };

        listen(&fd, Backlog::new(128)?)?;
        fd
      }
      SocketType::Uds => {
        let path = self.get_socket_path(&meta.listen, true)?;

        let fd = socket(
          AddressFamily::Unix,
          SockType::Stream,
          SockFlag::SOCK_NONBLOCK | SockFlag::SOCK_CLOEXEC,
          None,
        )?;
        let sockaddr = UnixAddr::new(&path)?;
        bind(fd.as_raw_fd(), &sockaddr)?;
        listen(&fd, Backlog::new(128)?)?;

        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o666))?;
        fd
      }
      SocketType::Udp => {
        let addr: std::net::SocketAddr = meta.listen.parse()?;
        let family = match addr {
          std::net::SocketAddr::V4(_) => AddressFamily::Inet,
          std::net::SocketAddr::V6(_) => AddressFamily::Inet6,
        };
        let fd = socket(
          family,
          SockType::Datagram,
          SockFlag::SOCK_NONBLOCK | SockFlag::SOCK_CLOEXEC,
          None,
        )?;

        match addr {
          std::net::SocketAddr::V4(a) => bind(fd.as_raw_fd(), &SockaddrIn::from(a))?,
          std::net::SocketAddr::V6(a) => bind(fd.as_raw_fd(), &SockaddrIn6::from(a))?,
        };
        fd
      }
    };
    Ok(fd)
  }

  fn owner_fds(&self, owner: &Ustr) -> Vec<(RawFd, Ustr)> {
    let mut fds: Vec<(RawFd, Ustr)> = self
      .owner
      .iter()
      .filter_map(|(fd, entry)| {
        if entry == owner {
          Some((
            *fd,
            self.instances.get(fd).cloned().unwrap_or(Ustr::from("")),
          ))
        } else {
          None
        }
      })
      .collect();
    fds.sort_by_key(|(fd, _)| *fd);
    fds
  }

  fn rebuild_trigger_index(&mut self, metadata: &MetadataRegistry) {
    self.trigger_index.clear();
    let sockets = metadata.items::<Socket>("*").unwrap_or_default();

    for (group, meta) in sockets {
      let key = Ustr::from(format!("{}:{}", group, meta.name));
      let mut interests = std::collections::HashSet::new();

      if let Some(start_on) = &meta.start_on {
        for item in start_on {
          interests.insert(item.name());
        }
      }
      if let Some(stop_on) = &meta.stop_on {
        for item in stop_on {
          interests.insert(item.name());
        }
      }

      for interest in interests {
        self
          .trigger_index
          .entry(interest.clone())
          .or_default()
          .insert(key.clone());
      }
    }
  }

  fn start_socket(
    &mut self,
    name: Ustr,
    resources: &mut Resources,
    registry: &mut InstanceRegistry,
    sr: &mut SocketRegistry,
  ) -> CoreResult<Void> {
    let sock = registry.instantiate_one("*", name.clone(), |metadata| {
      let owned_fd = self
        .create_socket(&metadata)
        .map_err(|e| CoreError::Custom(format!("failed to create socket {name}: {e}")))?;
      let fd = owned_fd.as_raw_fd();

      resources.own(fd, owned_fd);
      resources.action(fd, ("sockets", "drain_incoming"));

      Ok(Socket {
        metadata,
        fd,
        active: true,
      })
    })?;

    resources.resume(sock.fd);
    self.instances.insert(sock.fd, name);

    if let Some(owner) = sock.metadata.owner.as_ref() {
      self.owner.insert(sock.fd, owner.clone());
      sr.owners
        .entry(owner.clone())
        .or_default()
        .push((sock.metadata.name.clone(), sock.fd));
    }

    Ok(Void)
  }

  fn stop_socket(
    &mut self,
    name: Ustr,
    resources: &mut Resources,
    registry: &mut InstanceRegistry,
    sr: &mut SocketRegistry,
  ) -> CoreResult<Void> {
    let socket = registry.uninstantiate_one::<Socket>("*", name.clone())?;
    let fd = socket.fd;

    resources.terminate(fd);
    self.instances.remove(&fd);
    self.owner.remove(&fd);

    if let Some(owner) = &socket.metadata.owner {
      if let Some(paused) = self.paused.get_mut(owner) {
        paused.retain(|&f| f != fd);
      }
      sr.owners.remove(owner);
    }

    if socket.metadata.r#type == SocketType::Uds {
      self.get_socket_path(&socket.metadata.listen, false)?;
    }

    Ok(Void)
  }

  fn clear_socket(&mut self, socket: &Socket) {
    let fd = socket.fd;

    match socket.metadata.r#type {
      SocketType::Tcp | SocketType::Uds => {
        use nix::sys::socket::accept;
        use nix::unistd::close;
        loop {
          match accept(fd) {
            Ok(client_fd) => {
              let _ = close(client_fd);
            }
            Err(_) => break,
          }
        }
      }
      SocketType::Udp => {
        use nix::sys::socket::{MsgFlags, recv};
        let mut buf = [0u8; 2048];
        loop {
          match recv(fd, &mut buf, MsgFlags::MSG_DONTWAIT) {
            Ok(_) => {}
            Err(_) => break,
          }
        }
      }
    }
  }
}

#[derive(Default)]
pub struct SocketRegistry {
  pub owners: HashMap<Ustr, Vec<(Ustr, RawFd)>>,
}

impl SocketRegistry {
  pub const KEY: &str = "runtime:socket_registry";
}

#[runtime("sockets")]
impl SocketRuntime {
  fn bootstrap(&mut self) {
    ctx
      .registry
      .singleton_or_insert_with(SocketRegistry::KEY, || SocketRegistry::default());
    self.rebuild_trigger_index(ctx.registry.metadata);
  }

  fn evaluate_triggers(&mut self, #[default] trigger: EmitTrigger, #[optional] scope: Ustr) {
    let scope_val = scope.unwrap_or("static".to_ustr());
    if self.trigger_index.is_empty() {
      self.rebuild_trigger_index(ctx.registry.metadata);
    }

    ctx
      .registry
      .singleton_handle::<(&mut FacetGraph, &mut SocketRegistry), _>(
        (FacetGraph::KEY.into(), SocketRegistry::KEY.into()),
        |registry, (sm, sr)| {
          let target_keys = if let Some(event_name) = trigger.name.as_ref() {
            let mut out = HashSet::new();
            let direct = event_name.clone();
            let static_alias = if event_name.as_str().ends_with("@static") {
              Ustr::from(event_name.as_str().trim_end_matches("@static"))
            } else {
              Ustr::from(format!("{}@static", event_name))
            };
            for key in [direct, static_alias] {
              if let Some(found) = self.trigger_index.get(&key) {
                out.extend(found.iter().cloned());
              }
            }
            out
          } else {
            registry
              .metadata
              .items::<Socket>(scope_val.clone())
              .unwrap_or_default()
              .into_iter()
              .map(|(group, meta)| Ustr::from(format!("{}:{}@{}", group, meta.name, scope_val)))
              .collect::<HashSet<Ustr>>()
          };

          let emit_event = match (
            trigger.name.as_ref(),
            trigger.flow_type,
            trigger.payload.as_ref(),
          ) {
            (Some(name), Some(flow_type), Some(payload)) => Some(FlowInstance {
              name: name.clone().into(),
              payload: payload.clone(),
              r#type: flow_type,
            }),
            _ => None,
          };

          for socket_name in target_keys {
            let Some(meta) = registry.metadata.find::<Socket>("*", socket_name.as_str()) else {
              continue;
            };

            let is_active = if let Ok(sock) = registry.as_one::<Socket>("*", socket_name.as_str()) {
              sock.active
            } else {
              false
            };

            let should_start = meta
              .start_on
              .as_ref()
              .map(|conds| {
                conds
                  .iter()
                  .any(|cond| condition_matches(sm, cond, emit_event.as_ref(), None))
              })
              .unwrap_or(false);

            let should_stop = meta
              .stop_on
              .as_ref()
              .map(|conds| {
                conds
                  .iter()
                  .any(|cond| condition_matches(sm, cond, emit_event.as_ref(), None))
              })
              .unwrap_or(false);

            if should_start && !is_active {
              let _ = self.start_socket(socket_name.clone(), ctx.resources, registry, sr);
            } else if should_stop && is_active {
              let _ = self.stop_socket(socket_name.clone(), ctx.resources, registry, sr);
            }
          }
          Ok(Void)
        },
      )?;
  }

  fn setup_all(&mut self) {
    ctx
      .registry
      .singleton_handle::<(&mut FacetGraph, &mut VariableHeap, &mut SocketRegistry), _>(
        (
          FacetGraph::KEY.into(),
          VariableHeap::KEY.into(),
          SocketRegistry::KEY.into(),
        ),
        |registry, (sm, _vh, sr)| {
          let Some(active) = sm.facets.get("rind:active") else {
            return Ok(Void);
          };

          for branch in active {
            match self.start_socket(
              branch.payload.to_string_payload().to_ustr(),
              ctx.resources,
              registry,
              sr,
            ) {
              Ok(_) => {}
              Err(CoreError::MetadataNotFound(_)) => {}
              Err(e) => return Err(e),
            };
          }

          Ok(Void)
        },
      )?;
  }

  fn stop_for_scope(&mut self, scope: Ustr) {
    ctx
      .registry
      .singleton_handle::<(&mut SocketRegistry, &mut VariableHeap), _>(
        (SocketRegistry::KEY.into(), VariableHeap::KEY.into()),
        |registry, (sr, _vh)| {
          for (group, soc) in registry
            .metadata
            .items::<Socket>(scope.clone())
            .unwrap_or_default()
          {
            let full_name = rslvns!(u group, soc.name);
            self.stop_socket(full_name, ctx.resources, registry, sr)?;
          }
          Ok(Void)
        },
      )?;
  }

  fn stop(&mut self, name: Ustr) {
    ctx
      .registry
      .singleton_handle::<(&mut SocketRegistry, &mut VariableHeap), _>(
        (SocketRegistry::KEY.into(), VariableHeap::KEY.into()),
        |registry, (sr, _)| self.stop_socket(name, ctx.resources, registry, sr),
      )?;
  }

  fn start(&mut self, name: Ustr) {
    ctx
      .registry
      .singleton_handle::<(&mut SocketRegistry, &mut VariableHeap), _>(
        (SocketRegistry::KEY.into(), VariableHeap::KEY.into()),
        |registry, (sr, _)| self.start_socket(name, ctx.resources, registry, sr),
      )?;
  }

  #[action(rename = "reset_fds")]
  fn reset_fds_action(&mut self, name: Ustr) {
    if let Some(fds) = self.paused.remove(&name) {
      for fd in fds {
        ctx.resources.resume(fd);
      }
      if let Some(n) = &ctx.notifier {
        n.notify()?;
      }
    }
  }

  #[action(rename = "resume_fds")]
  fn resume_fds_action(&mut self, name: Ustr) {
    self.__runtime_reset_fds_action(
      RuntimePayload::default().insert("name", name),
      ctx,
      dispatch,
      log,
    )?;
  }

  fn clear_for(&mut self, name: Ustr) {
    let Some(sockets) = ctx
      .registry
      .singleton::<SocketRegistry>(SocketRegistry::KEY)
    else {
      return Ok(None);
    };

    let Some(sockets) = sockets.owners.get(&name) else {
      return Ok(None);
    };

    for (_, fd) in sockets {
      let sock = self.instances.get(&fd).ok_or(CoreError::InvalidState(
        "Socket for fd was not found".into(),
      ))?;
      let Ok(socket) = ctx.registry.as_one::<Socket>("*", sock.clone()) else {
        continue;
      };

      self.clear_socket(socket);
    }
  }

  fn clear(&mut self, name: Ustr) {
    let socket = ctx.registry.as_one::<Socket>("*", name.clone())?;
    self.clear_socket(socket);
  }

  fn drain_incoming(&mut self, fd: i32) {
    let fd_raw = fd as RawFd;
    let name = self.instances.get(&fd_raw).ok_or(CoreError::InvalidState(
      "Socket for fd was not found".into(),
    ))?;
    let socket = ctx.registry.as_one::<Socket>("*", name.clone())?;
    ctx.resources.pause(fd_raw);

    let pm = ctx
      .registry
      .singleton::<PermissionStore>(PermissionStore::KEY);

    log.log(
      LogLevel::Trace,
      "sockets",
      "socket accessed",
      [("name".to_string(), name.to_string())].into(),
    );

    if let Some(ref permissions) = socket.metadata.permissions
      && let Some(pm) = pm
    {
      let Ok(cred) = get_peer_cred(fd_raw) else {
        log.log(
          LogLevel::Debug,
          "sockets",
          "permission denied: failed to get peer credentials",
          [("name".to_string(), name.to_string())].into(),
        );

        self.clear_socket(socket);
        return Ok(None);
      };

      if !permissions
        .iter()
        .any(|x| pm.from_name(x).map_or(false, |x| pm.user_has(cred.uid, x)))
      {
        log.log(
          LogLevel::Debug,
          "sockets",
          "permission denied",
          [
            ("name".to_string(), name.to_string()),
            ("uid".to_string(), cred.uid.to_string()),
          ]
          .into(),
        );
        self.clear_socket(socket);
        return Ok(None);
      }
    }

    if let Some(owner) = socket.metadata.owner.clone() {
      self.paused.entry(owner.clone()).or_default().push(fd_raw);

      if let SocketServiceLifecycle::Owned = &socket.metadata.lifecycle {
        let owner_fds = self.owner_fds(&owner);
        let socket_fds: Vec<i32> = owner_fds.iter().map(|(fd, _)| *fd).collect();
        let socket_fd_names: Vec<Ustr> = owner_fds.into_iter().map(|(_, name)| name).collect();

        ServiceRuntime::actions
          .start(owner)
          .socket_fds(socket_fds)
          .socket_fd_names(socket_fd_names)
          .dispatch(dispatch)?;
      }
    }

    if let Some(triggers) = &socket.metadata.trigger {
      let triggers = triggers.clone();
      ctx.registry.singleton_handle::<(&mut FacetGraph,), _>(
        (FacetGraph::KEY.into(),),
        |_, (sm,)| {
          trigger_events(triggers, Some(sm), dispatch);
          Ok(Void)
        },
      )?;
    }
  }
}

fn ipc_owner_has_access(_owner: &Ustr, _user: &UserRecord) -> bool {
  false
}

pub fn handle_ipc_start_socket(
  msg: Message,
  ctx: &mut RuntimeContext<'_>,
  dispatch: &RuntimeDispatcher,
  _log: &LogHandle,
) -> Result<Message, CoreError> {
  let pm = ctx
    .registry
    .singleton::<PermissionStore>(PermissionStore::KEY)
    .cloned()
    .unwrap_or_default();

  let payload = msg
    .parse_payload::<SSPayload>()
    .map_err(CoreError::Custom)?;

  let Some(uid) = msg.from_uid else {
    return Err(CoreError::PermissionDenied);
  };

  let sock = ctx.registry.metadata.find::<Socket>("*", &payload.name);
  let caller = pm.users.lookup_by_uid(uid);
  let can_manage = if uid == 0 || pm.user_has(uid, PERM_SYSTEM_SERVICES) {
    true
  } else if let (Some(user), Some(sock)) = (caller, sock.as_ref()) {
    if let Some(ref perms) = sock.managed_by {
      perms
        .iter()
        .any(|x| pm.from_name(x).map_or(false, |x| pm.user_has(uid, x)))
    } else {
      sock
        .owner
        .as_ref()
        .map_or(false, |owner| ipc_owner_has_access(owner, user))
    }
  } else {
    false
  };

  if sock.is_none() {
    return Err(CoreError::not_found("socket", &payload.name));
  }

  if !can_manage {
    return Err(CoreError::PermissionDenied);
  }

  SocketRuntime::actions
    .start(payload.name.to_ustr())
    .dispatch(dispatch)?;

  if payload.persist {
    FlowRuntime::actions
      .set_facet("rind:active".into())
      .payload(serde_json::Value::String(payload.name.clone()))
      .dispatch(dispatch)?;
  }

  Ok(Message::ok(format!("started socket {}", payload.name)))
}

pub fn handle_ipc_stop_socket(
  msg: Message,
  ctx: &mut RuntimeContext<'_>,
  dispatch: &RuntimeDispatcher,
  _log: &LogHandle,
) -> Result<Message, CoreError> {
  let pm = ctx
    .registry
    .singleton::<PermissionStore>(PermissionStore::KEY)
    .cloned()
    .unwrap_or_default();

  let payload = msg
    .parse_payload::<SSPayload>()
    .map_err(CoreError::Custom)?;

  let Some(uid) = msg.from_uid else {
    return Err(CoreError::PermissionDenied);
  };

  let sock = ctx.registry.metadata.find::<Socket>("*", &payload.name);
  let caller = pm.users.lookup_by_uid(uid);
  let can_manage = if uid == 0 || pm.user_has(uid, PERM_SYSTEM_SERVICES) {
    true
  } else if let (Some(user), Some(sock)) = (caller, sock.as_ref()) {
    if let Some(ref perms) = sock.managed_by {
      perms
        .iter()
        .any(|x| pm.from_name(x).map_or(false, |x| pm.user_has(uid, x)))
    } else {
      sock
        .owner
        .as_ref()
        .map_or(false, |owner| ipc_owner_has_access(owner, user))
    }
  } else {
    false
  };

  if !can_manage {
    return Err(CoreError::PermissionDenied);
  }

  SocketRuntime::actions
    .stop(payload.name.to_ustr())
    .dispatch(dispatch)?;

  if payload.persist {
    FlowRuntime::actions
      .remove_facet("rind:active".into())
      .payload(serde_json::Value::String(payload.name.clone()))
      .dispatch(dispatch)?;
  }

  Ok(Message::ok(format!("stopped socket {}", payload.name)))
}

pub fn get_all_sockets(
  registry: &rind_core::registry::InstanceRegistry<'_>,
) -> HashMap<Ustr, SocketActivation> {
  let Some(sockets) = registry.singleton::<SocketRegistry>(SocketRegistry::KEY) else {
    return Default::default();
  };

  sockets
    .owners
    .clone()
    .into_iter()
    .map(|(name, vec)| (name, SocketActivation::from(vec)))
    .collect()
}
