use rind_core::{
  error::{CoreError, CoreResult},
  reexports::{bincode_next, once_cell::sync::Lazy},
  types::Ustr,
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};

#[derive(Serialize, Deserialize, Default)]
pub struct UnitSerialized {
  pub name: Ustr,
  pub services: usize,
  pub active_services: usize,
  pub mounts: usize,
  pub mounted: usize,
  pub sockets: usize,
  pub active_sockets: usize,
  pub facets: usize,
  pub active_facets: usize,
  pub impulses: usize,
}

impl UnitSerialized {
  pub fn from_bytes(data: &[u8]) -> Self {
    deser_from_vec(data, false).unwrap_or(Self {
      name: String::new().into(),
      ..Default::default()
    })
  }

  pub fn many_from_bytes(data: &[u8]) -> Vec<Self> {
    deser_from_vec(data, false).unwrap_or_default()
  }

  pub fn as_some(self) -> Option<Self> {
    Some(self)
  }
}

pub fn serialize_many<T: Serialize>(items: &Vec<T>) -> Vec<u8> {
  ser_to_vec(items, false)
}

#[derive(Serialize, Deserialize)]
pub struct ServiceSerialized {
  pub name: Ustr,
  pub description: Option<String>,
  pub last_state: String,
  pub after: Option<Vec<Ustr>>,
  pub restart: bool,
  pub run: Vec<Ustr>,
  pub pid: Option<u32>,
}

#[derive(Serialize, Deserialize)]
pub struct SocketSerialized {
  pub name: Ustr,
  pub listen: String,
  pub r#type: Ustr,
  pub triggers: usize,
  pub active: bool,
}

#[derive(Serialize, Deserialize)]
pub struct FacetSerialized {
  pub name: Ustr,
  pub instances: Vec<Vec<u8>>,
  pub keys: Vec<Ustr>,
}

#[derive(Serialize, Deserialize)]
pub struct ImpulseSerialized {
  pub name: Ustr,
}

#[derive(Serialize, Deserialize)]
pub struct MountSerialized {
  pub source: Option<Ustr>,
  pub target: Ustr,
  pub fstype: Option<Ustr>,
  pub mounted: bool,
}

#[derive(Serialize, Deserialize, Default)]
pub struct UnitItemsSerialized {
  pub description: Option<String>,
  pub mounts: Vec<MountSerialized>,
  pub services: Vec<ServiceSerialized>,
  pub sockets: Vec<SocketSerialized>,
  pub facets: Vec<FacetSerialized>,
  pub impulses: Vec<ImpulseSerialized>,
}

#[derive(Serialize, Deserialize)]
pub struct PermissionSerialized {
  pub name: Ustr,
  pub id: u16,
  pub group: Option<Ustr>,
}

pub trait SerializeSerialized {
  fn serialize(&self) -> Vec<u8>;
}

impl<T: Serialize> SerializeSerialized for T {
  fn serialize(&self) -> Vec<u8> {
    ser_to_vec(self, false)
  }
}

#[derive(Debug, Serialize, Deserialize, Default, Clone)]
pub struct IpcListPrinter {
  pub r#type: String, // table/list/string
  pub titles: Vec<String>,
  pub keys: Vec<String>,
  pub colors: Vec<String>,
}

#[derive(Default, Serialize, Deserialize)]
pub struct IpcListComponent {
  pub components: Vec<Vec<u8>>,
  pub printer: Option<IpcListPrinter>,
}

impl IpcListComponent {
  pub fn add(&mut self, item: impl SerializeSerialized) {
    self.components.push(item.serialize());
  }

  pub fn with_printer(mut self, printer: IpcListPrinter) -> Self {
    self.printer = Some(printer);
    self
  }
}

#[derive(Serialize, Deserialize)]
pub struct VariableSerialized {
  pub name: Ustr,
  pub default: String,
  pub value: String,
}

static BINCODE_CONFIG: Lazy<bincode_next::config::Configuration> =
  Lazy::new(|| bincode_next::config::standard());

pub fn ser_to_vec<T: Serialize>(item: T, bincode: bool) -> Vec<u8> {
  if bincode {
    bincode_next::serde::encode_to_vec(item, *BINCODE_CONFIG).unwrap_or_default()
  } else {
    // let mut buf = Vec::new();
    // let _ = ciborium::into_writer(&item, &mut buf);
    rmp_serde::to_vec_named(&item).unwrap_or_default()
    // serde_json::to_vec(&item).unwrap_or_default()
    // buf
  }
}

pub fn deser_from_vec<T: DeserializeOwned>(item: &[u8], bincode: bool) -> CoreResult<T> {
  Ok(if bincode {
    bincode_next::serde::decode_from_slice(item, *BINCODE_CONFIG)
      .map_err(|x| CoreError::custom(x))?
      .0
  } else {
    rmp_serde::from_slice(item).map_err(|x| CoreError::custom(x))?
    // serde_json::from_slice(item).map_err(|x| CoreError::custom(x))?
    // ciborium::from_reader(item).map_err(|x| CoreError::custom(x))?
  })
}

pub fn deser_string<V: AsRef<Vec<u8>>>(vec: V) -> String {
  deser_from_vec(vec.as_ref(), false).unwrap()
}
