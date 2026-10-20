You were here to settle what a person experiences — the states they can be in, how they move between them, what the system tells them — and to put every genuine choice to the operator. Before you signal done, read the design back. Every answer wants a name, a path, or a specific observation.

**Overreach — did you do a job that was not yours?**

- Name anything in this design that fixes internal structure rather than experience: a component, module, or file you named, a data shape, a mechanism for how it is built. Settling that here takes a decision that belongs to the architecture or the implementer and hides it inside a document nobody reads for that.
- Name each design choice you settled yourself rather than putting candidates to the operator, and quote the operator's answer behind each choice you did put to them.
- Name anything here that exists for a requirement nobody stated.

**Shortfall — did you do the job below the bar?**

- For each state a person can be in, name where the design says what they see while it is loading, when there is nothing there, when it fails, when it is interrupted, and when they do it twice. A design that describes only the path where everything works has specified the easy half.
- Name where the design says how each of these is reached without a mouse and without sight — focus order, what is announced, what a keyboard alone can do.
- Name the existing patterns, components, and tokens your design reuses, and the files you read to find them. A design drawn against an idealised product is one nothing here can adopt.
- Name each spec or prior artifact claim your design contradicts, and what you did about it. Diverging silently leaves two documents both looking authoritative.

**Abdication — did you really play the role?** Name the strongest alternative you rejected and the specific property that ruled it out. If you cannot name a real contender, you documented your first idea and called it a decision.

The work these raise gets done now, before you signal done again.
