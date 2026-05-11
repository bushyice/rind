use std::{
  any::{Any, TypeId},
  collections::HashMap,
  sync::Arc,
};

use anyhow::Context;
use crate::types::Ustr;
use kdl::{KdlDocument, KdlNode};

pub trait NamedItem {
  fn name(&self) -> &str;
}

pub trait Model {
  type M: serde::de::DeserializeOwned + NamedItem;
}

type ParserFn = Box<dyn Fn(Vec<KdlNode>) -> anyhow::Result<Box<dyn Any>> + Send + Sync>;

pub struct Metadata {
  pub name: Ustr,
  name_to_type: HashMap<Ustr, TypeId>,
  parsers: HashMap<TypeId, ParserFn>,
  values: HashMap<Ustr, HashMap<TypeId, Arc<Box<dyn Any>>>>,
}

impl Metadata {
  pub fn new(name: impl Into<Ustr>) -> Self {
    Self {
      name: name.into(),
      name_to_type: HashMap::new(),
      parsers: HashMap::new(),
      values: HashMap::new(),
    }
  }

  pub fn of<T>(mut self, name: impl Into<Ustr>) -> Self
  where
    T: Model + 'static,
  {
    let type_id = TypeId::of::<T::M>();

    self.name_to_type.insert(name.into(), type_id);
    self.parsers.insert(
      type_id,
      Box::new(|nodes| {
        let parsed: anyhow::Result<Vec<T::M>> = nodes
          .into_iter()
          .map(|node| {
            let children = node
              .children()
              .cloned()
              .ok_or_else(|| anyhow::anyhow!("node `{}` must contain child fields", node.name()))?;
            serde_kdl2::from_doc::<T::M>(&children)
              .with_context(|| format!("failed to parse `{}` node", node.name()))
          })
          .collect();
        let parsed = parsed?;
        Ok(Box::new(
          parsed
            .into_iter()
            .map(|x| Arc::new(x))
            .collect::<Vec<Arc<T::M>>>(),
        ))
      }),
    );

    self
  }

  pub fn from_kdl(&mut self, src: &str, group: impl Into<Ustr>) -> anyhow::Result<()> {
    let doc: KdlDocument = src.parse()?;
    self.collect_doc(doc, group)
  }

  pub fn collect_doc(&mut self, doc: KdlDocument, group: impl Into<Ustr>) -> anyhow::Result<()> {
    let group = group.into();
    let mut grouped: HashMap<Ustr, Vec<KdlNode>> = HashMap::new();
    for node in doc.nodes() {
      let key = Ustr::from(node.name().value());
      if self.name_to_type.contains_key(&key) {
        grouped.entry(key).or_default().push(node.clone());
      }
    }

    for (name, nodes) in grouped {
      self.insert_nodes(name, nodes, group.clone())?;
    }

    Ok(())
  }

  pub fn insert_nodes(
    &mut self,
    name: impl Into<Ustr>,
    nodes: Vec<KdlNode>,
    group: impl Into<Ustr>,
  ) -> anyhow::Result<()> {
    let name = name.into();
    let type_id = *self
      .name_to_type
      .get(&name)
      .ok_or_else(|| anyhow::anyhow!("unknown metadata key `{name}`"))?;
    let parser = self
      .parsers
      .get(&type_id)
      .ok_or_else(|| anyhow::anyhow!("missing parser for `{name}`"))?;

    let parsed = parser(nodes)?;
    self
      .values
      .entry(group.into())
      .or_default()
      .insert(type_id, Arc::new(parsed));

    Ok(())
  }

  pub fn get_in_group<T: Model + 'static>(
    &self,
    group: impl Into<Ustr>,
  ) -> Option<&Vec<Arc<T::M>>> {
    let group = group.into();
    self
      .values
      .get(&group)?
      .get(&TypeId::of::<T::M>())?
      .downcast_ref::<Vec<Arc<T::M>>>()
  }

  pub fn groups(&self) -> impl Iterator<Item = Ustr> + '_ {
    self.values.keys().cloned()
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use kdl::KdlNode;
  use rind_macros::model;

  #[model(meta_name = name, meta_fields(name, run))]
  struct Service {
    name: Ustr,
  }

  #[test]
  fn parse_grouped_metadata_from_kdl() {
    let mut metadata = Metadata::new("unit").of::<Service>("service");

    let src = r#"
service {
  name "web"
  run "/bin/webd"
}
service {
  name "api"
}
"#;

    metadata
      .from_kdl(src, "demo")
      .expect("kdl should parse into group");

    let services = metadata
      .get_in_group::<Service>("demo")
      .expect("service vec should exist in group");
    assert_eq!(services.len(), 2);
    assert_eq!(services[0].name.as_str(), "web");
    assert_eq!(services[1].name.as_str(), "api");
  }

  #[test]
  fn insert_nodes_type_mismatch_errors() {
    let mut metadata = Metadata::new("unit").of::<Service>("service");

    let node: KdlNode = "service \"not-an-object\"".parse().expect("node should parse");
    let err = metadata
      .insert_nodes("service", vec![node], "demo")
      .expect_err("node without child fields should fail Vec<Service> parser");

    assert!(!err.to_string().is_empty());
  }

  #[test]
  fn unknown_key_is_ignored_by_collect() {
    let mut metadata = Metadata::new("unit").of::<Service>("service");

    let src = r#"
mount {
  name "data"
}
"#;

    metadata
      .from_kdl(src, "demo")
      .expect("unknown top-level arrays are ignored");
    assert!(metadata.get_in_group::<Service>("demo").is_none());
  }
}
