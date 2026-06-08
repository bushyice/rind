use rind_core::prelude::rslvns;

#[test]
fn resolve_namespace() {
  assert_eq!(rslvns!("a", "b"), "a:b");
  assert_eq!(rslvns!("u", "a", "b"), "u:a:b");
  assert_eq!(rslvns!(norm "a:b@static"), "a:b");
  assert_eq!(rslvns!(norm "@s" "a:b@s"), "a:b");
  assert_eq!(rslvns!(res "a:b"), ("a", "b", "static"));
  assert_eq!(rslvns!(res "a:b@s"), ("a", "b", "s"));
}
