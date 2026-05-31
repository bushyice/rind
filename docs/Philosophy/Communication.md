In software, we are always solving communication problems. Processes are built isolated by-design, applications are sandboxed, components cloned for each context. Parts of the software grow independent and internally coherent but systematically incoherent. Because, essentially, communication is not about the transfer of bytes, it's about sharing the meaning. 


## Connection forges

Typically, we communicate processes through methods that cost both programs: polling, serialization, cloning, refresh of states... It becomes more like a side-quest. A design of an internal language as an interface for communication, a "meaning" only understood by software written to uphold such language, while the system itself remains unaware.

## Understanding

Communication implies messaging, it does not embody it. Understanding what a process means from one program to another in a way that gathers everyone into one language- that's what communication is.

Traditionally, we communicated through authoritarian commands such as "do this", which is effective, tells direct commands to a program, yet- the boundary remains local. A systematic approach would be to tell "this happened", "this is happening", "this changed". Difference lies in what this unlocks, because this is a contextual message. A system that understands itself cannot rely on solely imperative exchange, it could instead preserve continuity in meaning between components so that it can effectively construct a shared context.

In the end, the essence of communication is the distinction of **who should share this meaning**.