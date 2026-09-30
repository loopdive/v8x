// Run with the compiler checkout's tsx loader and Wasm exception support.
import assert from "node:assert/strict";
import { writeFileSync } from "node:fs";
import { resolve, join } from "node:path";
import { pathToFileURL } from "node:url";
import { CORE_SCRIPT_ORDER, stagedCoreSource, stagedCoreNamespaceSources } from "./staged-core.mjs";
import { CONTEXT_VALUE_BRIDGE_SOURCE, contextValueBridgeEntrypoints } from "./context-value-bridge.mjs";

if (!process.argv[2]) throw new Error("usage: test-staged-core.mjs JS2_CHECKOUT [OUTPUT]");
const { compileMulti } = await import(pathToFileURL(join(resolve(process.argv[2]), "src/index.ts")).href);
const scripts = new Map([
  [CORE_SCRIPT_ORDER[0], '(globalThis as any).first = 1;'],
  [CORE_SCRIPT_ORDER[1], '(globalThis as any).second = 2;'],
  [CORE_SCRIPT_ORDER[2], '(globalThis as any).answer = (globalThis as any).nativeOp(41);'],
  [CORE_SCRIPT_ORDER[3], '(globalThis as any).__bootstrap = { core: { answer: (globalThis as any).answer }, internals: {}, primordials: {} };'],
  ["mod.js", 'const { core, internals, primordials } = (globalThis as any).__bootstrap;\nexport { core, internals, primordials };'],
]);
const result = await compileMulti({
  "/staged/seed.ts": CONTEXT_VALUE_BRIDGE_SOURCE + "\ndeclare function __v8x_attach_context(): void;\n__v8x_attach_context();",
  ...Object.fromEntries(Object.entries(stagedCoreNamespaceSources()).map(([name, source]) => ["/staged/" + name, source])),
  "/staged/core.ts": stagedCoreSource(scripts, { nativeNamespace: true }),
  "/staged/entry.ts": `import "./seed.ts";
import { runScript, scriptPhase, runModule } from "./core.ts";
import * as namespace from "./core-namespace.ts";
export function __v8x_run_deno_core_script(index: number): number { return runScript(index); }
export function __v8x_deno_script_phase(): number { return scriptPhase(); }
export function moduleAnswer(): number { return runModule().core.answer; }
export function namespaceHandle(): number { return imported__v8x_value_keep(namespace); }
let resolvePending: any;
export function pendingPromise(): number {
  return imported__v8x_value_keep(new Promise((resolve) => { resolvePending = resolve; }));
}
export function settlePending(): void { resolvePending(42); }
export function fulfilledPromise(): number { return imported__v8x_value_keep(Promise.resolve(42)); }
export function namespaceProbe(): number {
  if (Object.getPrototypeOf(namespace) !== null) return -1;
  if (Object.keys(namespace).join(",") !== "core,internals,primordials") return -2;
  if (namespace.core !== (globalThis as any).__bootstrap.core) return -3;
  if (Reflect.set(namespace, "core", {})) return -4;
  if (namespace.core.answer !== 42) return -5;
  (globalThis as any).__bootstrap.core.answer = 43;
  return namespace.core.answer;
}
(globalThis as any).moduleAnswer = function (): number { return runModule().core.answer; };
` + contextValueBridgeEntrypoints("./seed.ts"),
}, "/staged/entry.ts", { target: "standalone", platform: "deno", hostBridge: "always", deferTopLevelInit: true, externImportModule: "v8x:deno" });
assert.equal(result.success, true, JSON.stringify(result.errors));

let diagnosticExports;
process.on("uncaughtException", (error) => {
  const e = diagnosticExports;
  if (e && error instanceof WebAssembly.Exception && error.is(e.__exn_tag)) {
    const length = e.__exn_render_prepare(error.getArg(e.__exn_tag, 0));
    let message = "";
    for (let i = 0; i < length; i++) message += String.fromCharCode(e.__exn_render_char(i));
    console.error("Uncaught compiled exception:", message);
  } else console.error(error);
  process.exitCode = 1;
});

async function fresh() {
  let e;
  const reactions = [];
  const { instance } = await WebAssembly.instantiate(result.binary, { "v8x:deno": {
    __v8x_attach_context() {},
    __v8x_host_call(id, receiver, args) {
      if (id === 2) return -e.__v8x_value_number(77) - 1;
      if (id === 1) {
        const value = e.__v8x_value_as_number(e.__v8x_value_get(args, str("0")));
        reactions.push(value);
        return e.__v8x_value_number(value + 1);
      }
      assert.equal(id, 0);
      return e.__v8x_value_number(e.__v8x_value_as_number(e.__v8x_value_get(args, str("0"))) + 1);
    },
  } });
  e = instance.exports;
  diagnosticExports = e;
  const str = (value) => {
    let handle = e.__v8x_value_string_empty();
    for (let i = 0; i < value.length; i++) handle = e.__v8x_value_string_append(handle, value.charCodeAt(i));
    return handle;
  };
  e.__module_init();
  return { e, str, reactions, global: e.__v8x_value_global() };
}

const { e, str, global } = await fresh();
assert.equal(e.__v8x_deno_script_phase(), 0);
assert.equal(e.__v8x_value_kind(e.__v8x_value_get(global, str("first"))), 0);
assert.throws(() => e.__v8x_run_deno_core_script(1));
assert.equal(e.__v8x_deno_script_phase(), 0);
assert.throws(() => e.moduleAnswer());
assert.equal(e.__v8x_run_deno_core_script(0), 1);
assert.equal(e.__v8x_run_deno_core_script(1), 2);
assert.equal(e.__v8x_value_as_number(e.__v8x_value_get(global, str("second"))), 2);
// The op does not exist until the real host registration boundary.
e.__v8x_value_set(global, str("nativeOp"), e.__v8x_value_host_function(0));
assert.equal(e.__v8x_run_deno_core_script(2), 3);
assert.equal(e.__v8x_run_deno_core_script(3), 4);
assert.equal(e.moduleAnswer(), 42);
assert.equal(e.namespaceProbe(), 43);
const namespaceHandle = e.namespaceHandle();
assert.equal(e.__v8x_value_kind(namespaceHandle), 5);
assert.equal(e.__v8x_value_kind(e.__v8x_value_get_prototype(namespaceHandle)), 1);
const exportedCore = e.__v8x_value_get(namespaceHandle, str("core"));
assert.equal(e.__v8x_value_as_number(e.__v8x_value_get(exportedCore, str("answer"))), 43);
assert.throws(() => e.moduleAnswer());

const failed = (await fresh()).e;
failed.__v8x_run_deno_core_script(0);
failed.__v8x_run_deno_core_script(1);
assert.throws(() => failed.__v8x_run_deno_core_script(2));
for (const initial of ["pendingPromise", "fulfilledPromise"]) {
  const { e, reactions } = await fresh();
  const original = e[initial]();
  const derived = e.__v8x_value_promise_then(original, e.__v8x_value_host_function(1), 0);
  assert.notEqual(derived, original);
  assert.deepEqual(reactions, [], "then must not invoke the callback synchronously");
  if (initial === "pendingPromise") e.settlePending();
  assert.deepEqual(reactions, [], "settlement must not invoke the callback synchronously");
  e.__drain_microtasks();
  assert.deepEqual(reactions, [42]);
  const promise = e.__v8x_value_unwrap(derived);
  assert.equal(e.__promise_boundary_state(promise), 1);
  assert.equal(e.__promise_boundary_value(promise), 43);
  const throwing = e.__v8x_value_promise_then(derived, e.__v8x_value_host_function(2), 0);
  const recovered = e.__v8x_value_promise_then(throwing, 0, e.__v8x_value_host_function(1));
  assert.equal(e.__promise_boundary_state(e.__v8x_value_unwrap(throwing)), 0);
  e.__drain_microtasks();
  assert.equal(e.__promise_boundary_state(e.__v8x_value_unwrap(throwing)), 2);
  assert.equal(e.__promise_boundary_value(e.__v8x_value_unwrap(throwing)), 77);
  assert.equal(e.__promise_boundary_state(e.__v8x_value_unwrap(recovered)), 1);
  assert.equal(e.__promise_boundary_value(e.__v8x_value_unwrap(recovered)), 78);
}
assert.equal(failed.__v8x_deno_script_phase(), -1);
assert.throws(() => failed.__v8x_run_deno_core_script(2));
if (process.argv[3]) writeFileSync(resolve(process.argv[3]), result.binary);
console.log("PASS: deferred scripts, host registration gap, module publication, rejected reorder/retry, pending/settled reactions and rejection recovery");
