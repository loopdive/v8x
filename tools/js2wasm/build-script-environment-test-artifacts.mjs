// Copyright 2026 Loopdive GmbH. Licensed under Apache-2.0 WITH LLVM-exception.
// Development fixtures, not production source-bound deployment packages.
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { pathToFileURL } from "node:url";
import { CONTEXT_VALUE_BRIDGE_SOURCE, contextScriptCompletionSource } from "./context-value-bridge.mjs";

const [compilerPath, outputPath] = process.argv.slice(2);
if (!compilerPath || !outputPath) throw new Error("usage: build-script-environment-test-artifacts.mjs JS2_CHECKOUT OUTPUT_DIR");
const compiler = resolve(compilerPath);
const output = resolve(outputPath);
mkdirSync(output, { recursive: true });
const { compile } = await import(pathToFileURL(join(compiler, "src/index.ts")).href);
const provider = readFileSync(join(compiler, "examples/v8x-js2wasm-spike/script-lexical-provider.ts"), "utf8");
const contextSource = provider + CONTEXT_VALUE_BRIDGE_SOURCE + contextScriptCompletionSource() + `
export function __v8x_probe_completion_number():number {return __v8x_value_as_number(__v8x_script_completion_handle());}
export function __v8x_probe_completion_identity():boolean {return __v8x_value_unwrap(__v8x_script_completion_handle())===globalThis.completionSaved;}
globalThis.score=0;
globalThis.published=0;
export function __v8x_context_global_this():any {return globalThis;}
export function __v8x_context_get(o:any,k:any,r:any):any {return Reflect.get(o,k,r);}
export function __v8x_context_array_prototype():any {return Array.prototype;}
export function __v8x_context_call(f:any,r:any,a:any):any {return f.apply(r,a);}
export function __v8x_context_lexical(n:any,op:number,v:any):any {return scriptLexicalOperation(n,op,v);}
export function __v8x_probe_script_score():number {return Number(globalThis.score);}
export function __v8x_probe_script_observed():number {return Number(globalThis.published);}
export function __v8x_probe_script_has_retained():boolean {return Boolean(scriptLexicalOperation("retained",9,undefined));}
export function __v8x_probe_script_has_first():boolean {return Boolean(scriptLexicalOperation("first",9,undefined));}
export function __v8x_probe_script_fixed():number {return Number(scriptLexicalOperation("fixed",5,undefined));}
export function __v8x_probe_script_caught():number {return globalThis.caught.name==="TypeError"?42:0;}
`;
const scripts = [
  "globalThis.score=0; let retained:any={valueOf(){globalThis.score++;return 18446744073709551616n;}};",
  "globalThis.saved=retained++;",
  'globalThis.published=String(retained)==="18446744073709551617" && String(globalThis.saved)==="18446744073709551616"?42:0;',
  "let first:any=1; let retained:any=0; globalThis.published=0;",
  "const fixed=41;",
  "try {fixed=42;} catch(error){globalThis.caught=error;}",
  "const active=true; globalThis.reader=()=>{globalThis.published=active?43:0;};",
  "globalThis.alias=globalThis.reader; globalThis.alias();",
  "41;42;",
  "void 0;",
  "globalThis.completionSaved={marker:42}; globalThis.completionSaved;",
];
const records = [];
async function build(name, source, options) {
  const result = await compile(source, options);
  assert.equal(result.success, true, JSON.stringify(result.errors));
  const module = new WebAssembly.Module(result.binary);
  const imports = WebAssembly.Module.imports(module);
  assert(imports.every(item => item.module === "v8x:context" || (item.module === "v8x:deno" && item.name === "__v8x_host_call")), JSON.stringify(imports));
  writeFileSync(join(output, name + ".wasm"), result.binary);
  records.push({ name, source, sourceSha256: createHash("sha256").update(source).digest("hex"),
    wasmSha256: createHash("sha256").update(result.binary).digest("hex"), bytes: result.binary.byteLength, imports });
  return { result, module };
}
const context = await build("context", contextSource, {
  target: "standalone", platform:"deno", hostBridge:"always", externImportModule:"v8x:deno", standaloneAllocationOwnerExport: "__v8x_context_owns",
});
const owner = new WebAssembly.Instance(context.module, {"v8x:deno": {__v8x_host_call(){throw new Error("unexpected host callback in completion fixture");}}});
const compiled = [];
for (let index = 0; index < scripts.length; index++) {
  compiled.push(await build(`script-${index}`, scripts[index], {
    target: "standalone", scriptGoal: true, allowJs: true, fileName: "script.ts",
    hostBridge: "always", deferTopLevelInit: true, standaloneScriptVarBindings: true,
    standaloneScriptLexicalImport: { module: "v8x:context", name: "__v8x_context_lexical" },
    standaloneScriptCompletionImport: { module: "v8x:context", name: "__v8x_context_script_completion" },
    standaloneAllocationOwnerExport: "localOwns",
    standaloneGlobalThisImport: { module: "v8x:context", name: "__v8x_context_global_this",
      owns: "__v8x_context_owns", get: "__v8x_context_get",
      arrayPrototype: "__v8x_context_array_prototype", exceptionTag: "__exn_tag" },
    link: ["v8x:context"],
  }));
}
function run(index) {
  const instance = new WebAssembly.Instance(compiled[index].module, { "v8x:context": owner.exports });
  owner.exports.__v8x_script_completion_reset();
  instance.exports.__module_init();
}
run(0); run(1); run(2);
assert.equal(owner.exports.__v8x_probe_script_score(), 1);
assert.equal(owner.exports.__v8x_probe_script_observed(), 42);
assert.throws(() => run(3), WebAssembly.Exception);
assert.equal(owner.exports.__v8x_probe_script_has_first(), 0);
assert.equal(owner.exports.__v8x_probe_script_observed(), 42);
run(4); run(5);
assert.equal(owner.exports.__v8x_probe_script_fixed(), 41);
assert.equal(owner.exports.__v8x_probe_script_caught(), 42);
run(6); run(7);
assert.equal(owner.exports.__v8x_probe_script_observed(), 43);
run(8);
assert.equal(owner.exports.__v8x_probe_completion_number(),42);
run(9);
assert.equal(owner.exports.__v8x_script_completion_handle(),0);
run(10);
assert.equal(owner.exports.__v8x_probe_completion_identity(),1);
writeFileSync(join(output, "test-inputs.json"), JSON.stringify({
  kind: "local-native-script-environment-test-not-production-package", compiler, records,
}, null, 2) + "\n");
console.log(JSON.stringify({ artifacts: records.length, score: 1, observed: 42, fixed: 41 }));
