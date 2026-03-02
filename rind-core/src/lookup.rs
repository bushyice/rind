use crate::{
  mount::Mount,
  services::Service,
  sockets::Socket,
  units::{Unit, UnitComponent, Units},
};

use std::collections::HashSet;

pub trait LookUpComponent: Sized {
  fn find_in_unit<'a>(unit: &'a Unit, name: &str) -> Option<&'a Self>;
}

#[derive(Default, Debug, Clone)]
pub struct ComponentFilter {
  pub include: HashSet<String>,
  pub exclude: HashSet<String>,
}

macro_rules! iter_typed_item {
  ($units:expr, $type:ident, ($items:ident, $unit_name:ident) $($body:block)?) => {
    $units.iter().flat_map(|($unit_name, unit)| {
      let $items = <$type as UnitComponent>::iter_field(unit);
      $(
        $body
      )?
    })
  };
}

macro_rules! iter_typed_item_mut {
  ($units:expr, $type:ident, ($items:ident, $unit_name:ident) $($body:block)?) => {
    $units.iter_mut().flat_map(|($unit_name, unit)| {
      let $items = <$type as UnitComponent>::iter_field_mut(unit);
      $(
        $body
      )?
    })
  };
}

macro_rules! impl_unit_component {
  ($field:ident, $type:ty, $name:ident) => {
    type Item = $type;
    fn iter_field(unit: &Unit) -> Box<dyn Iterator<Item = &Self::Item> + '_> {
      match &unit.$field {
        Some(s) => Box::new(s.iter()),
        None => Box::new(std::iter::empty()),
      }
    }

    fn iter_field_mut(unit: &mut Unit) -> Box<dyn Iterator<Item = &mut Self::Item> + '_> {
      match &mut unit.$field {
        Some(s) => Box::new(s.iter_mut()),
        None => Box::new(std::iter::empty()),
      }
    }

    fn item_name(item: &Self::Item) -> &str {
      &item.$name
    }
  };
}

impl UnitComponent for Service {
  impl_unit_component!(service, Service, name);
}

impl UnitComponent for Socket {
  impl_unit_component!(socket, Socket, name);
}

impl UnitComponent for Mount {
  impl_unit_component!(mount, Mount, target);
}

impl LookUpComponent for Socket {
  fn find_in_unit<'a>(unit: &'a crate::units::Unit, name: &str) -> Option<&'a Self> {
    unit.socket.as_ref()?.iter().find(|s| s.name == name)
  }
}

impl LookUpComponent for Service {
  fn find_in_unit<'a>(unit: &'a crate::units::Unit, name: &str) -> Option<&'a Self> {
    unit.service.as_ref()?.iter().find(|s| s.name == name)
  }
}

impl LookUpComponent for Mount {
  fn find_in_unit<'a>(unit: &'a crate::units::Unit, name: &str) -> Option<&'a Self> {
    unit.mount.as_ref()?.iter().find(|s| s.target == name)
  }
}

impl Units {
  pub fn items<T: UnitComponent>(&self) -> impl Iterator<Item = &T::Item> {
    iter_typed_item!(self.units, T, (items, _n) { items })
  }

  pub fn items_mut<T: UnitComponent>(&mut self) -> impl Iterator<Item = &mut T::Item> {
    iter_typed_item_mut!(self.units, T, (items, _n) { items })
  }

  pub fn enabled<T: UnitComponent>(&self) -> impl Iterator<Item = &T::Item> {
    iter_typed_item!(self.units, T, (items, unit_name) {
      let filter = self.enabled.get(unit_name);
      items.filter(move |item| {
        if let Some(f) = filter {
          let name = T::item_name(item);
          !f.exclude.contains(name) && (f.include.is_empty() || f.include.contains(name))
        } else {
          true
        }
      })
    })
  }

  pub fn enabled_mut<T: UnitComponent>(&mut self) -> impl Iterator<Item = &mut T::Item> {
    iter_typed_item_mut!(self.units, T, (items, unit_name) {
      let filter = self.enabled.get(unit_name);
      items.filter(move |item| {
        if let Some(f) = filter {
          let name = T::item_name(item);
          !f.exclude.contains(name) && (f.include.is_empty() || f.include.contains(name))
        } else {
          true
        }
      })
    })
  }

  pub fn lookup<T: LookUpComponent>(&self, name: &str) -> Option<&T> {
    if let Some((unit_name, thing)) = name.split_once('@') {
      let unit = self.units.get(&unit_name.into())?;
      T::find_in_unit(unit, thing)
    } else {
      self
        .units
        .values()
        .find_map(|unit| T::find_in_unit(unit, name.into()))
    }
  }
}
