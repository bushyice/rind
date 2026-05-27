// To handle tty, maybe either set the login_required state for each tty only on-access and set a timer to
// remove that state whenever it's not accessed for a while
//
// or maybe, make login_required just a signal instead of a state, and have a timer stop the service
// when it's not being accessed anymore?

use std::{
  fs::{self, File, OpenOptions},
  os::fd::{AsRawFd, BorrowedFd, OwnedFd, RawFd},
  sync::Arc,
};

use rind_core::prelude::*;
use rind_core::reexports::{nix::sys::epoll::EpollFlags, *};
use rind_core::reexports::{
  nix::unistd::{Whence, lseek, read},
  serde_json::json,
};
use rind_flow::{
  transport::{TransportMethod, TransportProtocolId},
  *,
};
use rind_ipc::{Message, recv::IpcSourcemap};
use rind_plugins::prelude::*;
use rind_plugins_common::TTYPayload;
use rind_primitives::mounts::MountMetadata;

plugin_extensible!(EXTENSIONS);

pub static PERM_TTY: PermissionId = PermissionId(1004);

#[derive(Default)]
struct TTYOrchestrator;

impl Orchestrator for TTYOrchestrator {
  fn id(&self) -> &str {
    "ttys"
  }

  fn depends_on(&self) -> &[&str] {
    &[]
  }

  fn when(&self) -> OrchestratorWhen<'static> {
    OrchestratorWhen {
      cycle: &[BootCycle::Collect, BootCycle::Runtime],
      phase: BootPhase::Start,
    }
  }

  fn run(&mut self, ctx: &mut OrchestratorContext<'_>) -> Result<Void, CoreError> {
    TTYRuntime::actions.bootstrap().orchestrate(ctx)?;
    TTYRuntime::actions.watch_events().orchestrate(ctx)?;

    Ok(Void)
  }

  fn runtimes(&self) -> Vec<Box<dyn Runtime>> {
    vec![Box::new(TTYRuntime::default())]
  }
}

struct TTYPumpOrchestrator;

impl Orchestrator for TTYPumpOrchestrator {
  fn id(&self) -> &str {
    "tty-pump"
  }

  fn depends_on(&self) -> &[&str] {
    &[]
  }

  fn when(&self) -> OrchestratorWhen<'static> {
    OrchestratorWhen {
      cycle: &[BootCycle::Pump],
      phase: BootPhase::End,
    }
  }

  fn run(&mut self, ctx: &mut OrchestratorContext<'_>) -> Result<Void, CoreError> {
    TTYRuntime::actions.drain_events().orchestrate(ctx)?;
    Ok(Void)
  }
}

const VT_ACTIVATE: libc::c_ulong = 0x5606;
const VT_WAITACTIVE: libc::c_ulong = 0x5607;

pub struct TTYRuntime {
  active: String,
  event_rx: Option<Subscription<FlowEvent>>,
}

impl Default for TTYRuntime {
  fn default() -> Self {
    Self {
      active: "tty1".to_string(),
      event_rx: None,
    }
  }
}

impl TTYRuntime {
  fn switch_tty(&self, n: u64) -> CoreResult<Void> {
    let file = OpenOptions::new()
      .read(true)
      .write(true)
      .open("/dev/tty0")?;

    let fd = file.as_raw_fd();

    unsafe {
      if libc::ioctl(fd, VT_ACTIVATE, n) == -1 {
        return Err(CoreError::Custom(
          std::io::Error::last_os_error().to_string(),
        ));
      }
      if libc::ioctl(fd, VT_WAITACTIVE, n) == -1 {
        return Err(CoreError::Custom(
          std::io::Error::last_os_error().to_string(),
        ));
      }
    }

    Ok(Void)
  }

  fn has_login_required(&self, sm: &FacetGraph, tty: &str) -> bool {
    sm.facets.get("tty:login_required").map_or(false, |x| {
      x.iter().any(|x| {
        x.payload
          .get_json_field_as::<String>("tty")
          .map_or(false, |v| v == tty)
      })
    })
  }

  fn reconcile_login(
    &self,
    sm: &FacetGraph,
    dispatch: &RuntimeDispatcher,
    tty_name: &str,
    last: &str,
  ) -> CoreResult<Void> {
    if tty_name != last && self.has_login_required(sm, &last) {
      FlowRuntime::actions
        .remove_facet("tty:login_required".into())
        .payload(json!({ "tty": last.to_string() }))
        .dispatch(dispatch)?;
    }

    if sm.facets.get("tty:taken").map_or(true, |x| {
      !x.iter().any(|x| x.payload.to_string_payload() == tty_name)
    }) && sm.facets.get("rind:user_session").map_or(true, |x| {
      !x.iter().any(|x| {
        x.payload
          .get_json_field_as::<String>("tty")
          .map_or(false, |x| x == tty_name)
      })
    }) && !self.has_login_required(sm, &tty_name)
    {
      FlowRuntime::actions
        .set_facet("tty:login_required".into())
        .payload(json!({ "tty": tty_name.to_string() }))
        .dispatch(dispatch)?;
    }

    Ok(Void)
  }

  fn taken_state(
    &self,
    dispatch: &RuntimeDispatcher,
    tty_name: Ustr,
    take: bool,
  ) -> CoreResult<Void> {
    // keep
    dispatch.dispatch(
      "flow",
      if take { "set_facet" } else { "remove_facet" },
      FlowRuntimePayload::new("tty:taken")
        .payload(serde_json::Value::String(tty_name.to_string()))
        .into(),
    )
  }
}

#[runtime("ttys")]
impl TTYRuntime {
  fn watch_events() {
    self.event_rx = Some(ctx.event_bus.subscribe::<FlowEvent>());
  }

  fn drain_events() {
    if let Some(rx) = &self.event_rx {
      while let Some(w) = rx.try_recv() {
        if w.name.as_str() == "rind:user_session" {
          self.reconcile_login(
            ctx
              .registry
              .singleton::<FacetGraph>(FacetGraph::KEY)
              .ok_or(CoreError::RuntimeStopped)?,
            dispatch,
            &self.active,
            &self.active,
          )?;
        }
      }
    }
  }

  fn bootstrap() {
    let current_tty = fs::read_to_string("/sys/class/tty/tty0/active")?
      .trim()
      .to_string();

    self.active = current_tty.clone();

    if let Some(target_name) = ctx
      .registry
      .singleton::<FacetGraph>(FacetGraph::KEY)
      .and_then(|sm| sm.facets.get("tty:active"))
      .and_then(|instances| instances.first())
      .map(|x| x.payload.to_string_payload())
    {
      if target_name != current_tty {
        let tty_num = target_name
          .strip_prefix("tty")
          .and_then(|n| n.parse::<u64>().ok())
          .ok_or_else(|| CoreError::InvalidState(format!("Invalid TTY name: {:?}", target_name)))?;

        log.log(
          LogLevel::Info,
          "tty",
          "switching tty",
          [("tty".to_string(), current_tty)].into(),
        );

        self.switch_tty(tty_num)?;
      }
    }

    let file =
      File::open("/sys/class/tty/tty0/active").map_err(|e| CoreError::Custom(e.to_string()))?;

    let fd = file.as_raw_fd();

    ctx.resources.own(fd, OwnedFd::from(file));

    ctx
      .resources
      .flag(fd, EpollFlags::EPOLLPRI | EpollFlags::EPOLLERR);

    ctx.resources.action(fd, ("ttys", "on_switch"));

    let sm = ctx
      .registry
      .singleton::<FacetGraph>(FacetGraph::KEY)
      .ok_or(CoreError::RuntimeStopped)?;

    let ipcsrc = ctx.scope.get::<IpcSourcemap>().cloned().unwrap_or_default();
    ipcsrc.register("tty", handle_ipc_tty, PERM_TTY);

    match EXTENSIONS.with(|extensions| {
      extensions
        .get()
        .expect("extension manager not initialized")
        .resolve(
          "boot",
          TTYPayload::Taken(sm.facets.get("tty:taken").map_or(Default::default(), |x| {
            x.iter()
              .map(|x| x.payload.to_string_payload().to_ustr())
              .collect()
          })),
        )
    })? {
      TTYPayload::Return(tty) => self.taken_state(dispatch, tty, false)?,
      TTYPayload::Take(tty) => self.taken_state(dispatch, tty, true)?,
      _ => {}
    }

    self.__runtime_reconcile(payload, ctx, dispatch, log)?;
  }

  fn reconcile() {
    let sm = ctx
      .registry
      .singleton::<FacetGraph>(FacetGraph::KEY)
      .ok_or(CoreError::RuntimeStopped)?;
    self.reconcile_login(sm, dispatch, &self.active, &self.active)?;
  }

  fn take(tty: Ustr) {
    log.log(
      LogLevel::Info,
      "ttys",
      &format!("taking tty {tty}"),
      Default::default(),
    );
    self.taken_state(dispatch, tty, true)?;
  }

  fn return_tty(tty: Ustr) {
    log.log(
      LogLevel::Info,
      "ttys",
      &format!("returning tty {tty}"),
      Default::default(),
    );

    self.taken_state(dispatch, tty, false)?;
  }

  fn on_switch(fd: i32) {
    let fd = fd as RawFd;
    let bfd = unsafe { BorrowedFd::borrow_raw(fd) };
    let _ = lseek(bfd, 0, Whence::SeekSet);
    let last = self.active.clone();

    let mut buf = [0u8; 32];
    if let Ok(bytes_read) = read(bfd, &mut buf) {
      let content = String::from_utf8_lossy(&buf[..bytes_read]);
      let tty_name = content.trim();

      self.active = tty_name.to_string();

      let sm = ctx
        .registry
        .singleton::<FacetGraph>(FacetGraph::KEY)
        .ok_or(CoreError::RuntimeStopped)?;

      if sm.facets.get("tty:active").map_or(true, |x| {
        !x.iter().any(|x| x.payload.to_string_payload() == tty_name)
      }) {
        FlowRuntime::actions
          .remove_facet("tty:active".into())
          .payload(serde_json::Value::String(last.clone()))
          .dispatch(dispatch)?;

        FlowRuntime::actions
          .set_facet("tty:active".into())
          .payload(serde_json::Value::String(tty_name.to_string()))
          .dispatch(dispatch)?;
      }

      if tty_name == last {
        return Ok(None);
      }

      FlowRuntime::actions
        .impulse("tty:switch".into())
        .payload(serde_json::Value::String(tty_name.to_string()))
        .dispatch(dispatch)?;

      self.reconcile_login(sm, dispatch, tty_name, &last)?;
    }
  }
}

fn handle_ipc_tty(
  msg: Message,
  ctx: &mut RuntimeContext<'_>,
  dispatch: &RuntimeDispatcher,
  _log: &LogHandle,
) -> CoreResult<Message> {
  let payload = msg
    .parse_payload::<TTYPayload>()
    .map_err(CoreError::Custom)?;

  let sm = ctx
    .registry
    .singleton::<FacetGraph>(FacetGraph::KEY)
    .ok_or(CoreError::RuntimeStopped)?;

  match payload {
    TTYPayload::Check => {
      return Ok(Message::ok(
        sm.facets
          .get("tty:active")
          .and_then(|x| x.first().map(|x| x.payload.to_string_payload()))
          .unwrap_or("tty1".to_string()),
      ));
    }
    TTYPayload::Take(tty) => {
      dispatch.dispatch("ttys", "take", RuntimePayload::default().insert("tty", tty))?
    }
    TTYPayload::Return(tty) => dispatch.dispatch(
      "ttys",
      "return",
      RuntimePayload::default().insert("tty", tty),
    )?,
    _ => {}
  }

  Ok(Message::ok("ok"))
}

fn inject_builtin(name: &str, mut metadata: Metadata) -> CoreResult<Metadata> {
  match name {
    "built_in" => {
      metadata
        .group("tty")
        .insert::<FlowFacet>(FlowFacetMetadata {
          name: "login_required".into(),
          payload: FlowPayloadType::Json,
          branch: Some(vec!["tty".into()]),
          stop_on: Some(vec![InverseBranchingConfig::Detailed {
            name: "rind:user_session".into(),
            branch: Some("tty".into()),
          }]),
          subscribers: Some(vec![
            TransportMethod::Type(TransportProtocolId("route:rind:sys-uds".into())),
            TransportMethod::Type(TransportProtocolId("route:rind:sys-shm".into())),
          ]),
          ..Default::default()
        })
        .insert::<FlowFacet>(FlowFacetMetadata {
          name: "taken".into(),
          payload: FlowPayloadType::String,
          subscribers: Some(vec![
            TransportMethod::Type(TransportProtocolId("route:rind:sys-uds".into())),
            TransportMethod::Type(TransportProtocolId("route:rind:sys-shm".into())),
          ]),
          ..Default::default()
        })
        .insert::<FlowFacet>(FlowFacetMetadata {
          name: "active".into(),
          payload: FlowPayloadType::String,
          subscribers: Some(vec![
            TransportMethod::Type(TransportProtocolId("route:rind:sys-uds".into())),
            TransportMethod::Type(TransportProtocolId("route:rind:sys-shm".into())),
          ]),
          ..Default::default()
        })
        .insert::<FlowImpulse>(FlowImpulseMetadata {
          name: "switch".into(),
          payload: FlowPayloadType::String,
          subscribers: Some(vec![
            TransportMethod::Type(TransportProtocolId("route:rind:sys-uds".into())),
            TransportMethod::Type(TransportProtocolId("route:rind:sys-shm".into())),
          ]),
          ..Default::default()
        })
        .close();
      Ok(metadata)
    }
    _ => Ok(metadata),
  }
}

fn trigger_ttyload(
  name: &str,
  ctx: ExtensionExecutionCtx<Arc<MountMetadata>>,
) -> CoreResult<ExtensionExecutionCtx<Arc<MountMetadata>>> {
  match name {
    "mount" if ctx.target.target.as_str() == "/sys" => Ok(ctx.with_fn(|_, _, _| {
      let mut tty_count = 0;

      let limit = std::env::var("RIND_ACTIVATE_TTYS")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(7);

      if limit == 0 {
        return Ok(Box::new(()));
      }

      if let Ok(dir) = fs::read_dir("/sys/class/tty") {
        let mut entries: Vec<_> = dir.collect::<Result<Vec<_>, _>>()?;
        entries.sort_by_key(|e| {
          let name = e.file_name();
          let name = name.to_string_lossy();

          name
            .strip_prefix("tty")
            .and_then(|n| n.parse::<u32>().ok())
            .unwrap_or(u32::MAX)
        });

        for item in entries {
          let name = item.file_name();
          let name = name.to_string_lossy();

          // TODO: proper tty fetch
          if name.starts_with("tty") && name != "tty" && name != "tty0" && tty_count < limit {
            tty_count += 1;

            if let Ok(file) = OpenOptions::new().write(true).open(format!("/dev/{name}")) {
              if unsafe { libc::ioctl(file.as_raw_fd(), libc::TIOCSCTTY, 1) } != 0 {}
            }
          }
        }
      }

      Ok(Box::new(()))
    })),
    _ => Ok(ctx),
  }
}

plugin!(
  name: "myplugin",
  version: 0,
  caps: PluginCapability::EXTENSIONS | PluginCapability::EXTENSIBLE | PluginCapability::ORCHESTRATORS | PluginCapability::RUNTIMES | PluginCapability::IPC,
  deps: &[],
  create: MyPlugin,
  orchestrators: [TTYOrchestrator::default(), TTYPumpOrchestrator],
  extensions: [resolve(inject_builtin), resolve(trigger_ttyload)],
  struct MyPlugin;
);

plugin_abi!(1);
