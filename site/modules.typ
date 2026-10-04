#import "./shim/html.typ": *

#set document(
  title: "modules · v8x",
  description: "Canonical module wrappers keep V8 module identity stable across compile, instantiate, and evaluate on JSC and QuickJS.",
)

#show: html-shim

#crumb(4, [modules])

= Modules

`deno_core` compiles a module, inspects its requests, instantiates it with
a resolver, sets `import.meta`, and intercepts dynamic `import()`. It
expects the same wrapper identity back from every one of those hooks, so
each backend keeps a canonical wrapper:

#table(
  columns: 2,
  [*backend*], [*how identity is kept*],
  [QuickJS], [module state, synthetic exports, and namespaces are keyed by
    the wrapper object's payload pointer; a name map retains the canonical
    wrapper],
  [vendored JSC], [native module records map to the same Rust-facing
    wrapper],
  [system JSC], [no public module hooks, so only closed graphs, flattened
    by a bundler before execution; fits `deno compile` and desktop apps,
    rules out open module loading],
)

The system-JSC row is a deliberate trade. A closed-graph restriction is
stronger than a partial emulation of hooks Apple's framework does not
expose, and it is much easier to test.

The experimental js2wasm backend keys module state by native wrapper identity.
When the next dependency has already failed, later evaluation returns a cached
rejected Promise with the original exception object. This includes transitive
dependencies and does not rerun the failed callback. An earlier unevaluated
dependency prevents this shortcut so its execution order is retained.
Leading synthetic dependencies execute their native callbacks before source
graph packaging. A first callback failure retains the original exception too.
Successful synthetic/source graph composition is not yet supported.
Opt-in source lifecycle packages publish a completed dependency's namespace
before a later source fails. Completed dependencies remain usable, executing
sources and their consumers retain the original exception, and later siblings
remain untouched. Calls into the completed dependency retain its original
allocation owner even before the failed entry's export registry is ready.
Packages without lifecycle events cannot recover these per-source states.
If a fresh source dependency precedes a cached source failure, its body runs
first. The namespace capability then throws the original cached JS object
through the Context's shared exception tag. The consumer's rejected Promise
retains that object without reexecuting the failed source.
The shared-module, typed-module, module-evaluation, and selected Deno module
test package builders explicitly enable lifecycle events. Generic graph
packaging still defaults to disabled.
Pinned-original Deno packages also cover main/side loader identity and the
test_mods import/host-op case. The latter checks that instantiation alone does
not execute source and that evaluation dispatches the Rust op exactly once.
General cyclic evaluation, snapshots, and complete Deno integration remain
unverified.

#next("snapshots", [Snapshots: record and replay])
