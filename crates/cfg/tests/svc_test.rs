use std::path::PathBuf;
use std::time::Duration;

use rind_flow::{transport::TransportRuntime, *};
use rind_primitives::prelude::VariableHeap;
use rind_services::*;

use rind_core::{
  prelude::{
    InstanceRegistry, LogConfig, Metadata, MetadataRegistry, Resources, RuntimeHandle,
    RuntimePayload, ScopeBuilder, StatePersistence, Ustr, start_logger, start_runtime,
  },
  rpayload,
};

fn temp_path(tag: &str) -> PathBuf {
  let now = std::time::SystemTime::now()
    .duration_since(std::time::UNIX_EPOCH)
    .expect("clock before epoch")
    .as_nanos();
  std::env::temp_dir().join(format!("rind-test-{tag}-{}-{now}", std::process::id()))
}

fn setup_test_runtime() -> (RuntimeHandle, MetadataRegistry, Resources, usize) {
  let log = start_logger(LogConfig {
    dir: temp_path("logs"),
    ..LogConfig::default()
  });

  let runtime = start_runtime(
    log,
    vec![
      Box::new(FlowRuntime::default()),
      Box::new(TimerRuntime),
      Box::new(SocketRuntime::default()),
      Box::new(ServiceRuntime::default()),
      Box::new(ReaperRuntime),
      Box::new(TransportRuntime::default()),
    ],
    None,
  );

  let metadata = MetadataRegistry::default();
  let context_id = 1usize;

  runtime
    .register_scopes(context_id, ScopeBuilder::default().build())
    .expect("context should register");

  runtime
    .with_instances(|instances| {
      let mut registry = InstanceRegistry::new(&metadata, instances);
      let state_path = temp_path("state");
      let vars_path = temp_path("vars");
      registry.singleton_or_insert_with(FacetGraph::KEY, || {
        FacetGraph::from_persistence(StatePersistence::new(state_path))
      });
      registry.singleton_or_insert_with(VariableHeap::KEY, || VariableHeap::new(vars_path));
      registry.singleton_or_insert_with(SocketRegistry::KEY, SocketRegistry::default);
    })
    .expect("singletons should initialize");

  (runtime, metadata, Resources::default(), context_id)
}

fn flush(
  runtime: &RuntimeHandle,
  context_id: usize,
  metadata: &MetadataRegistry,
  resources: &mut Resources,
) {
  runtime
    .flush_context(context_id, metadata, resources)
    .expect("flush should succeed");
}

#[test]
fn test_natural_executor_basic() {
  let (runtime, mut metadata, mut resources, context_id) = setup_test_runtime();

  let mut units = Metadata::new("test").of::<Service>("service");

  let source = r#"
[[service]]
name = "simple"
run.exec = "/bin/sh"
run.args = ["-c", "exit 0"]
restart = false
"#;

  units.from_toml(source, "svc").unwrap();
  metadata.insert_metadata(units);
  metadata.ensure_index_for_type::<Service>("test").unwrap();

  runtime
    .dispatch("services", "bootstrap", Default::default(), context_id)
    .unwrap();
  flush(&runtime, context_id, &metadata, &mut resources);

  runtime
    .dispatch(
      "services",
      "start",
      rpayload!({"name": Ustr::from("svc:simple@test")}),
      context_id,
    )
    .unwrap();
  flush(&runtime, context_id, &metadata, &mut resources);

  std::thread::sleep(Duration::from_millis(800));

  runtime
    .dispatch("reaper", "reap_once", Default::default(), context_id)
    .unwrap();
  flush(&runtime, context_id, &metadata, &mut resources);
  flush(&runtime, context_id, &metadata, &mut resources);

  runtime
    .with_instances(|instances| {
      let registry = InstanceRegistry::new(&metadata, instances);
      let svc = registry
        .instances::<Service>("test", "svc:simple")
        .unwrap()
        .into_iter()
        .next()
        .unwrap();
      assert!(matches!(svc.last_state, ServiceState::Exited(0)));
    })
    .unwrap();
}

#[test]
fn test_socket_activation_inheritance() {
  let (runtime, mut metadata, mut resources, context_id) = setup_test_runtime();
  let tmp_file = temp_path("sock_count");

  let mut units = Metadata::new("test")
    .of::<Socket>("socket")
    .of::<Service>("service");

  let source = format!(
    r#"
[[socket]]
name = "web"
type = "tcp"
listen = "127.0.0.1:0"
owner = "svc:web"

[[service]]
name = "web"
run.exec = "/bin/sh"
run.args = ["-c", "echo $RIND_SOCKET_COUNT > {}"]
restart = false
"#,
    tmp_file.to_str().unwrap()
  );

  units
    .from_toml(source.split("[[service]]").next().unwrap(), "sock")
    .unwrap();
  units
    .from_toml(
      &format!(
        "[[service]]\n{}",
        source.split("[[service]]").nth(1).unwrap()
      ),
      "svc",
    )
    .unwrap();

  metadata.insert_metadata(units);
  metadata.ensure_index_for_type::<Socket>("test").unwrap();
  metadata.ensure_index_for_type::<Service>("test").unwrap();

  runtime
    .dispatch("sockets", "bootstrap", Default::default(), context_id)
    .unwrap();
  runtime
    .dispatch("services", "bootstrap", Default::default(), context_id)
    .unwrap();
  flush(&runtime, context_id, &metadata, &mut resources);

  runtime
    .dispatch(
      "sockets",
      "start",
      rpayload!({"name": Ustr::from("sock:web@test")}),
      context_id,
    )
    .unwrap();
  flush(&runtime, context_id, &metadata, &mut resources);

  runtime
    .dispatch(
      "services",
      "start",
      rpayload!({"name": Ustr::from("svc:web@test")}),
      context_id,
    )
    .unwrap();
  flush(&runtime, context_id, &metadata, &mut resources);

  std::thread::sleep(Duration::from_millis(800));

  let count = std::fs::read_to_string(&tmp_file).unwrap_or_default();
  assert_eq!(count.trim(), "1");
  let _ = std::fs::remove_file(tmp_file);
}

#[test]
fn test_service_watchdog_restart() {
  let (runtime, mut metadata, mut resources, context_id) = setup_test_runtime();

  let mut units = Metadata::new("static").of::<Service>("service");

  let source = r#"
[[service]]
name = "stalled"
run.exec = "/bin/sh"
run.args = ["-c", "sleep 10"]
watchdog.grace-ms = 100
watchdog.action = "restart"
restart = true
"#;

  units.from_toml(source, "svc").unwrap();
  metadata.insert_metadata(units);
  metadata.ensure_index_for_type::<Service>("static").unwrap();

  runtime
    .dispatch("services", "bootstrap", Default::default(), context_id)
    .unwrap();
  flush(&runtime, context_id, &metadata, &mut resources);

  runtime
    .dispatch(
      "services",
      "start",
      rpayload!({"name": Ustr::from("svc:stalled")}),
      context_id,
    )
    .unwrap();
  flush(&runtime, context_id, &metadata, &mut resources);

  std::thread::sleep(Duration::from_millis(800));

  let unwatched = resources.unwatched_fds();
  for fd in unwatched {
    if let Some(act) = resources.get_action(fd) {
      if act.action == Ustr::from("watchdog_expired") {
        let payload = RuntimePayload::default().insert("fd", fd);
        runtime
          .dispatch(
            &act.runtime,
            &act.action,
            if let Some(p) = &act.payload {
              p(payload)
            } else {
              payload
            },
            context_id,
          )
          .unwrap();
      }
    }
  }

  flush(&runtime, context_id, &metadata, &mut resources);
  flush(&runtime, context_id, &metadata, &mut resources);
  flush(&runtime, context_id, &metadata, &mut resources);

  runtime
    .with_instances(|instances| {
      let registry = InstanceRegistry::new(&metadata, instances);
      let svc = registry
        .instances::<Service>("static", "svc:stalled")
        .unwrap()
        .into_iter()
        .next()
        .unwrap();
      assert!(svc.instances.is_active());
    })
    .unwrap();
}

// #[test]
// fn test_dynamic_units_loading() {
//   let (runtime, mut metadata, mut resources, context_id) = setup_test_runtime();
//   let tmp_dir = temp_path("dunits");
//   std::fs::create_dir_all(&tmp_dir).unwrap();

//   let source = r#"
// [[service]]
// name = "dynamic_svc"
// run.exec = "/bin/sh"
// run.args = ["-c", "exit 0"]
// restart = false
// "#;

//   std::fs::write(tmp_dir.join("dynamic.toml"), source).unwrap();

//   rind_cfg::dunits::create_scope_metadata_runtime("dynamic", &mut metadata, &tmp_dir).unwrap();

//   runtime
//     .dispatch("services", "bootstrap", Default::default(), context_id)
//     .unwrap();
//   flush(&runtime, context_id, &metadata, &mut resources);

//   runtime
//     .dispatch(
//       "services",
//       "start",
//       rpayload!({"name": Ustr::from("dynamic:dynamic_svc@dynamic")}),
//       context_id,
//     )
//     .unwrap();
//   flush(&runtime, context_id, &metadata, &mut resources);

//   std::thread::sleep(Duration::from_millis(800));

//   runtime
//     .dispatch("reaper", "reap_once", Default::default(), context_id)
//     .unwrap();
//   flush(&runtime, context_id, &metadata, &mut resources);
//   flush(&runtime, context_id, &metadata, &mut resources);

//   runtime
//     .with_instances(|instances| {
//       let registry = InstanceRegistry::new(&metadata, instances);
//       let svc = registry
//         .instances::<Service>("dynamic", "dynamic:dynamic_svc")
//         .unwrap()
//         .into_iter()
//         .next()
//         .unwrap();
//       assert!(matches!(svc.last_state, ServiceState::Exited(0)));
//     })
//     .unwrap();

//   std::fs::remove_dir_all(tmp_dir).unwrap();
// }

#[test]
fn test_service_transport_stdio() {
  let (runtime, mut metadata, mut resources, context_id) = setup_test_runtime();

  let mut units = Metadata::new("test").of::<Service>("service");

  let source = r#"
[[service]]
name = "talkative"
run.exec = "/bin/sh"
run.args = ["-c", "echo 'HELLO RIND'"]
transport = "stdio"
restart = false
"#;

  units.from_toml(source, "svc").unwrap();
  metadata.insert_metadata(units);
  metadata.ensure_index_for_type::<Service>("test").unwrap();

  runtime
    .dispatch("services", "bootstrap", Default::default(), context_id)
    .unwrap();
  flush(&runtime, context_id, &metadata, &mut resources);

  runtime
    .dispatch(
      "services",
      "start",
      rpayload!({"name": Ustr::from("svc:talkative@test")}),
      context_id,
    )
    .unwrap();
  flush(&runtime, context_id, &metadata, &mut resources);

  std::thread::sleep(Duration::from_millis(800));

  runtime
    .dispatch("reaper", "reap_once", Default::default(), context_id)
    .unwrap();
  flush(&runtime, context_id, &metadata, &mut resources);
  flush(&runtime, context_id, &metadata, &mut resources);

  runtime
    .with_instances(|instances| {
      let registry = InstanceRegistry::new(&metadata, instances);
      let svc = registry
        .instances::<Service>("test", "svc:talkative")
        .unwrap()
        .into_iter()
        .next()
        .unwrap();
      assert!(matches!(svc.last_state, ServiceState::Exited(0)));
    })
    .unwrap();
}

#[test]
fn test_user_service_resolution() {
  let (runtime, mut metadata, mut resources, context_id) = setup_test_runtime();

  let mut units = Metadata::new("test")
    .of::<Service>("service")
    .of::<FlowFacet>("facet");

  let source = r#"
[[facet]]
name = "user_session"
payload = "json"
branch = ["session_id"]

[[service]]
name = "user_worker"
space = "user"
user-source.facet = "test:user_session"
user-source.username-field = "user"
run.exec = "/bin/sh"
run.args = ["-c", "exit 0"]
restart = false
"#;

  units.from_toml(source, "svc").unwrap();
  metadata.insert_metadata(units);
  metadata.ensure_index_for_type::<Service>("test").unwrap();
  metadata.ensure_index_for_type::<FlowFacet>("test").unwrap();

  runtime
    .dispatch("services", "bootstrap", Default::default(), context_id)
    .unwrap();
  runtime
    .dispatch("flow", "bootstrap", Default::default(), context_id)
    .unwrap();
  flush(&runtime, context_id, &metadata, &mut resources);

  runtime
    .dispatch(
      "flow",
      "set_facet",
      FlowRuntimePayload::new("test:user_session")
        .payload(serde_json::json!({"session_id": "s1", "user": "nonexistent_user_xyz"}))
        .into(),
      context_id,
    )
    .unwrap();
  flush(&runtime, context_id, &metadata, &mut resources);

  runtime
    .dispatch(
      "services",
      "start",
      rpayload!({"name": Ustr::from("svc:user_worker@test")}),
      context_id,
    )
    .unwrap();
  flush(&runtime, context_id, &metadata, &mut resources);

  runtime
    .with_instances(|instances| {
      let registry = InstanceRegistry::new(&metadata, instances);
      let svc = registry
        .instances::<Service>("test", "svc:user_worker")
        .unwrap()
        .into_iter()
        .next()
        .unwrap();

      if let ServiceState::Error(err) = &svc.last_state {
        assert!(err.contains("nonexistent_user_xyz"));
      } else {
        panic!("Service should be in Error state, got {:?}", svc.last_state);
      }
    })
    .unwrap();
}
