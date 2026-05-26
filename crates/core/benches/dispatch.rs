use criterion::{Criterion, criterion_group, criterion_main};
use rind_core::context::RuntimeContext;
use rind_core::logging::LogHandle;
use rind_core::prelude::Resources;
use rind_core::registry::MetadataRegistry;
use rind_core::runtime::{Runtime, RuntimeDispatcher, RuntimePayload, start_runtime};

struct BenchRuntime;
impl Runtime for BenchRuntime {
  fn id(&self) -> &str {
    "bench"
  }
  fn handle(
    &mut self,
    _action: &str,
    _payload: RuntimePayload,
    _ctx: &mut RuntimeContext<'_>,
    _dispatch: &RuntimeDispatcher,
    _log: &LogHandle,
  ) -> Result<Option<RuntimePayload>, rind_core::error::CoreError> {
    Ok(None)
  }
}

fn bench_dispatch(c: &mut Criterion) {
  let log = LogHandle::mock();
  let runtime = Box::new(BenchRuntime);
  let handle = start_runtime(log.clone(), vec![runtime], None);
  let metadata = MetadataRegistry::default();
  let mut resources = Resources::default();
  let context_id = 1;

  handle
    .register_scopes(context_id, Default::default())
    .unwrap();

  c.bench_function("runtime_dispatch_and_flush", |b| {
    b.iter(|| {
      handle
        .dispatch("bench", "nop", RuntimePayload::default(), context_id)
        .unwrap();
      handle
        .flush_context(context_id, &metadata, &mut resources)
        .unwrap();
    })
  });
}

fn bench_payload(c: &mut Criterion) {
  c.bench_function("payload_insert_get", |b| {
    b.iter(|| {
      let mut payload = RuntimePayload::default()
        .insert("key1", 100u32)
        .insert("key2", "value".to_string());

      let _: u32 = payload.get("key1").unwrap();
      let _: String = payload.get("key2").unwrap();
    })
  });
}

criterion_group!(benches, bench_dispatch, bench_payload);
criterion_main!(benches);
