// Build compact, independently compiled modules for Rust callback-lifetime tests.
import assert from "node:assert/strict";
import { mkdirSync, writeFileSync } from "node:fs";
import { resolve, join } from "node:path";
import { pathToFileURL } from "node:url";
import { CONTEXT_VALUE_BRIDGE_SOURCE } from "./context-value-bridge.mjs";

const [compilerPath, outputPath] = process.argv.slice(2);
if (!compilerPath || !outputPath) throw new Error("usage: test-linked-callbacks.mjs JS2_CHECKOUT OUTPUT_DIR");
const { compile } = await import(pathToFileURL(join(resolve(compilerPath), "src/index.ts")).href);
const options = { target: "standalone", platform: "deno", hostBridge: "always", deferTopLevelInit: true, externImportModule: "v8x:deno", fileName: "fixture.ts" };
const linked = { ...options, standaloneGlobalThisImport: { module: "v8x:context", name: "__v8x_context_global_this", call: "__v8x_context_call" }, link: ["v8x:context"] };
mkdirSync(resolve(outputPath), { recursive: true });
for (const [name, source, config] of [
  ["context", CONTEXT_VALUE_BRIDGE_SOURCE + `
    export function __v8x_context_global_this():any {return globalThis;}
    export function __v8x_context_call(f:any, receiver:any, args:any):any {return f.apply(receiver,args);}
  `, options],
  ["producer", `
    let counter:any=40;
    (globalThis as any).deferredCallback=function foreignCallback(delta:any):any {
      counter += delta;
      return counter;
    };
  `, linked],
  ["replacement", `(globalThis as any).deferredCallback=function replacement():any {return 99;};`, linked],
]) {
  const result = await compile(source, config);
  assert.equal(result.success, true, JSON.stringify(result.errors));
  assert.equal(WebAssembly.validate(result.binary), true);
  writeFileSync(join(resolve(outputPath), name + ".wasm"), result.binary);
  console.log(`compiled ${name}: ${result.binary.length} bytes`);
}
