A [[Context]] is a transient execution handle in [[Rind]] that provides a component (either [[Orchestrators]] or [[Runtimes]]) with scoped access to system resources. Contexts are short-lived, existing only for the duration of a single dispatch or [[Boot#Boot Phases|Boot Phases]].

## Orchestrator Context

The [[#Orchestrator Context]] is used during the [[Boot]] sequence to allow policy components to set up the system.

- **Registry**: Provides access to metadata registration and instance data.

- **Dispatcher**: Enables sending initial commands to any registered [[Runtimes]] via [[Runtimes#Runtime Dispatcher|Runtime Dispatcher]].

## Runtime Context

The [[#Runtime Context]] is used during action handling to provide [[Runtimes]] with their specific execution environment.

- **Scope**: Provides access to runtime-specific dependencies via the [[Scopes]] system.

- **Registry**: Allows for querying metadata and mutating domain-specific instances.

- **Event Bus**: Permits publishing or subscribing to system-wide events.

- **Lifecycle**: Exposes the queue for scheduling asynchronous service transitions.