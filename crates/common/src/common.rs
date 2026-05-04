use rind_plugins::prelude::Ustr;

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
pub enum TTYPayload {
  Check,
  Take(Ustr),
  Return(Ustr),
  Taken(Vec<Ustr>),
}

impl TTYPayload {
  pub fn taken(&self) -> &[Ustr] {
    match self {
      TTYPayload::Taken(taken) => taken,
      _ => &[],
    }
  }
}
