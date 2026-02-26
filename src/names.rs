use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

#[derive(Clone)]
pub struct Name {
  hash: u64,
  string: Arc<str>,
}

impl Name {
  pub fn new<S: AsRef<str>>(s: S) -> Self {
    let arc: Arc<str> = Arc::from(s.as_ref());

    let mut hasher = DefaultHasher::new();
    arc.hash(&mut hasher);

    Self {
      hash: hasher.finish(),
      string: arc,
    }
  }

  pub fn to_string(&self) -> String {
    self.string.to_string()
  }
}

impl From<String> for Name {
  fn from(value: String) -> Self {
    Self::new(value.as_str())
  }
}

impl From<&str> for Name {
  fn from(value: &str) -> Self {
    Self::new(value)
  }
}

impl PartialEq for Name {
  fn eq(&self, other: &Self) -> bool {
    Arc::ptr_eq(&self.string, &other.string) || self.string == other.string
  }
}
impl Eq for Name {}

impl Hash for Name {
  fn hash<H: Hasher>(&self, state: &mut H) {
    state.write_u64(self.hash);
  }
}
