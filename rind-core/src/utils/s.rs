use serde::{Deserialize, Deserializer};
use strumbra::SharedString;

pub fn s(s: &str) -> SharedString {
  SharedString::try_from(s).unwrap()
}

pub fn de_arcstr<'de, D: Deserializer<'de>>(deserializer: D) -> Result<SharedString, D::Error> {
  let s: &str = Deserialize::deserialize(deserializer)?;
  Ok(SharedString::try_from(s).unwrap())
}
