use serde::{Deserialize, Deserializer};
use strumbra::SharedString;

pub fn s(s: &str) -> SharedString {
  SharedString::try_from(s).unwrap()
}

pub fn de_arcstr<'de, D: Deserializer<'de>>(deserializer: D) -> Result<SharedString, D::Error> {
  let s: &str = Deserialize::deserialize(deserializer)?;
  SharedString::try_from(s).map_err(serde::de::Error::custom)
}

#[cfg(test)]
mod tests {
  use super::s;
  use serde::Deserialize;

  #[derive(Deserialize)]
  struct Wrapped {
    #[serde(deserialize_with = "super::de_arcstr")]
    value: strumbra::SharedString,
  }

  #[test]
  fn shared_string_helpers() {
    let v = s("abc");
    assert_eq!(v.to_string(), "abc".to_string());

    let decoded: Wrapped = serde_json::from_str(r#"{"value":"xyz"}"#).unwrap();
    assert_eq!(decoded.value.to_string(), "xyz".to_string());
  }
}
