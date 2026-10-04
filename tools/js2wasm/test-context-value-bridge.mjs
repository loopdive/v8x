// Run with Node Wasm exception support and the compiler checkout's tsx loader.
import assert from "node:assert/strict";
import { writeFileSync } from "node:fs";
import { resolve, join } from "node:path";
import { pathToFileURL } from "node:url";
if (!process.argv[2]) throw new Error("usage: test-context-value-bridge.mjs JS2_CHECKOUT");
const { compile, compileMulti } = await import(pathToFileURL(join(resolve(process.argv[2]), "src/index.ts")).href);
import { CONTEXT_VALUE_BRIDGE_SOURCE, contextValueBridgeEntrypoints } from "./context-value-bridge.mjs";
const bootstrap = process.argv[4] === "bootstrap";
const allocationOwner = process.argv.includes("--allocation-owner");
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
(globalThis as any).stringHintObject = {
  [Symbol.toPrimitive](hint:any):any {
    if (hint !== "string") throw new Error("wrong string hint");
    return "string hint";
  }
};
(globalThis as any).throwingStringCoercion = {
  toString():any { throw (globalThis as any).coercionError; }
};
(globalThis as any).symbolStringCoercion = {
  [Symbol.toPrimitive](hint:any):any {
    if (hint !== "string") throw new Error("wrong string hint");
    return Symbol("coercion result");
  }
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
(globalThis as any).liveExportGetter = function (): any { return (globalThis as any).liveExportSlot; };
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
if (allocationOwner) options.standaloneAllocationOwnerExport = "__v8x_context_owns";
const result = bootstrap ? await compileMulti({
  "/v8x-test/runtime-seed.ts": CONTEXT_VALUE_BRIDGE_SOURCE + "\ndeclare function __v8x_attach_context(): void;\n__v8x_attach_context();\n",
  "/v8x-test/core.ts": bootstrapSource.replace("declare function __v8x_attach_context(): void;\n__v8x_attach_context();", ""),
  "/v8x-test/entry.ts": 'import "./runtime-seed.ts";\nimport "./core.ts";\n' + applicationSource + contextValueBridgeEntrypoints("./runtime-seed.ts"),
}, "/v8x-test/entry.ts", options) : await compile(CONTEXT_VALUE_BRIDGE_SOURCE + applicationSource, options);
assert.equal(result.success, true, JSON.stringify(result.errors));
assert.deepEqual(WebAssembly.Module.imports(new WebAssembly.Module(result.binary)).map(i => i.name).sort(),
  (bootstrap ? ["__v8x_attach_context", "__v8x_host_call"] : ["__v8x_host_call"]).sort());
let activeHostCall;
const {instance} = await WebAssembly.instantiate(result.binary, {"v8x:deno": {__v8x_attach_context() { e.__v8x_value_set(e.__v8x_value_global(),str("hostNumber"),e.__v8x_value_number(42)); e.__v8x_value_set(e.__v8x_value_global(),str("hostCallback"),e.__v8x_value_host_function(0)); }, __v8x_host_call(id, receiver, args) { if (activeHostCall) return activeHostCall(id,receiver,args); if (!bootstrap || id !== 0) throw new Error("unexpected callback in scalar fixture"); return e.__v8x_value_number(e.__v8x_value_as_number(e.__v8x_value_get(args,str("0"))) + 1); }}});
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
const decode = handle => {
  let result = "";
  for (let i=0;i<e.__v8x_value_utf16_length(handle);i++) result += String.fromCharCode(e.__v8x_value_utf16_unit(handle,i));
  return result;
};
for (const [value,expected] of [
  [e.__v8x_value_error(str("Error"),str("realm failure")),"Error: realm failure"],
  [e.__v8x_value_get(global,str("stringHintObject")),"string hint"],
]) {
  const envelope=e.__v8x_value_to_string(value);
  assert.equal(e.__v8x_value_as_boolean(e.__v8x_value_get(envelope,str("0"))),1);
  assert.equal(decode(e.__v8x_value_get(envelope,str("1"))),expected);
}
const stringFailure=e.__v8x_value_to_string(e.__v8x_value_get(global,str("throwingStringCoercion")));
assert.equal(e.__v8x_value_as_boolean(e.__v8x_value_get(stringFailure,str("0"))),0);
assert.equal(e.__v8x_value_get(stringFailure,str("1")),e.__v8x_value_get(global,str("coercionError")));
for (const symbol of [e.__v8x_value_symbol_create(0,str("x")),e.__v8x_value_get(global,str("symbolStringCoercion"))]) {
  const symbolFailure=e.__v8x_value_to_string(symbol);
  assert.equal(e.__v8x_value_as_boolean(e.__v8x_value_get(symbolFailure,str("0"))),0);
  assert.equal(decode(e.__v8x_value_get(e.__v8x_value_get(symbolFailure,str("1")),str("name"))),"TypeError");
}
console.log("PASS: realm ToString preserves errors, string hints, thrown identity and Symbol refusal");
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

function numericEnvelope(handle, label) {
  const checked = (stage, operation) => {
    try { return operation(); }
    catch (cause) { throw new Error(`numeric envelope ${label}: ${stage}`, { cause }); }
  };
  const envelope=checked("ToNumber", () => e.__v8x_value_to_number(handle));
  const ok=checked("status property", () => e.__v8x_value_get(envelope,str("0")));
  return [checked("boolean status", () => e.__v8x_value_as_boolean(ok)),
    checked("result property", () => e.__v8x_value_get(envelope,str("1")))];
}
for (const [input, expected] of [["",0],[" 42.9 ",42.9],["0x10",16],["0b11",3],["no",NaN]]) {
  const [ok,value]=numericEnvelope(str(input), JSON.stringify(input));
  assert.equal(ok,1,input);
  assert.equal(e.__v8x_value_as_number(value),expected,input);
}
const [coercedOk,coerced]=numericEnvelope(e.__v8x_value_get(global,str("coercionObject")), "coercionObject");
assert.equal(coercedOk,1);
assert.equal(e.__v8x_value_as_number(coerced),42.9);
const [threw,exception]=numericEnvelope(e.__v8x_value_get(global,str("throwingCoercion")), "throwingCoercion");
assert.equal(threw,0);
assert.equal(exception,e.__v8x_value_get(global,str("coercionError")));

console.log("PASS: numeric strings, number-hint coercion, and original exception identity");
const positiveZero=e.__v8x_value_number(0), negativeZero=e.__v8x_value_number(-0);
assert.equal(e.__v8x_value_number(-0), negativeZero, "negative zero before handle growth");
const identities=[];
for(let i=0;i<512;i++) identities.push(e.__v8x_value_object());
assert.equal(new Set(identities).size,512);
assert.equal(e.__v8x_value_number(-0), negativeZero, "negative zero after object handle growth");
for(let i=0;i<512;i++) {
  const n=e.__v8x_value_number(i+1000);
  assert.equal(e.__v8x_value_number(i+1000),n);
  assert.equal(e.__v8x_value_number(-0),negativeZero, `negative zero after number ${i+1000}`);
}
assert.equal(e.__v8x_value_number(-0),negativeZero);
assert.equal(e.__v8x_value_number(0),positiveZero);
assert.notEqual(positiveZero,negativeZero);
assert.equal(e.__v8x_value_as_number(positiveZero),0);
assert.ok(Object.is(e.__v8x_value_as_number(negativeZero),-0));
const repeatedPacket=e.__v8x_value_buffer_create(0);
const decodedEmpty=e.__v8x_value_string_from_buffer(repeatedPacket);
assert.equal(e.__v8x_value_kind(decodedEmpty),4,"decoded packet is a string");
assert.equal(e.__v8x_value_utf16_length(decodedEmpty),0,"decoded packet is empty");
assert.equal(decodedEmpty,str(""));
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
const liveObject = e.__v8x_value_object(), liveKey = str("live-export");
const originalExport = e.__v8x_value_object(), replacementExport = e.__v8x_value_object();
let currentExport = originalExport, getterCalls = 0;
activeHostCall = (id, receiver, args) => {
  assert.equal(id, 2001);
  assert.equal(receiver, liveObject);
  assert.equal(e.__v8x_value_as_number(e.__v8x_value_get(args, str("length"))), 0);
  getterCalls++;
  return currentExport;
};
const liveGetter = e.__v8x_value_host_function(2001);
e.__v8x_value_define_getter(liveObject, liveKey, liveGetter, 4);
assert.equal(getterCalls, 0, "defining a getter must not read the export");
assert.equal(e.__v8x_value_get(liveObject, liveKey), originalExport);
currentExport = replacementExport;
assert.equal(e.__v8x_value_get(liveObject, liveKey), replacementExport);
const liveDescriptor = e.__v8x_value_descriptor(liveObject, liveKey);
assert.equal(e.__v8x_value_get(liveDescriptor, str("get")), liveGetter);
assert.equal(e.__v8x_value_kind(e.__v8x_value_get(liveDescriptor, str("set"))), 0);
assert.equal(e.__v8x_value_as_boolean(e.__v8x_value_get(liveDescriptor, str("enumerable"))), 1);
assert.equal(e.__v8x_value_as_boolean(e.__v8x_value_get(liveDescriptor, str("configurable"))), 0);
assert.throws(() => e.__v8x_value_set(liveObject, liveKey, originalExport));
assert.throws(() => e.__v8x_value_define_data(liveObject, liveKey, originalExport, 0));
assert.equal(e.__v8x_value_get(liveObject, liveKey), replacementExport);
for (const invalid of [-1, 1, 3, 5, 7, 8, 0.5, NaN, Infinity])
  assert.throws(() => e.__v8x_value_define_getter(e.__v8x_value_object(), liveKey, liveGetter, invalid));
assert.throws(() => e.__v8x_value_define_getter(e.__v8x_value_object(), liveKey, originalExport, 4));
assert.equal(getterCalls, 3);
activeHostCall = undefined;
console.log("PASS: live getter identity, deferred reads, descriptor flags and write protection");
activeHostCall = (id, receiver, args) => {
  const arg = i => e.__v8x_value_get(args,str(String(i)));
  if (id === 1002) return e.__v8x_value_number(e.__v8x_value_as_number(arg(0))+1);
  assert.equal(id,1001);
  const object = arg(0);
  assert.equal(receiver,object,"callback receiver and argument zero share a handle");
  const array = arg(1);
  const sum = [0,1,2].reduce((sum,i)=>sum+e.__v8x_value_as_number(e.__v8x_value_get(array,str(String(i)))),0);
  const next=e.__v8x_value_as_number(e.__v8x_value_get(object,str("value")))+sum;
  const nestedArgs=e.__v8x_value_array();
  e.__v8x_value_set(nestedArgs,str("0"),e.__v8x_value_number(next));
  const final=e.__v8x_value_call(arg(2),global,nestedArgs);
  e.__v8x_value_set(object,str("value"),final);
  return object;
};
const callbackArgs=e.__v8x_value_array();
e.__v8x_value_set(callbackArgs,str("0"),e.__v8x_value_host_function(1001));
e.__v8x_value_set(callbackArgs,str("1"),e.__v8x_value_host_function(1002));
const callbackResult=e.__v8x_value_call(e.__v8x_value_get(global,str("exerciseHost")),global,callbackArgs);
assert.equal(e.__v8x_value_as_number(callbackResult),1,"nested host callback result");
activeHostCall=undefined;
console.log("PASS: exact host callback receiver identity and nested reentry");
activeHostCall = (id) => {
  assert.equal(id,1003);
  const error=e.__v8x_value_error(str("TypeError"),str("host failure"));
  return -error-1;
};
const throwArgs=e.__v8x_value_array();
e.__v8x_value_set(throwArgs,str("0"),e.__v8x_value_host_function(1003));
const throwResult=e.__v8x_value_call(e.__v8x_value_get(global,str("exerciseThrow")),global,throwArgs);
assert.equal(e.__v8x_value_as_number(throwResult),1,"construct rejection and caught host exception");
activeHostCall=undefined;
console.log("PASS: host construction rejection and caught exception");
if (process.argv[3]) writeFileSync(resolve(process.argv[3]), result.binary);
