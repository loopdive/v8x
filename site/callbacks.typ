#import "./shim/html.typ": *

#set document(
  title: "callbacks & exceptions · v8x",
  description: "How native callbacks cross the engine C frame into Rust: trampolines, IsoState, exception side state, and template records.",
)

#show: html-shim

#crumb(3, [callbacks and exceptions])

= Callbacks and exceptions

JSC and QuickJS native callbacks cross an engine C frame before reaching Rust.
The trampoline does five things, in order:

+ restore the thread-local isolate and context; many ABI functions receive
  only a value pointer, so this state must already be in place
+ intern the receiver and arguments, which on QuickJS allocates arena slots
+ build the exact `FunctionCallbackInfo` pointer layout rusty_v8 expects
+ call the Rust callback, catching panics so they never unwind through
  engine frames
+ translate the return-value slot back into an engine value

The experimental js2wasm backend uses Wasmtime host imports instead of an
engine C trampoline. Values stay rooted in their compiled realm, and Rust
wrappers retain object identity. Synchronous nested callbacks use the active
Wasmtime caller to access that realm. Ordinary host callbacks and built-in
error transport are covered by focused tests; host construction, arbitrary
exotic values and complete exception identity are not implemented.

A realm-backed callback can have an undefined or non-string JavaScript
`name` property. Function conversion preserves that property and call identity
in the realm; it uses an empty native wrapper display name when there is no
string name to cache. Focused tests cover both cases and ordinary named
functions.

== Exceptions live in side state

JSC records the pending exception and its context in `IsoState` on every
call that can throw. QuickJS turns `JS_TAG_EXCEPTION` into
`JS_GetException`. `TryCatch`, `MaybeLocal`, and the message functions read
that stored state; they never ask the engine directly.

== IsoState

The implicit V8 machinery lives in one struct behind the opaque
`v8::Isolate` pointer:

#table(
  columns: 2,
  [*backend*], [*IsoState owns*],
  [JSC], [context group, global contexts, entered-context stack, protected
    locals, pending exception, weak records, GC callbacks],
  [QuickJS], [runtime, contexts, handle arena, persistent cells, module
    tables, snapshot state, promise hooks, interrupt state, memory counters],
)

One ordering rule: QuickJS sizes each context's class table at
`JS_NewContext`, so external-object and named-handler classes register
before the first context exists.

== Templates

Neither engine has V8's template concept. The template description
(properties, accessors, callbacks, internal-field count) lives in a native
adapter record, and instantiation reads it to build the real engine object.
Internal fields go in backend-owned records tied to the engine object; they
hold raw native pointers and stay invisible to JavaScript enumeration.
Function templates store the Rust callback and its data, then install the
trampoline above when materialized.

#next("modules", [Modules: identity across compile, instantiate, evaluate])


The js2wasm runtime artifact exposes deferred core-script stages. A focused
Rust-host fixture verifies that callbacks registered between stages are visible
to later compiled scripts, and that failed stages cannot be retried. Native
context internal fields remain in the Rust wrapper when its object enters the
realm. Native microtasks preserve continuation data across callbacks. These
checks do not establish complete Deno compatibility.

A pinned, unchanged Deno core fixture now boots through public `Script::Run` and
executes its hello-world example with real Rust callbacks. Pending ops settle
through `core.__eventLoopTick` and the native microtask checkpoint, preserving
scalar, object and rejection-reason identity. Compiled queues publish their
pending-job count so checkpoints verify quiescence. Native `Promise.then/catch`
registration on this compiled realm preserves asynchronous callbacks, derived
Promise results, thrown handlers and native continuation data. Separately
compiled namespace graphs route pending and settled Promise reactions through
their ownership-checked intrinsic export in the shared store. Compiled Promise
handler state is persistent in Wasm and shared by native `HasHandler` and
`MarkAsHandled`. The pinned realm test checks this state before and after a
microtask checkpoint. An opt-in compiled enqueue notification now inserts
single-job drains into the same FIFO as native callbacks. The pinned realm
test verifies a compiled reaction, a native callback and a chained compiled
reaction in their enqueue order, including compiler-free AOT replay. Older
artifacts without this notification retain batch draining. General rejection
events remain incomplete for Wasm-owned Promises. Native Promise resolvers
now deliver unhandled rejection, first late-handler attachment and duplicate
settlement notifications through an isolate-local rusty_v8 callback. Public
API controls check exact Promise/reason identity, absent values for handler
events, reentrant attachment and isolate isolation. An event-producing compiled
test context also delivers unhandled rejection and first late-handler events
with exact Promise/reason identity. The callback can attach a handler after the
runtime borrow ends. Both controls pass in compiler-free AOT replay. This
opt-in transport is included in the runtime compiler pin. A clean complete-core
artifact passes the compiler-free public API controls without an interpreter
provider. Compiled notifications share an isolate-local queue. A compiler-free
two-realm control verifies that callback reentry cannot overtake a previously
queued handler notification, preserving each realm's Promise and reason
identity. General thenable and duplicate-resolution coverage remains incomplete.
A separate two-graph acceptance test verifies
compiled reactions interleaved with native callbacks in one shared Store,
including compiler-free replay of trusted context and source-bound graph
artifacts. It also checks live namespace values, exception identity and
that no interpreter provider is instantiated. This is a focused acceptance
test, not complete module or Deno conformance.

Pending reactions within one compiled Promise run in registration order on
fulfillment and rejection. Multi-reaction lists are reordered at settlement;
the common single-reaction path needs no copied callback nodes. A native Deno
pending-op check verifies three Rust reactions in registration order. This is
not proof of global ordering across all graph and native queues.

Linked graph compilation can select the context's exported exception tag with
`standaloneGlobalThisImport.exceptionTag`. This shares tag identity without a
JavaScript host. A compiler control verifies that a provider-thrown payload is
caught unchanged; separate module-local tags cannot catch that exception.

The Deno host installs `Error.captureStackTrace` before primordial capture. It
records actual Wasmtime frames and a non-enumerable stack property without
replacing the error. Source locations, identity-based `constructorOpt` trimming
and full V8 stack formatting are not implemented.

The compiled value bridge has a string-conversion operation with a success and
value result. Its focused compiled controls verify Error text, the string hint
for object coercion, rejection of Symbol values and original thrown-value
identity. This is not a complete native coercion or Deno conformance result.
Compiler-free native replay also verifies string coercion reentry into Rust
and native TryCatch retaining the original Rust-thrown object. The unchanged
core async stub refusal reaches native code with its Error text intact.
The compiled upstream error builder preserves the message for its six registered
builtin error classes when constructors are passed through parameters. The
unchanged upstream Deno hello-world example also preserves the full native
serde error diagnostic with the compiler-free AOT artifact.

Native js2wasm functions retain the length supplied by Function and
FunctionTemplate builders. The property is included when a host callback
enters the compiled realm. A bootstrap fixture checks its value after transfer.

Host object graph adoption preserves explicit null and object prototypes,
including shared identity and property cycles. Prototype reads and writes on
adopted objects use the compiled realm; rejected prototype cycles return false.

Native numeric conversion uses the compiled realm for strings and objects.
Native arrays acquire that realm's intrinsic iterator on demand. The unchanged
Deno WebIDL integer and basic sequence conversions pass. A clean core build
also passes the retained live-mutation iterator control after the compiler's
first-class values factory was changed to read the original receiver live.
The ordinary compiler-free adapter run passes 34 tests with 14 ignored; additional
artifact-backed controls cover function, foreign-value and module execution paths.
This does not establish full WebIDL or Deno compatibility.

Context artifacts also export a lexical operation for independently compiled
Scripts. The linker resolves it against the retained Context, without requiring
a runtime-eval provider. A separate native fixture covers persistent lexical
state. Public source-bound Script packages also retain completion values and
exceptions as native references. Typed lexical bindings and full unchanged
Deno conformance remain incomplete.
Inferred number and boolean constants retain their typed slots across the
lexical operation. Shared global-object function writes use native AOT callable
carriers, allowing later Scripts to call them without an interpreter. Mutable
and reference-typed bindings still lack safe cross-Script type planning.

Known function bodies can be instantiated during a native host callback without
borrowing the runtime again. Nested instantiation preserves the caller's completion
and retains the new owning graph even if its initializer throws. Cross-Script reads
and calls select the allocation owner, not a structural layout or an undefined
result. Native controls cover thrown getter identity, own undefined and receiver
identity. New Script packages also export the three-reference Reflect getter ABI,
so an explicit receiver from another graph reaches the owning accessor unchanged.
Old packages keep their two-reference getter; alternate receivers are refused
when that extended export is absent.

Module evaluation failures return a cached rejected Promise, preserving the native
exception value. A Promise reaction uses its own exception scope, so a surrounding
TryCatch cannot turn a thrown callback into a fulfilled derived Promise. Native
controls cover missing graph artifacts and synthetic thrown-value identity; a
precompiled source graph also verifies observable global writes and namespace
publication. A throwing AOT graph retains its original payload in the Context
keeper. Its rejected Promise and Module exception preserve that object's identity;
property reads use proven graph allocation ownership even when initialization
never reached namespace publication. Native Promise transport remains incomplete.
A mirror path uses a real compiled Promise and native identity bindings. Native
controls verify fulfillment and rejection before and after transfer, exact
payload identity and single rejection delivery. Unsupported settled payloads
are refused before publication. The full Deno Context has been rebuilt with these
exports. The unchanged main/side module test passes, and missing packages report
the original loading error. Full Deno transport coverage is not established.

The native import-meta callback registration and lazy object cache now retain
Module identity across repeated and reentrant initialization. Modules with the
same URL do not share metadata. Compiled module access now uses instance-bound
host capabilities, including loader-provided properties and resolve callbacks.
The unchanged Deno main-versus-side module control passes. A native AOT control
verifies null-prototype identity and an older graph's metadata after another
same-URL Module executes. Native plain-object CreateDataProperty supports these
initialization callbacks; defining properties on compiled and native exotic
objects is still explicitly refused. Full module conformance is not established.
Five selected unchanged module tests pass with exact AOT packages, including
importing the built-in core from another graph, metadata resolution,
filename/dirname and repeated evaluation. Missing graph packages fail loading.
A two-entry native control retains the original dependency namespace, executes
the dependency once and observes live named and namespace imports. Calls use
the original function's receiver state. The control also verifies namespace
and bare receivers, distinct same-URL Modules and no compiler or interpreter.
The same JavaScript fixture passes under Node's V8 module evaluator.
Canonical shared-module linking remains experimental. Prepared IR initialization,
cycles, temporal dead zones and full unchanged population still need coverage.
Within one graph, repeated namespace imports, namespace re-exports and native
publication share one object. Compiler controls and the first entry of the
native shared-dependency control verify this identity.
