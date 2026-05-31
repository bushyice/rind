Modern computing is built around separate programs we call applications. An application for taking notes, for browsing the web, for video editing... Each application defines its own boundaries, its own systems, its own languages, its own layers of software.

## The system hogs

Applications have long grown independent of the system as no matter what system they are on, they create their own system, introducing complexities that require more duplication, containerization and isolation to contain the software that runs in complexity more than the system they are on.

As isolation deepens, duplication follows. Logic, state and resources become repeatedly reconstructed across disconnected software boundaries. And then software is fragmented, system gets incoherent in exchange for containment. 

## A system of systems

When I think of components, I see pieces of machine logic that each serve a specific domain. Could be the audio stack, could be the display stack, could be filesystem stack, etc. The point here being components should be supplying systems, whereas applications should focus on their own scope. For example, a text-editor app uses the display stack and filesystem stack but does not need to isolate itself nor create systems around the existing system.

In conclusion, a component is responsibility. A responsibility that only exists once for a domain. A component is not valuable because it is reusable but because it is replaceable.