[[Rind]] has two registries that serve as the central database of the whole system, holding all configuration such as metadata and all the runtime data such as the [[Flow#FacetGraph|FacetGraph]].

## MetadataRegistry
The `MetadataRegistry` is a registry of all [[Entities#Metadata|Metadata]] along with an index of their [[Entities#Address|Addresses]]. 

```rust
pub struct MetadataRegistry {
    metadata: HashMap<Ustr, Arc<Metadata>>,
    pub indexes: HashMap<TypeId, HashMap<Ustr, usize>>,
    pub stoppers: HashMap<TypeId, (&'static str, &'static str)>,
}
```

| Field       | Purpose                                                                 |
| ----------- | ----------------------------------------------------------------------- |
| `metadata`  | Map of metadata names to their `Metadata` instances                     |
| `indexes`   | Type-indexed lookup tables for fast entity retrieval                     |
| `stoppers`  | Registered runtime+action pairs for stop signal routing                 |


## Metadata Pages

```rust
pub struct Metadata {
    pub name: Ustr,
    name_to_type: HashMap<Ustr, TypeId>,
    parsers: HashMap<TypeId, ParserFn>,
    values: HashMap<Ustr, HashMap<TypeId, Arc<Box<dyn Any>>>>,
}
```

A metadata page is a container of a set of [[Entities]] and other metadata properties.

A metadata page can hold type-separated lists of [[Entities#Models|Models]] and filter them out via [[Entities#Address|Addresses]] 

## Instance Registry
The instance registry is where everything integral happens, from [[Entities#Instantiation|Instantiation]] to singleton stores, it's the living database of the whole system.

```rust
pub struct InstanceRegistry<'a> {
    pub metadata: &'a MetadataRegistry,
    pub instances: &'a mut InstanceMap,
}

pub type InstanceMap = HashMap<Ustr, Vec<Box<dyn Any>>>;
```

### Instantiation
The most integral part is instantiation of [[Entities#Models|Models]], as models are the basic language of the internal state.

```rust
// instantiate
let service = registry.instantiate::<Service>("static", "example:web_service" |metadata| Service {
	metadata,
	state: ServiceState::Inactive
})?;

// get instances
let service_instances = registry.instances::<Service>("static", "example:web_service")?; // or instances_mut

// uninstantiate
registry.uninstantiate::<Service>("static", "example:web_service")?;

// for single-instances you can do as follows:
let service = registry.instantiate_one::<Service>("static", "example:web_service" |metadata| Service {
	metadata,
	state: ServiceState::Inactive
})?;

// get single-instance
let service = registry.as_one::<Service>("static", "example:web_service")?; // or as_one_mut

// uninstantiate single-instance
registry.uninstantiate_one::<Service>("static", "example:web_service")?;
```

### Singletons
Singletons are single-instance stores such as [[Flow#FacetGraph|FacetGraph]]. They are initiated once and then borrowed for specific use.

```rust
// register singleton
registry.singleton_or_insert_with::<FacetGraph>(FacetGraph::KEY, || FacetGraph::default());

// borrow singleton
let fg: Option<&FacetGraph> = registry
	.singleton::<FacetGraph>(FacetGraph::KEY);

// borrow singleton as mutable
let fg_mut: Option<&mut FacetGraph> = registry
	.singleton_mut::<FacetGraph>(FacetGraph::KEY);
```

**Multiple borrows**: To borrow multiple singletons at once, you can use `singleton_handle` as follows:
```rust
ctx.registry
	.singleton_handle::<(&mut FacetGraph, &mut VariableHeap), _)>>>(
	(FacetGraph::KEY.into(), VariableHeap::KEY.into()),
	|registry, (fg, vh)| { Ok(()) })?;
```

- **The singletons (A, B)**: A list of what to borrow
- **The return type**: What the result of the closure is
- **The keys (A, B)**: A list of keys by the order of the type
- **The closure**: Where the actual borrowing happens

See also: [[Units]], [[Orchestrators]], [[Context]], [[Runtimes]], [[Persistence]]
