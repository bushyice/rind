[[Flow]] is the transition engine that maps [[Signals]] + current [[States]] into new states and service lifecycle actions. They are broadcasted all across [[Rind]] and [[Services]].

## Flow Components
- [[States]]: Long-lived runtime facts that persist across reboots.
- [[Signals]]: One-shot runtime events with a no persistence.

## Responsibilities

- evaluate state transition rules,
- resolve guards/conditions,
- compute start/stop/restart intents,
- coordinate branching behavior,
- maintain transition invariants.
- handle and maintain the [[State Machine]]

## Evaluation Model

![[tree-example.png]]

Conceptually:

- Inputs: current states, incoming signal, variables, runtime context.
- Rules: matching conditions, guard predicates, branch selectors.
- Outputs: state mutations, lifecycle actions, emitted events/signals.

## Failure Handling

Flow implementations should define behavior for:

- invalid transition attempts,
- missing branch keys,
- unresolved dependencies,
- retry/backoff policy for failed lifecycle actions.
