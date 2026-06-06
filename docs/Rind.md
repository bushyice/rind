---
title: Welcome to rind
aliases:
  - Rind
---
> [!CAUTION]
> **Experimental Status:** Rind is currently a proof of concept. The codebase is under active development and in a state of rapid flux. Expect architectural shifts, breaking changes, and occasional instability as I refine the core engine.

## What is Rind?

**[[Rind]]** is an init system and system runtime written in Rust. It replaces the traditional model of static service dependency graphs with a reactive, state-driven architecture where services emerge from the live conditions of the machine rather than from a fixed boot order.

Where conventional init systems ask *"what depends on what?"*, Rind asks *"what is true right now, and what should happen because of it?"*

---

## Philosophy

Rind is built on a set of [[Philosophy/The Awkward Philosophy|philosophical pillars]] that guide its design:

- **[[Philosophy/Continuity|Continuity]]**: The machine should preserve its state across interruption. A system that knows itself can rebuild from what changed, not from scratch.

- **[[Philosophy/Reactivity|Reactivity]]**:  Change is not discovered through polling. It propagates. Components do not ask *"has this changed yet?"*; they are told.

- **[[Philosophy/Persistence|Persistence]]**: Identity survives reboot. Transient states birth change, persistent states carry it forward. The system is frugal about what lives and what dies.

- **[[Philosophy/Branching|Branching]]**: Reality is not singular. Systems encounter branching at every turn. Branches do not isolate; they extend the same continuity graph.

- **[[Philosophy/Communication|Communication]]**: Sharing meaning, not just bytes. A system that understands itself speaks in contextual messages, not imperative commands.

- **[[Philosophy/Unity|Unity]]**: Fragmentation separates understanding. Rind seeks a shared reality where components communicate, understand, and react in one continuous system.

- **[[Philosophy/Componentization|Componentization]]**: Components are responsibilities. A responsibility that exists once for a domain. Valuable not because they are reusable, but because they are replaceable.

---

## Architecture

At a high level, Rind is composed of three layers:

### Flow Engine
The [[Architecture/Flow|Flow engine]] is the reactive core. It watches events, evaluates conditions against [[Architecture/Facets|Facets]] (what the system *is*), and emits [[Architecture/Impulses|Impulses]] (what the system *does*). Facets are persistent, branchable state facts. Impulses are ephemeral shouts. Together they form a dynamic dependency graph where services respond to real conditions rather than static declarations.

### Orchestrators & Runtimes
[[Architecture/Orchestrators|Orchestrators]] handle discovery and wiring — reading unit definitions from disk, building indexes, and registering metadata during boot. [[Architecture/Runtimes|Runtimes]] are the active layer — they handle actions, manage service lifecycles, and communicate by dispatching messages to each other.

### The Boot Engine
The [[Architecture/Boot|Boot engine]] sequences the system from dead to alive through distinct cycles — Collect, Runtime, PostRuntime — and then enters a continuous Pump cycle that drives all reactivity. It orchestrates the transition from static configuration to living state.

---

## Units

Everything Rind manages is described by [[Architecture/Units|Units]] — TOML definition files that declare services, facets, impulses, timers, sockets, mounts, networks, permissions, and variables. Units are the metadata layer; the [[Architecture/Registry|Registry]] is the living database built from them.

---

## Key Ideas

| Idea                        | Description                                                                                                                                                                                   |
| --------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **Facets over files**       | System state lives in named, typed, branchable facts and not scattered across config files and environment variables.                                                                         |
| **Branches over instances** | A single service definition can spawn unique instances per context (e.g., per-user, per-TTY) through facet branching.                                                                         |
| **Transcendence**           | Facets declare dependencies on other facets. When all dependencies are met, the facet activates. When any is removed, it deactivates. The system maintains these relationships automatically. |
| **Transport protocols**     | Services communicate with the daemon via stdio, UDS, shared memory, environment variables, or command-line arguments — chosen per service.                                                    |
| **Scoped isolation**        | The [[Scopes\|Scope]] system provides per-user or per-domain metadata namespaces with their own units, facets, and lifecycle.                                                                 |
| **Permissions**             | Access control for facet mutations and impulse emissions, with overlay grants, group-based rules, and expression evaluation.                                                                  |
| **Plugins**                 | The system is extensible through plugins that can introduce new orchestrators, runtimes, entity types, and custom extensions.                                                                 |

---

## Go Deeper

- [[Architecture/Overview|Architecture Overview]]
- [[Philosophy/The Awkward Philosophy|The Awkward Philosophy]]
- [[Architecture/Flow|Flow Engine]]
- [[Architecture/Services|Services]]
- [[Architecture/Boot|Boot Sequence]]
