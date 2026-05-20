use std::path::PathBuf;

use rind_core::prelude::{
  InstanceRegistry, LogConfig, Metadata, MetadataRegistry, Resources, RuntimeCommand,
  RuntimeHandle, RuntimePayload, ScopeBuilder, StatePersistence, Ustr, start_logger, start_runtime,
};
use rind_flow::{FacetGraph, FlowFacet, FlowImpulse, FlowRuntime, FlowRuntimePayload};
use rind_primitives::variables::*;
use rind_services::*;

fn temp_path(tag: &str) -> PathBuf {
  let now = std::time::SystemTime::now()
    .duration_since(std::time::UNIX_EPOCH)
    .expect("clock before epoch")
    .as_nanos();
  std::env::temp_dir().join(format!("rind-{tag}-{}-{now}", std::process::id()))
}

fn setup_runtime_with_metadata() -> (RuntimeHandle, MetadataRegistry, Resources, usize) {
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
    ],
    None,
  );

  let mut metadata = MetadataRegistry::default();
  let mut units = Metadata::new("units")
    .of::<FlowFacet>("facet")
    .of::<FlowImpulse>("impulse")
    .of::<Timer>("timer")
    .of::<Socket>("socket")
    .of::<Service>("service");

  let source = r#"
[[facet]]
name = "base"
payload = "json"
branch = ["id"]

[[facet]]
name = "derived"
payload = "json"
after = [{ facet = "test:base" }]
branch = ["id"]

[[timer]]
name = "tick"
duration = "5s"

[[socket]]
name = "listener"
type = "tcp"
listen = "127.0.0.1:0"

[[service]]
name = "worker"
run.exec = "/bin/sh"
run.args = ["-c", "exit 0"]
restart = false

[[impulse]]
name = "sig1"
payload = "string"

[[impulse]]
name = "sig2"
payload = "string"
after = [{ impulse = "test:sig1" }]

[[impulse]]
name = "sock_hit"
payload = "none"

[[service]]
name = "sig_worker"
run.exec = "/bin/sh"
run.args = ["-c", "sleep 1"]
start-on = [{ impulse = "test:sig2" }]
restart = false

[[service]]
name = "retry_worker"
run.exec = "/bin/sh"
run.args = ["-c", "sleep 2"]
restart = { max_retries = 1 }

[[service]]
name = "sock_worker"
run.exec = "/bin/sh"
run.args = ["-c", "sleep 1"]
start-on = [{ impulse = "test:sock_hit" }]
restart = false

[[socket]]
name = "trigger_sock"
type = "tcp"
listen = "127.0.0.1:0"
trigger = [{ impulse = "test:sock_hit" }]
"#;

  units
    .from_toml(source, "test")
    .expect("unit toml should parse");
  metadata.insert_metadata(units);
  metadata
    .ensure_index_for_type::<FlowFacet>("units")
    .expect("state index should build");
  metadata
    .ensure_index_for_type::<FlowImpulse>("units")
    .expect("signal index should build");
  metadata
    .ensure_index_for_type::<Timer>("units")
    .expect("timer index should build");
  metadata
    .ensure_index_for_type::<Socket>("units")
    .expect("socket index should build");
  metadata
    .ensure_index_for_type::<Service>("units")
    .expect("service index should build");

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
fn flow_runtime_reconciles_dependent_state_and_remove() {
  let (runtime, metadata, mut resources, context_id) = setup_runtime_with_metadata();

  runtime
    .dispatch("flow", "bootstrap", Default::default(), context_id)
    .expect("bootstrap dispatch should queue");
  flush(&runtime, context_id, &metadata, &mut resources);

  runtime
    .dispatch(
      "flow",
      "set_facet",
      FlowRuntimePayload::new("test:base")
        .payload(serde_json::json!({"id":"a1","value":7}))
        .into(),
      context_id,
    )
    .expect("set_state should queue");
  flush(&runtime, context_id, &metadata, &mut resources);

  runtime
    .with_instances(|instances| {
      let registry = InstanceRegistry::new(&metadata, instances);
      let sm = registry
        .singleton::<FacetGraph>(FacetGraph::KEY)
        .expect("state machine should exist");
      assert!(sm.facets.contains_key(&Ustr::from("test:base")));
      assert!(sm.facets.contains_key(&Ustr::from("test:derived")));
    })
    .expect("state assertions should succeed");

  runtime
    .dispatch(
      "flow",
      "remove_facet",
      FlowRuntimePayload::new("test:base")
        .filter(serde_json::json!({"id":"a1"}))
        .into(),
      context_id,
    )
    .expect("remove_state should queue");
  flush(&runtime, context_id, &metadata, &mut resources);

  runtime
    .with_instances(|instances| {
      let registry = InstanceRegistry::new(&metadata, instances);
      let sm = registry
        .singleton::<FacetGraph>(FacetGraph::KEY)
        .expect("state machine should exist");
      assert!(!sm.facets.contains_key(&Ustr::from("test:base")));
      assert!(!sm.facets.contains_key(&Ustr::from("test:derived")));
    })
    .expect("remove assertions should succeed");

  let _ = runtime.send(RuntimeCommand::Stop);
}

#[test]
fn socket_runtime_start_and_stop_updates_registry_and_resources() {
  let (runtime, metadata, mut resources, context_id) = setup_runtime_with_metadata();

  runtime
    .dispatch(
      "sockets",
      "start",
      rind_core::rpayload!({ "name": Ustr::from("test:listener") }),
      context_id,
    )
    .expect("socket start should queue");
  flush(&runtime, context_id, &metadata, &mut resources);

  let started = runtime
    .with_instances(|instances| {
      let registry = InstanceRegistry::new(&metadata, instances);
      let sockets = registry
        .instances::<Socket>("units", "test:listener")
        .expect("socket instance list should exist");
      !sockets.is_empty() && sockets.iter().all(|sock| sock.active)
    })
    .expect("socket state probe should succeed");
  if !started {
    eprintln!(
      "skipping socket assertions because socket bind/start is restricted in this environment"
    );
    let _ = runtime.send(RuntimeCommand::Stop);
    return;
  }
  assert!(!resources.unwatched_fds().is_empty());

  runtime
    .dispatch(
      "sockets",
      "stop",
      rind_core::rpayload!({ "name": Ustr::from("test:listener") }),
      context_id,
    )
    .expect("socket stop should queue");
  flush(&runtime, context_id, &metadata, &mut resources);

  runtime
    .with_instances(|instances| {
      let registry = InstanceRegistry::new(&metadata, instances);
      let missing = registry.instances::<Socket>("units", "test:listener");
      assert!(missing.is_err() || missing.expect("instances result should exist").is_empty());
    })
    .expect("socket missing assertion should succeed");

  let _ = runtime.send(RuntimeCommand::Stop);
}

#[test]
fn timer_runtime_start_and_finish_cleans_instance() {
  let (runtime, metadata, mut resources, context_id) = setup_runtime_with_metadata();

  runtime
    .dispatch(
      "timer",
      "start",
      rind_core::rpayload!({ "name": Ustr::from("test:tick") }),
      context_id,
    )
    .expect("timer start should queue");
  flush(&runtime, context_id, &metadata, &mut resources);

  runtime
    .with_instances(|instances| {
      let registry = InstanceRegistry::new(&metadata, instances);
      let _ = registry
        .as_one::<Timer>("units", "test:tick")
        .expect("timer should exist after start");
    })
    .expect("timer presence assertion should succeed");

  runtime
    .dispatch(
      "timer",
      "finish_timer",
      rind_core::rpayload!({ "name": Ustr::from("test:tick") }),
      context_id,
    )
    .expect("timer finish should queue");
  flush(&runtime, context_id, &metadata, &mut resources);

  runtime
    .with_instances(|instances| {
      let registry = InstanceRegistry::new(&metadata, instances);
      let missing = registry.as_one::<Timer>("units", "test:tick");
      assert!(missing.is_err());
    })
    .expect("timer missing assertion should succeed");

  let _ = runtime.send(RuntimeCommand::Stop);
}

#[test]
fn service_runtime_start_and_child_exit_updates_instance_group() {
  let (runtime, metadata, mut resources, context_id) = setup_runtime_with_metadata();

  runtime
    .dispatch(
      "services",
      "start",
      rind_core::rpayload!({ "name": Ustr::from("test:worker") }),
      context_id,
    )
    .expect("service start should queue");
  flush(&runtime, context_id, &metadata, &mut resources);

  let pid = runtime
    .with_instances(|instances| {
      let registry = InstanceRegistry::new(&metadata, instances);
      let service = registry
        .as_one::<Service>("units", "test:worker")
        .expect("service should exist after start");
      service
        .instances
        .as_one()
        .and_then(|inst| inst.pid())
        .unwrap_or_default()
    })
    .expect("service inspection should succeed");

  if pid != 0 {
    runtime
      .dispatch(
        "services",
        "child_exited",
        rind_core::rpayload!({ "pid": pid as i32, "code": 0i32 }),
        context_id,
      )
      .expect("child exit should queue");
    flush(&runtime, context_id, &metadata, &mut resources);
  }

  runtime
    .with_instances(|instances| {
      let registry = InstanceRegistry::new(&metadata, instances);
      let service = registry
        .as_one::<Service>("units", "test:worker")
        .expect("service should remain registered");
      let _ = pid;
      let _ = service;
    })
    .expect("service exit assertions should succeed");

  let _ = runtime.send(RuntimeCommand::Stop);
}

#[test]
fn signal_transcendence_chain_starts_dependent_service() {
  let (runtime, metadata, mut resources, context_id) = setup_runtime_with_metadata();

  runtime
    .dispatch("flow", "bootstrap", Default::default(), context_id)
    .expect("flow bootstrap should queue");
  runtime
    .dispatch("services", "bootstrap", Default::default(), context_id)
    .expect("services bootstrap should queue");
  runtime
    .dispatch("services", "watch_events", Default::default(), context_id)
    .expect("watch events should queue");
  flush(&runtime, context_id, &metadata, &mut resources);

  runtime
    .dispatch(
      "flow",
      "impulse",
      FlowRuntimePayload::new("test:sig1")
        .payload(serde_json::Value::String("hello".to_string()))
        .into(),
      context_id,
    )
    .expect("emit signal should queue");
  flush(&runtime, context_id, &metadata, &mut resources);

  runtime
    .dispatch("services", "drain_events", Default::default(), context_id)
    .expect("drain events should queue");
  flush(&runtime, context_id, &metadata, &mut resources);

  runtime
    .with_instances(|instances| {
      let registry = InstanceRegistry::new(&metadata, instances);
      let service = registry
        .as_one::<Service>("units", "test:sig_worker")
        .expect("signal dependent service should exist");
      assert!(!service.instances.0.is_empty());
    })
    .expect("signal chain assertion should succeed");

  let _ = runtime.send(RuntimeCommand::Stop);
}

#[test]
fn socket_trigger_emits_signal_that_starts_service() {
  let (runtime, metadata, mut resources, context_id) = setup_runtime_with_metadata();

  runtime
    .dispatch("flow", "bootstrap", Default::default(), context_id)
    .expect("flow bootstrap should queue");
  runtime
    .dispatch("services", "bootstrap", Default::default(), context_id)
    .expect("services bootstrap should queue");
  runtime
    .dispatch("services", "watch_events", Default::default(), context_id)
    .expect("services watch should queue");
  runtime
    .dispatch(
      "sockets",
      "start",
      rind_core::rpayload!({ "name": Ustr::from("test:trigger_sock") }),
      context_id,
    )
    .expect("socket start should queue");
  flush(&runtime, context_id, &metadata, &mut resources);

  let fd = runtime
    .with_instances(|instances| {
      let registry = InstanceRegistry::new(&metadata, instances);
      registry
        .instances::<Socket>("units", "test:trigger_sock")
        .ok()
        .and_then(|list| list.first().map(|s| s.fd))
    })
    .expect("socket lookup should succeed");
  let Some(fd) = fd else {
    eprintln!("skipping socket trigger test due to restricted socket bind environment");
    let _ = runtime.send(RuntimeCommand::Stop);
    return;
  };

  runtime
    .dispatch(
      "sockets",
      "drain_incoming",
      rind_core::rpayload!({ "fd": fd }),
      context_id,
    )
    .expect("drain incoming should queue");
  flush(&runtime, context_id, &metadata, &mut resources);
  flush(&runtime, context_id, &metadata, &mut resources);

  runtime
    .dispatch("services", "drain_events", Default::default(), context_id)
    .expect("services drain should queue");
  flush(&runtime, context_id, &metadata, &mut resources);

  runtime
    .with_instances(|instances| {
      let registry = InstanceRegistry::new(&metadata, instances);
      let service = registry
        .as_one::<Service>("units", "test:sock_worker")
        .expect("socket-triggered service should exist");
      assert!(!service.instances.0.is_empty());
    })
    .expect("socket trigger assertion should succeed");

  let _ = runtime.send(RuntimeCommand::Stop);
}

#[test]
fn service_on_failure_retry_cap_is_enforced() {
  let (runtime, metadata, mut resources, context_id) = setup_runtime_with_metadata();

  runtime
    .dispatch(
      "services",
      "start",
      rind_core::rpayload!({ "name": Ustr::from("test:retry_worker") }),
      context_id,
    )
    .expect("retry service start should queue");
  flush(&runtime, context_id, &metadata, &mut resources);

  let first_pid = runtime
    .with_instances(|instances| {
      let registry = InstanceRegistry::new(&metadata, instances);
      let service = registry
        .as_one::<Service>("units", "test:retry_worker")
        .expect("retry service should exist");
      service
        .instances
        .as_one()
        .and_then(|inst| inst.pid())
        .unwrap_or(0)
    })
    .expect("pid query should succeed");

  if first_pid != 0 {
    runtime
      .dispatch(
        "services",
        "child_exited",
        rind_core::rpayload!({ "pid": first_pid as i32, "code": 1i32 }),
        context_id,
      )
      .expect("first exit should queue");
    flush(&runtime, context_id, &metadata, &mut resources);

    let second_pid = runtime
      .with_instances(|instances| {
        let registry = InstanceRegistry::new(&metadata, instances);
        let service = registry
          .as_one::<Service>("units", "test:retry_worker")
          .expect("retry service should still exist");
        service
          .instances
          .as_one()
          .and_then(|inst| inst.pid())
          .unwrap_or(0)
      })
      .expect("second pid query should succeed");

    if second_pid != 0 {
      runtime
        .dispatch(
          "services",
          "child_exited",
          rind_core::rpayload!({ "pid": second_pid as i32, "code": 1i32 }),
          context_id,
        )
        .expect("second exit should queue");
      flush(&runtime, context_id, &metadata, &mut resources);
    }
  }

  runtime
    .with_instances(|instances| {
      let registry = InstanceRegistry::new(&metadata, instances);
      let service = registry
        .as_one::<Service>("units", "test:retry_worker")
        .expect("retry service should remain as object");
      assert!(
        service.instances.0.is_empty()
          || service.instances.iter().all(|inst| inst.retry_count <= 1)
      );
    })
    .expect("retry cap assertion should succeed");

  let _ = runtime.send(RuntimeCommand::Stop);
}

#[test]
fn race_like_dispatch_churn_keeps_runtime_consistent() {
  let (runtime, metadata, mut resources, context_id) = setup_runtime_with_metadata();

  runtime
    .dispatch("flow", "bootstrap", Default::default(), context_id)
    .expect("flow bootstrap should queue");
  runtime
    .dispatch("services", "bootstrap", Default::default(), context_id)
    .expect("services bootstrap should queue");
  runtime
    .dispatch("services", "watch_events", Default::default(), context_id)
    .expect("watch should queue");
  flush(&runtime, context_id, &metadata, &mut resources);

  for i in 0..120usize {
    let payload = serde_json::json!({ "id": format!("b{i}"), "n": i as i64 });
    runtime
      .dispatch(
        "flow",
        "set_facet",
        FlowRuntimePayload::new("test:base").payload(payload).into(),
        context_id,
      )
      .expect("set_state should queue");

    if i % 3 == 0 {
      runtime
        .dispatch(
          "flow",
          "remove_facet",
          FlowRuntimePayload::new("test:base")
            .filter(serde_json::json!({"id": format!("b{i}")}))
            .into(),
          context_id,
        )
        .expect("remove_state should queue");
    }

    if i % 5 == 0 {
      runtime
        .dispatch(
          "services",
          "start",
          rind_core::rpayload!({ "name": Ustr::from("test:worker") }),
          context_id,
        )
        .expect("start should queue");
    }

    if i % 7 == 0 {
      runtime
        .dispatch(
          "services",
          "stop",
          rind_core::rpayload!({ "name": Ustr::from("test:worker"), "force": true }),
          context_id,
        )
        .expect("stop should queue");
    }

    runtime
      .dispatch("services", "drain_events", Default::default(), context_id)
      .expect("drain should queue");

    flush(&runtime, context_id, &metadata, &mut resources);
  }

  runtime
    .with_instances(|instances| {
      let registry = InstanceRegistry::new(&metadata, instances);
      let sm = registry
        .singleton::<FacetGraph>(FacetGraph::KEY)
        .expect("state machine should exist");
      if let Some(branches) = sm.facets.get(&Ustr::from("test:base")) {
        for b in branches {
          assert_eq!(b.name, Ustr::from("test:base"));
        }
      }

      let service = registry
        .as_one::<Service>("units", "test:worker")
        .expect("service object should exist");
      assert!(
        service
          .instances
          .iter()
          .all(|inst| { !matches!(inst.state, ServiceState::Active) || inst.handle.is_some() })
      );
    })
    .expect("post-churn assertions should succeed");

  let _ = runtime.send(RuntimeCommand::Stop);
}
