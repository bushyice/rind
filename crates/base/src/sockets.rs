use rind_ipc::payloads::SSPayload;
use std::collections::HashMap;
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

use nix::sys::socket::{
  AddressFamily, Backlog, SockFlag, SockType, SockaddrIn, SockaddrIn6, UnixAddr, bind, listen,
  setsockopt, socket, sockopt,
};
use rind_core::prelude::*;
use serde::{Deserialize, Serialize};

use crate::flow::{FlowItem, StateMachine, Trigger};
use crate::permissions::PERM_SYSTEM_SERVICES;
use crate::prelude::{SocketActivation, VariableHeap, trigger_events};
use rind_ipc::Message;

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

#[model(meta_name = name, meta_fields(name, listen, r#type, owner, start_on, lifecycle, trigger, stop_on, on_start, on_stop, on_data), derive_metadata(Debug, Clone))]
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

  pub fd: RawFd,
  pub active: bool,
}

pub struct SocketRuntime {
  instances: HashMap<RawFd, Ustr>,
  owner: HashMap<RawFd, Ustr>,
  paused: HashMap<Ustr, Vec<RawFd>>,
  trigger_index: HashMap<Ustr, std::collections::HashSet<Ustr>>,
  event_rx: Option<rind_core::events::Subscription<rind_core::prelude::FlowEvent>>,
}

impl Default for SocketRuntime {
  fn default() -> Self {
    Self {
      instances: HashMap::new(),
      owner: HashMap::new(),
      paused: HashMap::new(),
      trigger_index: HashMap::new(),
      event_rx: None,
    }
  }
}

impl SocketRuntime {
  fn get_socket_path(&self, listen: &str, create: bool) -> anyhow::Result<PathBuf> {
    let path = PathBuf::from("/var/sock").join(listen);
    if let (Some(p), true) = (path.parent(), create) {
      std::fs::create_dir_all(p)?;
    }
    if path.exists() {
      let _ = std::fs::remove_file(&path);
    }
    Ok(path)
  }

  fn create_socket(&self, meta: &SocketMetadata) -> anyhow::Result<std::os::fd::OwnedFd> {
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
    let sockets = metadata.items::<Socket>("units").unwrap_or_default();

    for (group, meta) in sockets {
      let key = Ustr::from(format!("{}@{}", group, meta.name));
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
  ) -> CoreResult<()> {
    let sock = registry.instantiate_one("units", name.clone(), |metadata| {
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

    Ok(())
  }

  fn stop_socket(
    &mut self,
    name: Ustr,
    resources: &mut Resources,
    registry: &mut InstanceRegistry,
    sr: &mut SocketRegistry,
  ) -> CoreResult<()> {
    let socket = registry.uninstantiate_one::<Socket>("units", name.clone())?;
    let fd = socket.fd;

    resources.terminate(fd);
    self.instances.remove(&fd);
    self.owner.remove(&fd);

    if socket.metadata.r#type == SocketType::Uds {
      self.get_socket_path(&socket.metadata.listen, false)?;
    }

    if let Some(owner) = &socket.metadata.owner {
      if let Some(paused) = self.paused.get_mut(owner) {
        paused.retain(|&f| f != fd);
      }
      sr.owners.remove(owner);
    }

    Ok(())
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
  pub const KEY: &str = "runtime@socket_registry";
}

impl Runtime for SocketRuntime {
  fn id(&self) -> &str {
    "sockets"
  }

  fn handle(
    &mut self,
    action: &str,
    mut payload: RuntimePayload,
    ctx: &mut RuntimeContext<'_>,
    dispatch: &RuntimeDispatcher,
    log: &LogHandle,
  ) -> Result<Option<RuntimePayload>, CoreError> {
    match action {
      "bootstrap" => {
        ctx
          .registry
          .singleton_or_insert_with(SocketRegistry::KEY, || SocketRegistry::default());
        self.rebuild_trigger_index(ctx.registry.metadata);
      }
      "watch_events" => {
        self.event_rx = Some(ctx.event_bus.subscribe::<rind_core::prelude::FlowEvent>());
      }
      "drain_events" => {
        if let Some(rx) = &self.event_rx {
          while let Some(w) = rx.try_recv() {
            let mut trig = crate::services::EmitTrigger::default();
            trig.state = Some(w.name);
            trig.payload = Some(crate::flow::FlowPayload::from_json(Some(w.payload)));
            trig.flow_type = Some(match w.flow_type {
              rind_core::prelude::FlowEventType::State => crate::flow::FlowType::State,
              rind_core::prelude::FlowEventType::Signal => crate::flow::FlowType::Signal,
            });
            trig.action = w.action;
            let _ = dispatch.dispatch(
              "sockets",
              "evaluate_triggers",
              RuntimePayload::default().insert("trigger", trig),
            );
          }
        }
      }
      "evaluate_triggers" => {
        let emit_trig = payload
          .get::<crate::services::EmitTrigger>("trigger")
          .unwrap_or_default();

        if self.trigger_index.is_empty() {
          self.rebuild_trigger_index(ctx.registry.metadata);
        }

        ctx
          .registry
          .singleton_handle::<(&mut StateMachine, &mut SocketRegistry), _>(
            (StateMachine::KEY.into(), SocketRegistry::KEY.into()),
            |registry, (sm, sr)| {
              let target_keys = if let Some(event_name) = emit_trig.state.as_ref() {
                self
                  .trigger_index
                  .get(event_name)
                  .cloned()
                  .unwrap_or_default()
              } else {
                registry
                  .metadata
                  .items::<Socket>("units")
                  .unwrap_or_default()
                  .into_iter()
                  .map(|(group, meta)| Ustr::from(format!("{}@{}", group, meta.name)))
                  .collect::<std::collections::HashSet<Ustr>>()
              };

              let emit_event = match (
                emit_trig.state.as_ref(),
                emit_trig.flow_type,
                emit_trig.payload.as_ref(),
              ) {
                (Some(name), Some(flow_type), Some(payload)) => Some(crate::flow::FlowInstance {
                  name: name.clone().into(),
                  payload: payload.clone(),
                  r#type: flow_type,
                }),
                _ => None,
              };

              for socket_name in target_keys {
                let Some(meta) = registry
                  .metadata
                  .find::<Socket>("units", socket_name.as_str())
                else {
                  continue;
                };

                let is_active =
                  if let Ok(sock) = registry.as_one::<Socket>("units", socket_name.as_str()) {
                    sock.active
                  } else {
                    false
                  };

                let should_start = meta
                  .start_on
                  .as_ref()
                  .map(|conds| {
                    conds.iter().any(|cond| {
                      crate::flow::condition_matches(sm, cond, emit_event.as_ref(), None)
                    })
                  })
                  .unwrap_or(false);

                let should_stop = meta
                  .stop_on
                  .as_ref()
                  .map(|conds| {
                    conds.iter().any(|cond| {
                      crate::flow::condition_matches(sm, cond, emit_event.as_ref(), None)
                    })
                  })
                  .unwrap_or(false);

                if should_start && !is_active {
                  let _ = self.start_socket(socket_name.clone(), ctx.resources, registry, sr);
                } else if should_stop && is_active {
                  let _ = self.stop_socket(socket_name.clone(), ctx.resources, registry, sr);
                }
              }
              Ok(())
            },
          )?;
      }
      "setup_all" => {
        ctx
          .registry
          .singleton_handle::<(&mut StateMachine, &mut VariableHeap, &mut SocketRegistry), _>(
            (
              StateMachine::KEY.into(),
              VariableHeap::KEY.into(),
              SocketRegistry::KEY.into(),
            ),
            |registry, (sm, _vh, sr)| {
              let Some(active) = sm.states.get("rind@active") else {
                return Ok(());
              };

              for branch in active {
                self.start_socket(
                  branch.payload.to_string_payload().to_ustr(),
                  ctx.resources,
                  registry,
                  sr,
                )?;
              }

              Ok(())
            },
          )?;
      }
      "get_all_fds" => {
        let fds: Vec<_> = self.instances.keys().map(|i| *i).collect();
        return Ok(Some(rpayload!({
          "fds": fds
        })));
      }
      "get_inherited_fds" => {
        let owner = payload.get::<Ustr>("owner")?;
        let fds = self.owner_fds(&owner);
        return Ok(Some(rpayload!({
          "fds": fds
        })));
      }
      "stop" => {
        let name = payload.get::<Ustr>("name")?;

        ctx
          .registry
          .singleton_handle::<(&mut SocketRegistry, &mut VariableHeap), _>(
            (SocketRegistry::KEY.into(), VariableHeap::KEY.into()),
            |registry, (sr, _)| self.stop_socket(name, ctx.resources, registry, sr),
          )?;
      }
      "start" => {
        let name = payload.get::<Ustr>("name")?;

        ctx
          .registry
          .singleton_handle::<(&mut SocketRegistry, &mut VariableHeap), _>(
            (SocketRegistry::KEY.into(), VariableHeap::KEY.into()),
            |registry, (sr, _)| self.start_socket(name, ctx.resources, registry, sr),
          )?;
      }
      "reset_fds" | "resume_fds" => {
        let owner = payload.get::<Ustr>("name")?;
        if let Some(fds) = self.paused.remove(&owner) {
          for fd in fds {
            ctx.resources.resume(fd);
          }
          if let Some(n) = &ctx.notifier {
            n.notify()?;
          }
        }
      }
      "clear_for" => {
        let name = payload.get::<Ustr>("name")?;

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
          let Ok(socket) = ctx.registry.as_one::<Socket>("units", sock.clone()) else {
            continue;
          };

          self.clear_socket(socket);
        }
      }
      "clear" => {
        let name = payload.get::<Ustr>("name")?;
        let socket = ctx.registry.as_one::<Socket>("units", name.clone())?;
        self.clear_socket(socket);
      }
      "drain_incoming" => {
        let fd = payload.get::<i32>("fd")? as RawFd;
        let name = self.instances.get(&fd).ok_or(CoreError::InvalidState(
          "Socket for fd was not found".into(),
        ))?;
        let socket = ctx.registry.as_one::<Socket>("units", name.clone())?;
        ctx.resources.pause(fd);

        log.log(
          LogLevel::Trace,
          "sockets",
          "socket accessed",
          [("name".to_string(), name.to_string())].into(),
        );

        if let Some(owner) = socket.metadata.owner.clone() {
          self.paused.entry(owner.clone()).or_default().push(fd);

          if let SocketServiceLifecycle::Owned = &socket.metadata.lifecycle {
            let owner_fds = self.owner_fds(&owner);
            let socket_fds: Vec<i32> = owner_fds.iter().map(|(fd, _)| *fd).collect();
            let socket_fd_names: Vec<Ustr> = owner_fds.into_iter().map(|(_, name)| name).collect();

            let _ = dispatch.dispatch(
              "services",
              "start",
              rpayload!({
                "name": owner,
                "socket_fds": socket_fds,
                "socket_fd_names": socket_fd_names,
              }),
            );
          }
        }

        if let Some(triggers) = &socket.metadata.trigger {
          let triggers = triggers.clone();
          ctx.registry.singleton_handle::<(&mut StateMachine,), _>(
            (StateMachine::KEY.into(),),
            |_, (sm,)| {
              trigger_events(triggers, Some(sm), dispatch);
              Ok(())
            },
          )?;
        }
      }
      _ => {}
    }
    Ok(None)
  }
}

fn ipc_owner_has_access(_owner: &Ustr, _user: &UserRecord) -> bool {
  true
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

  let sock = ctx.registry.metadata.find::<Socket>("units", &payload.name);
  let caller = pm.users.lookup_by_uid(uid);
  let can_manage = if uid == 0 || pm.user_has(uid, PERM_SYSTEM_SERVICES) {
    true
  } else if let (Some(user), Some(sock)) = (caller, sock.as_ref()) {
    sock
      .owner
      .as_ref()
      .map_or(false, |owner| ipc_owner_has_access(owner, user))
  } else {
    false
  };

  if !can_manage {
    return Err(CoreError::PermissionDenied);
  }

  let _ = dispatch.dispatch(
    "sockets",
    "start",
    rpayload!({ "name": payload.name.to_ustr() }),
  );

  if payload.persist {
    let _ = dispatch.dispatch(
      "flow",
      "set_state",
      rpayload!({ "name": "rind@active", "payload": payload.name.clone() }),
    );
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

  let sock = ctx.registry.metadata.find::<Socket>("units", &payload.name);
  let caller = pm.users.lookup_by_uid(uid);
  let can_manage = if uid == 0 || pm.user_has(uid, PERM_SYSTEM_SERVICES) {
    true
  } else if let (Some(user), Some(sock)) = (caller, sock.as_ref()) {
    sock
      .owner
      .as_ref()
      .map_or(false, |owner| ipc_owner_has_access(owner, user))
  } else {
    false
  };

  if !can_manage {
    return Err(CoreError::PermissionDenied);
  }

  let _ = dispatch.dispatch(
    "sockets",
    "stop",
    rpayload!({ "name": payload.name.to_ustr() }),
  );

  if payload.persist {
    let _ = dispatch.dispatch(
      "flow",
      "remove_state",
      rpayload!({ "name": "rind@active", "payload": payload.name.clone() }),
    );
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
