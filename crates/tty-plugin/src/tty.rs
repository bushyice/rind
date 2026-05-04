// To handle tty, maybe either set the login_required state for each tty only on-access and set a timer to
// remove that state whenever it's not accessed for a while
//
// or maybe, make login_required just a signal instead of a state, and have a timer stop the service
// when it's not being accessed anymore?

use std::{
  fs::{self, File, OpenOptions},
  os::fd::{AsRawFd, BorrowedFd, OwnedFd, RawFd},
};

use rind_plugins::prelude::{
  nix::{
    sys::epoll::EpollFlags,
    unistd::{Whence, lseek, read},
  },
  serde_json::json,
  *,
};

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

  fn run(&mut self, ctx: &mut OrchestratorContext<'_>) -> Result<(), CoreError> {
    ctx.dispatch("ttys", "bootstrap", Default::default())?;
    ctx.dispatch("ttys", "watch_events", Default::default())?;

    Ok(())
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

  fn run(&mut self, ctx: &mut OrchestratorContext<'_>) -> Result<(), CoreError> {
    ctx.dispatch("ttys", "drain_events", Default::default())?;

    Ok(())
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
  fn switch_tty(&self, n: u64) -> CoreResult<()> {
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

    Ok(())
  }

  fn has_login_required(&self, sm: &StateMachine, tty: &str) -> bool {
    sm.states.get("tty@login_required").map_or(false, |x| {
      x.iter().any(|x| {
        x.payload
          .get_json_field_as::<String>("tty")
          .map_or(false, |v| v == tty)
      })
    })
  }

  fn reconcile_login(
    &self,
    sm: &StateMachine,
    dispatch: &RuntimeDispatcher,
    tty_name: &str,
    last: &str,
  ) -> CoreResult<()> {
    if tty_name != last && self.has_login_required(sm, &last) {
      dispatch.dispatch(
        "flow",
        "remove_state",
        FlowRuntimePayload::new("tty@login_required")
          .payload(json!({ "tty": last.to_string() }))
          .into(),
      )?;
    }

    if sm.states.get("tty@taken").map_or(true, |x| {
      !x.iter().any(|x| x.payload.to_string_payload() == tty_name)
    }) && sm.states.get("rind@user_session").map_or(true, |x| {
      !x.iter().any(|x| {
        x.payload
          .get_json_field_as::<String>("tty")
          .map_or(false, |x| x == format!("/dev/{tty_name}"))
      })
    }) && !self.has_login_required(sm, &tty_name)
    {
      dispatch.dispatch(
        "flow",
        "set_state",
        FlowRuntimePayload::new("tty@login_required")
          .payload(json!({ "tty": tty_name.to_string() }))
          .into(),
      )?;
    }

    Ok(())
  }
}

impl Runtime for TTYRuntime {
  fn id(&self) -> &str {
    "ttys"
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
      "watch_events" => {
        self.event_rx = Some(ctx.event_bus.subscribe::<FlowEvent>());
      }
      "drain_events" => {
        if let Some(rx) = &self.event_rx {
          while let Some(w) = rx.try_recv() {
            if w.name.as_str() == "rind@user_session" {
              self.reconcile_login(
                ctx
                  .registry
                  .singleton::<StateMachine>(StateMachine::KEY)
                  .ok_or(CoreError::RuntimeStopped)?,
                dispatch,
                &self.active,
                &self.active,
              )?;
            }
          }
        }
      }
      "bootstrap" => {
        let current_tty = fs::read_to_string("/sys/class/tty/tty0/active")?
          .trim()
          .to_string();

        self.active = current_tty.clone();

        if let Some(target_name) = ctx
          .registry
          .singleton::<StateMachine>(StateMachine::KEY)
          .and_then(|sm| sm.states.get("tty@active"))
          .and_then(|instances| instances.first())
          .map(|x| x.payload.to_string_payload())
        {
          if target_name != current_tty {
            let tty_num = target_name
              .strip_prefix("tty")
              .and_then(|n| n.parse::<u64>().ok())
              .ok_or_else(|| {
                CoreError::InvalidState(format!("Invalid TTY name: {:?}", target_name))
              })?;

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
          .singleton::<StateMachine>(StateMachine::KEY)
          .ok_or(CoreError::RuntimeStopped)?;

        self.reconcile_login(sm, dispatch, &self.active, &self.active)?;
      }
      "on_switch" => {
        let fd = payload.get::<i32>("fd")? as RawFd;
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
            .singleton::<StateMachine>(StateMachine::KEY)
            .ok_or(CoreError::RuntimeStopped)?;

          if sm.states.get("tty@active").map_or(true, |x| {
            !x.iter().any(|x| x.payload.to_string_payload() == tty_name)
          }) {
            dispatch.dispatch(
              "flow",
              "remove_state",
              FlowRuntimePayload::new("tty@active")
                .payload(serde_json::Value::String(last.clone()))
                .into(),
            )?;

            dispatch.dispatch(
              "flow",
              "set_state",
              FlowRuntimePayload::new("tty@active")
                .payload(serde_json::Value::String(tty_name.to_string()))
                .into(),
            )?;
          }

          if tty_name == last {
            return Ok(None);
          }

          dispatch.dispatch(
            "flow",
            "emit_signal",
            FlowRuntimePayload::new("tty@switch")
              .payload(serde_json::Value::String(tty_name.to_string()))
              .into(),
          )?;

          self.reconcile_login(sm, dispatch, tty_name, &last)?;
        }
      }
      _ => {}
    }

    Ok(None)
  }
}

fn inject_builtin(name: &str, mut metadata: Metadata) -> CoreResult<Metadata> {
  match name {
    "built_in" => {
      metadata
        .from_toml(
          r#"
          [[variable]]
          name = "ttys"
          default = ["/dev/tty1", "/dev/tty2"]

          [[state]]
          name = "login_required"
          payload = "json"
          branch = ["tty"]
          stop-on = [{
            name = "rind@user_session",
            branch = "tty"
          }]

          [[signal]]
          name = "switch"
          payload = "string"

          [[state]]
          name = "taken"
          payload = "string"

          [[state]]
          name = "active"
          payload = "string"
      "#,
          "tty",
        )
        .ok();
      Ok(metadata)
    }
    _ => Ok(metadata),
  }
}

plugin!(
  name: "myplugin",
  version: 0,
  caps: PluginCapability::all(),
  deps: &[],
  create: MyPlugin,
  orchestrators: [TTYOrchestrator::default(), TTYPumpOrchestrator],
  extensions: [resolve(inject_builtin)],
  struct MyPlugin;
);

plugin_abi!(1);
