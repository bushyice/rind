use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use serde::Deserialize;
use serde::de::Deserializer;

#[derive(Clone, serde::Serialize)]
pub struct Name {
  hash: u64,
  #[serde(serialize_with = "ser_name", deserialize_with = "de_name")]
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

fn ser_name<S: serde::Serializer>(f: &Arc<str>, serializer: S) -> Result<S::Ok, S::Error> {
  serializer.collect_str(&f.to_string())
}

fn de_name<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Name, D::Error> {
  let s: &str = Deserialize::deserialize(deserializer)?;
  Ok(Name::new(s))
}
