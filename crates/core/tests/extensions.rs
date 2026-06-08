use rind_core::extensions::{Extension, ExtensionExecutionCtx, ExtensionManager};
use rind_core::types::Void;

fn enquire_len(name: &str) -> rind_core::error::CoreResult<usize> {
  Ok(name.len())
}

fn act_push(_name: &str, input: &mut Vec<String>) -> rind_core::error::CoreResult<Void> {
  input.push("acted".to_string());
  Ok(Void)
}

fn resolve_suffix(_name: &str, input: String) -> rind_core::error::CoreResult<String> {
  Ok(format!("{input}-resolved"))
}

#[test]
fn manager_executes_enquire_act_and_resolve_extensions() {
  let mut mgr = ExtensionManager::default();
  mgr.register::<usize>(Extension::Enquire(enquire_len));
  mgr.register::<Vec<String>>(Extension::Act(act_push));
  mgr.register::<String>(Extension::Resolve(resolve_suffix));

  let results = mgr
    .enquire::<usize>("abcd")
    .expect("enquire should return result");
  assert_eq!(results, vec![4usize]);

  let mut target = Vec::<String>::new();
  mgr
    .act::<Vec<String>>("anything", &mut target)
    .expect("act should mutate target");
  assert_eq!(target, vec!["acted".to_string()]);

  let resolved = mgr
    .resolve::<String>("name", "value".to_string())
    .expect("resolve should transform");
  assert_eq!(resolved, "value-resolved".to_string());
}

#[test]
fn execution_ctx_with_fn_dispatches_custom_response() {
  let out = ExtensionExecutionCtx::new(())
    .with_fn(|_, _, _| Ok(Box::new(11u32)))
    .dispatch(None, None, None)
    .expect("dispatch should run function response");
  let value = out
    .downcast_ref::<u32>()
    .expect("result type should be u32");
  assert_eq!(*value, 11);
}
