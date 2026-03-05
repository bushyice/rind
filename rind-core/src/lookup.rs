use crate::{
  flow::{SignalDefinition, StateDefinition},
  mount::Mount,
  name::Name,
  services::Service,
  store::Store,
  units::{Unit, UnitComponent},
};

use std::collections::HashSet;

pub trait LookUpComponent: Sized {
  fn find_in_unit<'a>(unit: &'a Unit, name: &str) -> Option<&'a Self>;
  fn find_in_unit_mut<'a>(unit: &'a mut Unit, name: &str) -> Option<&'a mut Self>;
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

macro_rules! impl_lookup_component {
  ($field:ident, $name:ident, $key:expr) => {
    fn find_in_unit<'a>(unit: &'a crate::units::Unit, name: &str) -> Option<&'a Self> {
      let $field = unit.$field.as_ref()?;
      let id = format!("{}@{}", $key, name);

      if unit.index.contains_key(&id) {
        let index = unit.index.get(&id)?;
        return $field.get(*index);
      }

      let mut iter = $field.iter().filter(|m| m.$name == name);

      let first = iter.next()?;
      if iter.next().is_some() {
        None
      } else {
        Some(first)
      }
    }

    fn find_in_unit_mut<'a>(unit: &'a mut crate::units::Unit, name: &str) -> Option<&'a mut Self> {
      let $field = unit.$field.as_mut()?;
      let id = format!("{}@{}", $key, name);

      if unit.index.contains_key(&id) {
        let index = unit.index.get(&id)?;
        return $field.get_mut(*index);
      }

      let mut iter = $field.iter_mut().filter(|m| m.$name == name);

      let first = iter.next()?;
      if iter.next().is_some() {
        None
      } else {
        Some(first)
      }
    }
  };
}

impl UnitComponent for Service {
  impl_unit_component!(service, Service, name);
}

impl UnitComponent for StateDefinition {
  impl_unit_component!(state, StateDefinition, name);
}

impl UnitComponent for SignalDefinition {
  impl_unit_component!(signal, SignalDefinition, name);
}

impl UnitComponent for Mount {
  impl_unit_component!(mount, Mount, target);
}

impl LookUpComponent for StateDefinition {
  impl_lookup_component!(state, name, "state");
}

impl LookUpComponent for SignalDefinition {
  impl_lookup_component!(signal, name, "signal");
}

impl LookUpComponent for Service {
  impl_lookup_component!(service, name, "service");
}

impl LookUpComponent for Mount {
  impl_lookup_component!(mount, target, "mount");
}

impl LookUpComponent for Unit {
  // placeholder
  fn find_in_unit<'a>(_unit: &'a Unit, _name: &str) -> Option<&'a Self> {
    None
  }

  fn find_in_unit_mut<'a>(_unit: &'a mut Unit, _name: &str) -> Option<&'a mut Self> {
    None
  }
}

impl Unit {
  pub fn len<T: UnitComponent>(&self) -> usize {
    T::iter_field(self).count()
  }

  pub fn len_for<T: UnitComponent>(&self, filter: fn(item: &T::Item) -> bool) -> usize {
    T::iter_field(self).filter(|x| filter(x)).count()
  }
}

impl Store {
  pub fn items<T: UnitComponent>(&self) -> impl Iterator<Item = (&Name, &T::Item)> {
    iter_typed_item!(self.units, T, (items, unit_name) { items.map(move |item| (unit_name, item)) })
  }

  pub fn items_mut<T: UnitComponent>(&mut self) -> impl Iterator<Item = (&Name, &mut T::Item)> {
    iter_typed_item_mut!(self.units, T, (items, unit_name) { items.map(move |item| (unit_name, item)) })
  }

  pub fn enabled<T: UnitComponent>(&self) -> impl Iterator<Item = (&Name, &T::Item)> {
    iter_typed_item!(self.units, T, (items, unit_name) {
      let filter = self.enabled.get(unit_name);
      items.filter_map(move |item| {
        if let Some(f) = filter {
          let name = T::item_name(item);
          if f.is_empty() || f.contains(name) {
            Some((unit_name, item))
          } else {
            None
          }
        } else {
          None
        }
      })
    })
  }

  pub fn enabled_mut<T: UnitComponent>(&mut self) -> impl Iterator<Item = (&Name, &mut T::Item)> {
    iter_typed_item_mut!(self.units, T, (items, unit_name) {
      let filter = self.enabled.get(unit_name);
      items.filter_map(move |item| {
        if let Some(f) = filter {
          let name = T::item_name(item);
          if f.is_empty() || f.contains(name) {
            Some((unit_name, item))
          } else {
            None
          }
        } else {
          None
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

  pub fn lookup_mut<T: LookUpComponent>(&mut self, name: &str) -> Option<&mut T> {
    if let Some((unit_name, thing)) = name.split_once('@') {
      let unit = self.units.get_mut(&unit_name.into())?;
      T::find_in_unit_mut(unit, thing)
    } else {
      self
        .units
        .values_mut()
        .find_map(|unit| T::find_in_unit_mut(unit, name.into()))
    }
  }
}
