use rind_core::types::Ustr;

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
pub enum SeatPayload {
  Check,
  Take(Ustr),
  Return(Ustr),
  Taken(Vec<Ustr>),
  List,
  Activate(Ustr),
  Session {
    seat: Ustr,
    session: Ustr,
    user: String,
  },
  SessionEnd {
    seat: Ustr,
    session: Ustr,
  },
  Devices(Ustr),
}

impl SeatPayload {
  pub fn taken(&self) -> &[Ustr] {
    match self {
      SeatPayload::Taken(taken) => taken,
      _ => &[],
    }
  }
}

#[derive(Clone)]
pub struct TTYEvent {
  pub tty: Ustr,
  pub from: Ustr,
}
