use rind_core::prelude::{RuntimeScope, ScopeBuilder};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Debug, Default, PartialEq, Eq)]
struct MyType {
  value: usize,
}

#[test]
fn scope_builder_inserts_per_runtime() {
  let mut builder = ScopeBuilder::default();
  builder.insert("svc", || MyType { value: 42 });

  let scopes = builder.build();
  let value = scopes
    .scope("svc")
    .and_then(|scope| scope.get::<MyType>())
    .expect("typed runtime value should exist");

  assert_eq!(*value, MyType { value: 42 });
  assert!(scopes.scope("missing").is_none());
}

#[test]
fn scope_builder_inserts_runtime_scope_values() {
  let mut builder = ScopeBuilder::default();
  builder.insert_scope("svc", || {
    let mut scope = RuntimeScope::default();
    scope.insert(MyType { value: 7 });
    scope
  });

  let scopes = builder.build();
  let value = scopes
    .scope("svc")
    .and_then(|scope| scope.get::<MyType>())
    .expect("typed runtime value from runtime scope should exist");

  assert_eq!(*value, MyType { value: 7 });
}

#[test]
fn take_or_build_scope_applies_globals_once_per_runtime() {
  let mut builder = ScopeBuilder::default();
  let counter = Arc::new(AtomicUsize::new(0));
  let counter_clone = counter.clone();
  builder.globals(move |_scope| {
    counter_clone.fetch_add(1, Ordering::SeqCst);
  });

  let mut scopes = builder.build();
  let scope = scopes.take_or_build_scope("svc");
  scopes.put_scope("svc", scope);
  let scope = scopes.take_or_build_scope("svc");
  scopes.put_scope("svc", scope);

  assert_eq!(counter.load(Ordering::SeqCst), 1);
}
