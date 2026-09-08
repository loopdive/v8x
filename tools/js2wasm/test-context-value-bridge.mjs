// Run with Node Wasm exception support and the compiler checkout's tsx loader.
import assert from "node:assert/strict";
import { writeFileSync } from "node:fs";
import { resolve, join } from "node:path";
import { pathToFileURL } from "node:url";
if (!process.argv[2]) throw new Error("usage: test-context-value-bridge.mjs JS2_CHECKOUT");
const { compile, compileMulti } = await import(pathToFileURL(join(resolve(process.argv[2]), "src/index.ts")).href);
import { CONTEXT_VALUE_BRIDGE_SOURCE, contextValueBridgeEntrypoints } from "./context-value-bridge.mjs";
const bootstrap = process.argv[4] === "bootstrap";
const bootstrapSource = bootstrap ? `
declare function __v8x_attach_context(): void;
__v8x_attach_context();
if ((globalThis as any).hostNumber !== 42) throw new Error("host seed missing during initialization");
(globalThis as any).initializedFromHost = (globalThis as any).hostCallback((globalThis as any).hostNumber);
if ((globalThis as any).bootFail) throw new Error("requested bootstrap failure");
` : "";
const applicationSource = `
(globalThis as any).identity = function (value: any): any { return value; };
(globalThis as any).sample = { answer: 42 };
(globalThis as any).values = [1, true, null];
(globalThis as any).useReceiver = function (this: any, delta: any): any { return this.answer + delta; };
(globalThis as any).throwFromRealm = function (): any { throw new Error("realm failure"); };
(globalThis as any).exerciseHost = function (host: any, leaf: any): number {
  const object = { value: 40 };
  const result = host.call(object, object, [1, 2, 3], function(value: any): any { return leaf(value); });
  if (result !== object || object.value !== 47) return -1;
  return 1;
};
(globalThis as any).exerciseThrow = function (host: any): number {
  let rejected = false;
  try { new host(); } catch (error) {
    rejected = error instanceof TypeError && error.message === "constructing a host callback is not implemented";
  }
  if (!rejected) return -2;
  try { host(); } catch (error) {
    return error instanceof TypeError && error.message === "host failure" ? 1 : -1;
  }
  return 0;
};
(globalThis as any).inspectHost = function (value: any): number {
  if (value.self !== value || value.left !== value.right) return -1;
  if (value.list[0] !== value.left || value.list[1] !== value) return -2;
  if (value.left.answer !== 7) return -3;
  const descriptor = Object.getOwnPropertyDescriptor(value, "fixed");
  if (!descriptor || descriptor.value !== 9 || descriptor.writable ||
      descriptor.enumerable || descriptor.configurable) return -4;
  if (!Object.prototype.hasOwnProperty.call(value, "__proto__") ||
      value.__proto__ !== 11) return -5;
  value.left.answer = 23;
  return 1;
};
`;
const options = { target: "standalone", platform: "deno", hostBridge: "always", deferTopLevelInit: bootstrap, externImportModule: "v8x:deno" };
const result = bootstrap ? await compileMulti({
  "/v8x-test/runtime-seed.ts": CONTEXT_VALUE_BRIDGE_SOURCE + "\ndeclare function __v8x_attach_context(): void;\n__v8x_attach_context();\n",
  "/v8x-test/core.ts": bootstrapSource.replace("declare function __v8x_attach_context(): void;\n__v8x_attach_context();", ""),
  "/v8x-test/entry.ts": 'import "./runtime-seed.ts";\nimport "./core.ts";\n' + applicationSource + contextValueBridgeEntrypoints("./runtime-seed.ts"),
}, "/v8x-test/entry.ts", options) : await compile(CONTEXT_VALUE_BRIDGE_SOURCE + applicationSource, options);
assert.equal(result.success, true, JSON.stringify(result.errors));
assert.deepEqual(WebAssembly.Module.imports(new WebAssembly.Module(result.binary)).map(i => i.name).sort(),
  (bootstrap ? ["__v8x_attach_context", "__v8x_host_call"] : ["__v8x_host_call"]).sort());
const {instance} = await WebAssembly.instantiate(result.binary, {"v8x:deno": {__v8x_attach_context() { e.__v8x_value_set(e.__v8x_value_global(),str("hostNumber"),e.__v8x_value_number(42)); e.__v8x_value_set(e.__v8x_value_global(),str("hostCallback"),e.__v8x_value_host_function(0)); }, __v8x_host_call(id, receiver, args) { if (!bootstrap || id !== 0) throw new Error("unexpected callback in scalar fixture"); return e.__v8x_value_number(e.__v8x_value_as_number(e.__v8x_value_get(args,str("0"))) + 1); }}});
const e = instance.exports;
const str = (s) => { let id=e.__v8x_value_string_empty(); for(let i=0;i<s.length;i++) id=e.__v8x_value_string_append(id,s.charCodeAt(i)); return id; };
if (bootstrap) e.__module_init();
const global = e.__v8x_value_global(), key=str("shared"), obj=e.__v8x_value_object();
e.__v8x_value_set(global,key,obj);
assert.equal(e.__v8x_value_get(global,key), obj);
const args=e.__v8x_value_array(); e.__v8x_value_set(args,str("0"),obj);
const fn=e.__v8x_value_get(global,str("identity"));
assert.equal(e.__v8x_value_call(fn,global,args),obj);
assert.equal(e.__v8x_value_kind(fn),6);
const text=str("Grüße 😀");
assert.equal(e.__v8x_value_utf16_length(text),8);
assert.equal(e.__v8x_value_utf16_unit(text,6),0xd83d);
assert.equal(e.__v8x_value_number(NaN),e.__v8x_value_number(NaN));
assert.notEqual(e.__v8x_value_number(0),e.__v8x_value_number(-0));
assert.throws(()=>e.__v8x_value_kind(-1));
assert.throws(()=>e.__v8x_value_as_number(obj));
if (process.argv[3]) writeFileSync(resolve(process.argv[3]), result.binary);
console.log("PASS: identity, callable result, UTF-16, NaN, signed zero, invalid handles");
