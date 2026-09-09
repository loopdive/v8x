// Run with the compiler checkout's tsx loader and Wasm exception support.
import assert from "node:assert/strict";
import { writeFileSync } from "node:fs";
import { resolve, join } from "node:path";
import { pathToFileURL } from "node:url";
import { CORE_SCRIPT_ORDER, stagedCoreSource } from "./staged-core.mjs";
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
  "/staged/core.ts": stagedCoreSource(scripts),
  "/staged/entry.ts": `import "./seed.ts";
import { runScript, scriptPhase, runModule } from "./core.ts";
export function __v8x_run_deno_core_script(index: number): number { return runScript(index); }
export function __v8x_deno_script_phase(): number { return scriptPhase(); }
export function moduleAnswer(): number { return runModule().core.answer; }
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
  const { instance } = await WebAssembly.instantiate(result.binary, { "v8x:deno": {
    __v8x_attach_context() {},
    __v8x_host_call(id, receiver, args) {
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
  return { e, str, global: e.__v8x_value_global() };
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
assert.throws(() => e.moduleAnswer());

const failed = (await fresh()).e;
failed.__v8x_run_deno_core_script(0);
failed.__v8x_run_deno_core_script(1);
assert.throws(() => failed.__v8x_run_deno_core_script(2));
assert.equal(failed.__v8x_deno_script_phase(), -1);
assert.throws(() => failed.__v8x_run_deno_core_script(2));
if (process.argv[3]) writeFileSync(resolve(process.argv[3]), result.binary);
console.log("PASS: deferred scripts, host registration gap, module publication, rejected reorder/retry");
