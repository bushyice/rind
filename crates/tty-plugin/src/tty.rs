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
  triggers::TriggerActions,
  *,
};
use rind_ipc::{Message, recv::IpcSourcemap};
use rind_plugins::prelude::*;
use rind_plugins_common::{SeatPayload, TTYEvent};
use rind_primitives::mounts::MountMetadata;

mod seatd;

plugin_extensible!(EXTENSIONS);

pub static PERM_SEAT: PermissionId = PermissionId(1004);

fn seat_to_tty(seat: &str) -> String {
  let n = seat
    .strip_prefix("seat")
    .and_then(|n| n.parse::<u64>().ok())
    .unwrap_or(0);
  format!("tty{}", n + 1)
}

fn tty_to_seat(tty: &str) -> String {
  let n = tty
    .strip_prefix("tty")
    .and_then(|n| n.parse::<u64>().ok())
    .unwrap_or(1);
  format!("seat{}", n.saturating_sub(1))
}

#[derive(Default)]
struct SeatdOrchestrator;

impl Orchestrator for SeatdOrchestrator {
  fn id(&self) -> &str {
    "seatd"
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
    SeatRuntime::actions.bootstrap().orchestrate(ctx)?;
    SeatRuntime::actions.watch_events().orchestrate(ctx)?;
    Ok(Void)
  }

  fn runtimes(&self) -> Vec<Box<dyn Runtime>> {
    vec![Box::new(SeatRuntime::default())]
  }
}

struct SeatdPumpOrchestrator;

impl Orchestrator for SeatdPumpOrchestrator {
  fn id(&self) -> &str {
    "seatd-pump"
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
    SeatRuntime::actions.drain_events().orchestrate(ctx)?;
    Ok(Void)
  }
}

const VT_ACTIVATE: libc::c_ulong = 0x5606;
const VT_WAITACTIVE: libc::c_ulong = 0x5607;

pub struct SeatRuntime {
  active: String,
  seats: Vec<String>,
  event_rx: Option<Subscription<FlowEvent>>,
}

impl Default for SeatRuntime {
  fn default() -> Self {
    Self {
      active: "seat0".to_string(),
      seats: Vec::new(),
      event_rx: None,
    }
  }
}

impl SeatRuntime {
  fn switch_seat(&self, seat: &str) -> CoreResult<Void> {
    let tty = seat_to_tty(seat);
    let n = tty
      .strip_prefix("tty")
      .and_then(|n| n.parse::<u64>().ok())
      .ok_or_else(|| CoreError::InvalidState(format!("Invalid TTY from seat: {seat:?}")))?;

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

  fn has_login_required(&self, sm: &FacetGraph, seat: &str) -> bool {
    sm.facets.get("seat:login_required").map_or(false, |x| {
      x.iter().any(|x| {
        x.payload
          .get_json_field_as::<String>("seat")
          .map_or(false, |v| v == seat)
      })
    })
  }

  fn reconcile_login(
    &self,
    sm: &FacetGraph,
    dispatch: &RuntimeDispatcher,
    seat: &str,
    last: &str,
  ) -> CoreResult<Void> {
    if seat != last && self.has_login_required(sm, last) {
      FlowRuntime::actions
        .remove_facet("seat:login_required".into())
        .payload(json!({ "seat": last.to_string() }))
        .dispatch(dispatch)?;
    }

    if sm.facets.get("seat:taken").map_or(true, |x| {
      !x.iter().any(|x| x.payload.to_string_payload() == seat)
    }) && sm.facets.get("rind:user_session").map_or(true, |x| {
      !x.iter().any(|x| {
        x.payload
          .get_json_field_as::<String>("seat")
          .map_or(false, |x| tty_to_seat(&x) == seat)
      })
    }) && !self.has_login_required(sm, seat)
    {
      FlowRuntime::actions
        .set_facet("seat:login_required".into())
        .payload(json!({ "seat": seat.to_string() }))
        .dispatch(dispatch)?;
    }

    Ok(Void)
  }

  fn taken_state(&self, dispatch: &RuntimeDispatcher, seat: Ustr, take: bool) -> CoreResult<Void> {
    dispatch.dispatch(
      "flow",
      if take { "set_facet" } else { "remove_facet" },
      FlowRuntimePayload::new("seat:taken")
        .payload(serde_json::Value::String(seat.to_string()))
        .into(),
    )
  }

  fn discover_seats() -> Vec<String> {
    let limit = std::env::var("RIND_ACTIVATE_TTYS")
      .ok()
      .and_then(|v| v.parse::<usize>().ok())
      .unwrap_or(7);

    let mut seats = Vec::new();
    if let Ok(dir) = fs::read_dir("/sys/class/tty") {
      let mut entries: Vec<_> = dir.filter_map(|e| e.ok()).collect();
      entries.sort_by_key(|e| {
        let name = e.file_name();
        let name = name.to_string_lossy();
        name
          .strip_prefix("tty")
          .and_then(|n| n.parse::<u32>().ok())
          .unwrap_or(u32::MAX)
      });
      for (i, item) in entries.iter().enumerate() {
        let name = item.file_name();
        let name = name.to_string_lossy();
        if name.starts_with("tty") && name != "tty" && name != "tty0" && i < limit {
          seats.push(format!("seat{i}"));
        }
      }
    }
    seats
  }
}

#[runtime("seatd")]
impl SeatRuntime {
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

    let current_seat = tty_to_seat(&current_tty);
    self.active = current_seat.clone();
    self.seats = Self::discover_seats();

    let sm = ctx
      .registry
      .singleton::<FacetGraph>(FacetGraph::KEY)
      .ok_or(CoreError::RuntimeStopped)?;

    if let Some(target_seat) = sm
      .facets
      .get("seat:active")
      .and_then(|instances| instances.first())
      .map(|x| x.payload.to_string_payload())
    {
      if target_seat != current_seat {
        log.log(
          LogLevel::Info,
          "seatd",
          "switching seat",
          [
            ("current".to_string(), current_seat),
            ("target".to_string(), target_seat.clone()),
          ]
          .into(),
        );

        self.switch_seat(&target_seat)?;
      }
    }

    let file =
      File::open("/sys/class/tty/tty0/active").map_err(|e| CoreError::Custom(e.to_string()))?;
    let fd = file.as_raw_fd();
    ctx.resources.own(fd, OwnedFd::from(file));
    ctx
      .resources
      .flag(fd, EpollFlags::EPOLLPRI | EpollFlags::EPOLLERR);
    ctx.resources.action(fd, ("seatd", "on_switch"));

    let ipcsrc = ctx.scope.get::<IpcSourcemap>().cloned().unwrap_or_default();
    ipcsrc.register("seat", handle_ipc_seat, PERM_SEAT);

    match EXTENSIONS.with(|extensions| {
      extensions
        .get()
        .ok_or(CoreError::InvalidState(
          "extension manager not initialized".into(),
        ))?
        .resolve(
          "boot",
          SeatPayload::Taken(sm.facets.get("seat:taken").map_or(Default::default(), |x| {
            x.iter()
              .map(|x| x.payload.to_string_payload().to_ustr())
              .collect()
          })),
        )
    })? {
      SeatPayload::Return(seat) => self.taken_state(dispatch, seat, false)?,
      SeatPayload::Take(seat) => self.taken_state(dispatch, seat, true)?,
      _ => {}
    }

    std::thread::spawn(|| seatd::start());

    self.__runtime_reconcile(payload, ctx, dispatch, log)?;
  }

  fn reconcile() {
    let sm = ctx
      .registry
      .singleton::<FacetGraph>(FacetGraph::KEY)
      .ok_or(CoreError::RuntimeStopped)?;
    self.reconcile_login(sm, dispatch, &self.active, &self.active)?;
  }

  fn take(seat: Ustr) {
    log.log(
      LogLevel::Info,
      "seatd",
      &format!("taking seat {seat}"),
      Default::default(),
    );
    self.taken_state(dispatch, seat, true)?;
  }

  fn return_seat(seat: Ustr) {
    log.log(
      LogLevel::Info,
      "seatd",
      &format!("returning seat {seat}"),
      Default::default(),
    );
    self.taken_state(dispatch, seat, false)?;
  }

  fn act_trigger(args: Vec<Ustr>) {
    if args.len() < 2 {
      return Ok(None);
    }

    let act = args.remove(0);
    let tty = args.remove(0);

    let sm = ctx
      .registry
      .singleton::<FacetGraph>(FacetGraph::KEY)
      .ok_or(CoreError::RuntimeStopped)?;

    match &**act {
      "take" => {
        self.taken_state(dispatch, tty.clone(), true)?;
        if self.active == &**tty {
          self.reconcile_login(sm, dispatch, &self.active, "seat-1")?;
        }
      }
      "return" => {
        self.taken_state(dispatch, tty.clone(), false)?;
        if self.active == &**tty {
          self.reconcile_login(sm, dispatch, &self.active, "seat-1")?;
        }
      }
      _ => {}
    }
  }

  fn activate(seat: Ustr) {
    log.log(
      LogLevel::Info,
      "seatd",
      &format!("activating seat {seat}"),
      Default::default(),
    );
    self.switch_seat(seat.as_str())?;
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
      let seat_name = tty_to_seat(tty_name);

      self.active = seat_name.clone();

      let sm = ctx
        .registry
        .singleton::<FacetGraph>(FacetGraph::KEY)
        .ok_or(CoreError::RuntimeStopped)?;

      if sm.facets.get("seat:active").map_or(true, |x| {
        !x.iter().any(|x| x.payload.to_string_payload() == seat_name)
      }) {
        FlowRuntime::actions
          .remove_facet("seat:active".into())
          .payload(serde_json::Value::String(last.clone()))
          .dispatch(dispatch)?;

        FlowRuntime::actions
          .set_facet("seat:active".into())
          .payload(serde_json::Value::String(seat_name.clone()))
          .dispatch(dispatch)?;
      }

      if seat_name == last {
        return Ok(None);
      }

      FlowRuntime::actions
        .impulse("seat:switch".into())
        .payload(serde_json::Value::String(seat_name.clone()))
        .dispatch(dispatch)?;

      ctx.event_bus.emit(TTYEvent {
        tty: tty_name.to_ustr(),
        from: last.to_ustr(),
      });

      self.reconcile_login(sm, dispatch, &seat_name, &last)?;
    }
  }
}

fn handle_ipc_seat(
  msg: Message,
  ctx: &mut RuntimeContext<'_>,
  dispatch: &RuntimeDispatcher,
  _log: &LogHandle,
) -> CoreResult<Message> {
  let payload = msg
    .parse_payload::<SeatPayload>()
    .map_err(CoreError::Custom)?;

  let sm = ctx
    .registry
    .singleton::<FacetGraph>(FacetGraph::KEY)
    .ok_or(CoreError::RuntimeStopped)?;

  match payload {
    SeatPayload::Check => {
      return Ok(Message::ok(
        sm.facets
          .get("seat:active")
          .and_then(|x| x.first().map(|x| x.payload.to_string_payload()))
          .unwrap_or("seat0".to_string()),
      ));
    }
    SeatPayload::Take(seat) => dispatch.dispatch(
      "seatd",
      "take",
      RuntimePayload::default().insert("seat", seat),
    )?,
    SeatPayload::Return(seat) => dispatch.dispatch(
      "seatd",
      "return_seat",
      RuntimePayload::default().insert("seat", seat),
    )?,
    SeatPayload::Activate(seat) => dispatch.dispatch(
      "seatd",
      "activate",
      RuntimePayload::default().insert("seat", seat),
    )?,
    SeatPayload::List => {
      let taken: Vec<String> = sm.facets.get("seat:taken").map_or(Vec::new(), |x| {
        x.iter().map(|x| x.payload.to_string_payload()).collect()
      });
      let login_required: Vec<String> =
        sm.facets
          .get("seat:login_required")
          .map_or(Vec::new(), |x| {
            x.iter()
              .filter_map(|x| x.payload.get_json_field_as::<String>("seat"))
              .collect()
          });
      let active = sm
        .facets
        .get("seat:active")
        .and_then(|x| x.first())
        .map(|x| x.payload.to_string_payload())
        .unwrap_or_default();
      let sessions: Vec<serde_json::Value> =
        sm.facets.get("seat:session").map_or(Vec::new(), |x| {
          x.iter()
            .filter_map(|x| {
              let v = x.payload.to_json();
              Some(json!({
                "seat": v.get("seat")?,
                "session": v.get("session")?,
                "user": v.get("user")?,
              }))
            })
            .collect()
        });

      return Ok(Message::ok(
        serde_json::to_string(&json!({
          "active": active,
          "taken": taken,
          "login_required": login_required,
          "sessions": sessions,
        }))
        .unwrap_or_default(),
      ));
    }
    SeatPayload::Session {
      seat,
      session,
      user,
    } => {
      FlowRuntime::actions
        .set_facet("seat:session".into())
        .payload(json!({
          "seat": seat.as_str(),
          "session": session.as_str(),
          "user": user,
        }))
        .dispatch(dispatch)?;
    }
    SeatPayload::SessionEnd {
      seat: _,
      session: _,
    } => {
      FlowRuntime::actions
        .remove_facet("seat:session".into())
        .payload(serde_json::Value::Null)
        .dispatch(dispatch)?;
    }
    SeatPayload::Devices(seat) => {
      let tty = seat_to_tty(seat.as_str());
      return Ok(Message::ok(
        serde_json::to_string(&json!({
          "seat": seat.as_str(),
          "tty": tty,
          "devices": [
            format!("/dev/{tty}"),
          ],
        }))
        .unwrap_or_default(),
      ));
    }
    SeatPayload::Taken(_) => {}
  }

  Ok(Message::ok("ok"))
}

fn inject_builtin(name: &str, mut metadata: Metadata) -> CoreResult<Metadata> {
  match name {
    "built_in" => {
      metadata
        .group("seat")
        .insert::<FlowFacet>(FlowFacetMetadata {
          name: "login_required".into(),
          payload: FlowPayloadType::Json,
          branch: Some(vec!["seat".into()]),
          stop_on: Some(vec![InverseBranchingConfig::Detailed {
            name: "rind:user_session".into(),
            branch: Some("seat".into()),
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
        .insert::<FlowFacet>(FlowFacetMetadata {
          name: "session".into(),
          payload: FlowPayloadType::Json,
          branch: Some(vec!["seat".into()]),
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

fn register_trigger(_: &str, actions: &mut TriggerActions) -> CoreResult<()> {
  actions.insert("tty", "tty:act_trigger");
  Ok(())
}

plugin!(
  name: "tty",
  version: 0,
  caps: PluginCapability::EXTENSIONS | PluginCapability::EXTENSIBLE | PluginCapability::ORCHESTRATORS | PluginCapability::RUNTIMES | PluginCapability::IPC,
  deps: &[],
  create: SeatPlugin,
  orchestrators: [SeatdOrchestrator::default(), SeatdPumpOrchestrator],
  extensions: [resolve(inject_builtin), resolve(trigger_ttyload), act(register_trigger)],
  struct SeatPlugin;
);

plugin_abi!(1);
