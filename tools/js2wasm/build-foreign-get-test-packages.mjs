import assert from "node:assert/strict";
import { packageScript } from "./script-packages.mjs";
import { packageFunction } from "./compiled-function-packages.mjs";

const [compiler, precompiler, output] = process.argv.slice(2);
assert(output, "usage: build-foreign-get-test-packages.mjs JS2 PACKAGER OUTPUT");
const callsOnly = process.argv.includes("--calls-only");
const getSources = [
  "globalThis.reentrantScriptHost(4).foo;",
  "try {globalThis.reentrantScriptHost(5,globalThis.completionSaved).foo;}catch(error){error;}",
  "globalThis.reentrantScriptHost(6).foo;",
  "const foreign=globalThis.reentrantScriptHost(7);foreign.foo===foreign;",
];
const callSources = [
  "globalThis.reentrantScriptHost(8).foo(2);",
  "try {globalThis.reentrantScriptHost(5,globalThis.completionSaved).foo((globalThis.shouldNotRun=1));}catch(error){error;}",
  'Reflect.get(globalThis.reentrantScriptHost(7),"foo",{});',
];
for (const source of [...(callsOnly ? [] : getSources), ...callSources]) {
  console.log(await packageScript(compiler, precompiler, "<anonymous>", source, output));
}
const getBodies = [
  "return { foo: 'foo', bar: 123 };",
  "return {get foo(){throw value;}};",
  "return {get foo(){return undefined;}};",
  "return {get foo(){return this;}};",
];
for (const body of [...(callsOnly ? [] : getBodies),
  "return {foo(value){return this.marker + value;}, marker:40};",
]) {
  console.log(await packageFunction(compiler, precompiler, "<anonymous>", body, ["value"], output));
}
