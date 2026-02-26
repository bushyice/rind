use rind_core::{
  name::Name,
  units::{UNITS, Unit, Units},
};

#[derive(serde::Serialize, serde::Deserialize)]
pub struct UnitsSerialized {
  pub units: Vec<String>,
  pub names: Vec<String>,
  pub enabled: Vec<String>,
}

impl UnitsSerialized {
  pub fn to_string(&self) -> String {
    toml::to_string(&self).unwrap()
  }

  pub fn to_units(&self) -> Units {
    let mut units = Units::default();

    let units_iter = self
      .units
      .iter()
      .map(|x| toml::from_str::<Unit>(x).unwrap());

    let names: Vec<Name> = self.names.iter().map(|x| Name::from(x.clone())).collect();

    for (index, unit) in units_iter.enumerate() {
      let name = &names[index];
      // let enabled = self.enabled.contains(&name.to_string());

      units.insert_unit(name.clone(), unit);

      // if enabled {
      //   units.enable_unit(name.clone(), false);
      // }
    }

    units
  }

  pub fn from_registry() -> Self {
    let units = UNITS.read().unwrap();

    UnitsSerialized {
      units: units.units().map(|u| toml::to_string(u).unwrap()).collect(),
      names: units.names().map(|k| k.to_string()).collect(),
      enabled: units.enabled_names().map(|x| x.to_string()).collect(),
    }
  }

  pub fn from_string(s: String) -> Self {
    toml::from_str(&s).unwrap()
  }
}

impl From<Units> for UnitsSerialized {
  fn from(value: Units) -> Self {
    UnitsSerialized {
      units: value.units().map(|u| toml::to_string(u).unwrap()).collect(),
      names: value.names().map(|k| k.to_string()).collect(),
      enabled: value.enabled_names().map(|x| x.to_string()).collect(),
    }
  }
}
