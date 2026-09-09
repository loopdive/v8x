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
(globalThis as any).stringStorageCases = function(seed:any):any {
  let deep:any = seed;
  for (let i = 0; i < 128; i++) deep = deep + seed;
  return [seed.slice(1, -1), seed + seed, deep, ""];
};
(globalThis as any).coercionError = { marker: 73 };
(globalThis as any).coercionObject = {
  [Symbol.toPrimitive](hint:any):any {
    if (hint !== "number") throw new Error("wrong numeric hint");
    return "42.9";
  }
};
(globalThis as any).throwingCoercion = {
  valueOf():any { throw (globalThis as any).coercionError; }
};

const unnamedFunction:any = function():number { return 42; };
Object.defineProperty(unnamedFunction, "name", { value: undefined, configurable: true });
(globalThis as any).unnamedForHost = unnamedFunction;
const numericNameFunction:any = function():number { return 43; };
Object.defineProperty(numericNameFunction, "name", { value: 17, configurable: true });
(globalThis as any).numericNameForHost = numericNameFunction;
(globalThis as any).namedForHost = function namedCallback():number { return 44; };
(globalThis as any).exerciseSharedBuffer = function(host:any,view:any):number { view[0]=7; host(); return view[0]; };
(globalThis as any).throwSharedBuffer = function(view:any):void { view[0]=11; throw new Error("buffer throw"); };
(globalThis as any).identity = function (value: any): any { return value; };
(globalThis as any).registeredSymbol = Symbol.for("errorAdditionalPropertyKeys");
(globalThis as any).freshSymbolA = Symbol("same");
(globalThis as any).freshSymbolB = Symbol("same");
(globalThis as any).absentSymbol = Symbol();
(globalThis as any).emptySymbol = Symbol("");
(globalThis as any).iteratorSymbol = Symbol.iterator;
(globalThis as any).readErrorSymbol = function(value:any):any { return value[Symbol.for("errorAdditionalPropertyKeys")]; };
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
const proto=e.__v8x_value_object(), nil=e.__v8x_value_null();
assert.equal(e.__v8x_value_set_prototype(proto,nil),1);
assert.equal(e.__v8x_value_get_prototype(proto),nil);
assert.equal(e.__v8x_value_set_prototype(obj,proto),1);
assert.equal(e.__v8x_value_get_prototype(obj),proto);
e.__v8x_value_set(proto,str("inherited"),e.__v8x_value_number(73));
assert.equal(e.__v8x_value_as_number(e.__v8x_value_get(obj,str("inherited"))),73);
assert.equal(e.__v8x_value_set_prototype(proto,obj),0);
assert.equal(e.__v8x_value_get_prototype(proto),nil);
assert.throws(()=>e.__v8x_value_set_prototype(obj,e.__v8x_value_number(1)));
const text=str("Grüße 😀");
assert.equal(e.__v8x_value_utf16_length(text),8);
assert.equal(e.__v8x_value_utf16_unit(text,6),0xd83d);
assert.equal(e.__v8x_value_number(NaN),e.__v8x_value_number(NaN));
assert.notEqual(e.__v8x_value_number(0),e.__v8x_value_number(-0));
assert.throws(()=>e.__v8x_value_kind(-1));
assert.throws(()=>e.__v8x_value_as_number(obj));

const buffer=e.__v8x_value_buffer_create(16);
const u8=e.__v8x_value_typed_array(buffer,0,0,16);
const u32=e.__v8x_value_typed_array(buffer,2,4,2);
const i32=e.__v8x_value_typed_array(buffer,3,4,2);
assert.equal(e.__v8x_value_get(u8,str("buffer")),buffer);
assert.equal(e.__v8x_value_get(u32,str("buffer")),buffer);
assert.equal(e.__v8x_value_as_number(e.__v8x_value_get(u32,str("byteOffset"))),4);
e.__v8x_value_set(u32,str("0"),e.__v8x_value_number(0x12345678));
assert.deepEqual([4,5,6,7].map(i=>e.__v8x_value_as_number(e.__v8x_value_get(u8,str(String(i))))),[120,86,52,18]);
e.__v8x_value_set(u8,str("7"),e.__v8x_value_number(255));
assert.equal(e.__v8x_value_as_number(e.__v8x_value_get(u32,str("0"))),4281620088);
assert.equal(e.__v8x_value_as_number(e.__v8x_value_get(i32,str("0"))),-13347208);
for(const kind of [1,4,5]) {
 const view=e.__v8x_value_typed_array(buffer,kind,0,1);
 assert.equal(e.__v8x_value_get(view,str("buffer")),buffer);
}
assert.throws(()=>e.__v8x_value_buffer_storage(obj));
assert.throws(()=>e.__v8x_value_buffer_create(-1));
assert.throws(()=>e.__v8x_value_typed_array(buffer,99,0,1));
assert.throws(()=>e.__v8x_value_typed_array(buffer,2,1,1));
assert.throws(()=>e.__v8x_value_typed_array(buffer,2,12,2));
console.log("PASS: fixed host buffer handles, shared overlapping views, view bounds and kind rejection");

console.log("PASS: identity, prototype updates and refusals, callable result, UTF-16, NaN, signed zero, invalid handles");

function numericEnvelope(handle) {
  const envelope=e.__v8x_value_to_number(handle);
  return [e.__v8x_value_as_boolean(e.__v8x_value_get(envelope,str("0"))),
    e.__v8x_value_get(envelope,str("1"))];
}
for (const [input, expected] of [["",0],[" 42.9 ",42.9],["0x10",16],["0b11",3],["no",NaN]]) {
  const [ok,value]=numericEnvelope(str(input));
  assert.equal(ok,1,input);
  assert.equal(e.__v8x_value_as_number(value),expected,input);
}
const [coercedOk,coerced]=numericEnvelope(e.__v8x_value_get(global,str("coercionObject")));
assert.equal(coercedOk,1);
assert.equal(e.__v8x_value_as_number(coerced),42.9);
const [threw,exception]=numericEnvelope(e.__v8x_value_get(global,str("throwingCoercion")));
assert.equal(threw,0);
assert.equal(exception,e.__v8x_value_get(global,str("coercionError")));

console.log("PASS: numeric strings, number-hint coercion, and original exception identity");
const positiveZero=e.__v8x_value_number(0), negativeZero=e.__v8x_value_number(-0);
const identities=[];
for(let i=0;i<512;i++) identities.push(e.__v8x_value_object());
assert.equal(new Set(identities).size,512);
for(let i=0;i<512;i++) {
  const n=e.__v8x_value_number(i+1000);
  assert.equal(e.__v8x_value_number(i+1000),n);
}
assert.equal(e.__v8x_value_number(-0),negativeZero);
assert.equal(e.__v8x_value_number(0),positiveZero);
assert.notEqual(positiveZero,negativeZero);
assert.equal(e.__v8x_value_as_number(positiveZero),0);
assert.ok(Object.is(e.__v8x_value_as_number(negativeZero),-0));
const repeatedPacket=e.__v8x_value_buffer_create(0);
assert.equal(e.__v8x_value_string_from_buffer(repeatedPacket),str(""));
assert.throws(()=>e.__v8x_value_buffer_storage(repeatedPacket));
assert.equal(e.__v8x_value_string_from_buffer(e.__v8x_value_buffer_create(0)),str(""));
console.log("PASS: indexed handle growth, signed-zero stability, and packet retirement");
const transientIds = new Set();
for (let i = 0; i < 512; i++) {
  const packet = e.__v8x_value_packet_create(0);
  assert.ok(!transientIds.has(packet));
  transientIds.add(packet);
  assert.equal(e.__v8x_value_string_from_buffer(packet), str(""));
  assert.throws(() => e.__v8x_value_buffer_storage(packet));
  assert.throws(() => e.__v8x_value_string_from_buffer(packet));
}
for (const length of [-1, 0.5, NaN, Infinity, 2147483648])
  assert.throws(() => e.__v8x_value_packet_create(length));
const persistent = e.__v8x_value_buffer_create(16);
const view = e.__v8x_value_typed_array(persistent, 0, 0, 16);
assert.equal(e.__v8x_value_get(view, str("buffer")), persistent);
assert.equal(e.__v8x_value_get(view, str("buffer")), persistent);
console.log("PASS: transient packet retirement and persistent buffer identity");
assert.equal(e.__v8x_value_kind(-0), 0);
const integerKey = str("integer-validation");
for (const invalid of [-1, 0.5, NaN, Infinity, -Infinity, Number.MAX_SAFE_INTEGER]) {
  assert.throws(() => e.__v8x_value_kind(invalid));
  assert.throws(() => e.__v8x_value_buffer_create(invalid));
  assert.throws(() => e.__v8x_value_packet_create(invalid));
  assert.throws(() => e.__v8x_value_typed_array(persistent, 0, invalid, 0));
  assert.throws(() => e.__v8x_value_typed_array(persistent, 0, 0, invalid));
  assert.throws(() => e.__v8x_value_define_data(obj, integerKey, positiveZero, invalid));
}
assert.equal(e.__v8x_value_string_from_buffer(e.__v8x_value_packet_create(-0)), str(""));
assert.equal(e.__v8x_value_get(e.__v8x_value_typed_array(persistent, 0, -0, 0), str("buffer")), persistent);
for (let flags = 0; flags <= 7; flags++)
  e.__v8x_value_define_data(e.__v8x_value_object(), integerKey, positiveZero, flags);
assert.throws(() => e.__v8x_value_define_data(obj, integerKey, positiveZero, 8));
console.log("PASS: integer validation, NaN/infinities/signed zero, and descriptor flags");
if (process.argv[3]) writeFileSync(resolve(process.argv[3]), result.binary);
