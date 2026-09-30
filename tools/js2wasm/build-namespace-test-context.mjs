// Copyright 2026 Loopdive GmbH. Licensed under Apache-2.0 WITH LLVM-exception.
import { writeFileSync } from "node:fs";
import { resolve, join } from "node:path";
import { pathToFileURL } from "node:url";
import { CONTEXT_VALUE_BRIDGE_SOURCE, contextPromiseRejectionDispatcherSource } from "./context-value-bridge.mjs";

if (!process.argv[2] || !process.argv[3]) {
  throw new Error("usage: build-namespace-test-context.mjs JS2_CHECKOUT OUTPUT_WASM");
}
const { compile } = await import(pathToFileURL(join(resolve(process.argv[2]), "src/index.ts")).href);
const eventFixture = process.argv[4];
if (eventFixture && !["--rejection-events", "--rejection-events-disabled"].includes(eventFixture)) throw new Error("unknown context fixture mode");
const fixture = eventFixture ? `
const rejectionMarker = { token: 42 };
(globalThis as any).__v8x_test_reason = rejectionMarker;
(globalThis as any).__v8x_test_reject = function(): any { return Promise.reject(rejectionMarker); };
(globalThis as any).__v8x_test_reject_and_handle = function(): any {
  const promise = Promise.reject(rejectionMarker);
  promise.catch(() => 42);
  return promise;
};
(globalThis as any).__v8x_test_attach = function(promise: any): any { return promise.catch(() => 42); };
` : "";
const dispatcher = eventFixture === "--rejection-events" ? contextPromiseRejectionDispatcherSource() : "";
const result = await compile(CONTEXT_VALUE_BRIDGE_SOURCE + dispatcher + fixture + `
export function __v8x_context_global_this(): any { return globalThis; }
export function __v8x_context_call(callable:any, receiver:any, args:any):any {
  return callable.apply(receiver,args);
}
export function __v8x_context_get(object:any, key:any, receiver:any):any {
  return Reflect.get(object,key,receiver);
}
`, { target:"standalone", platform:"deno", hostBridge:"always", externImportModule:"v8x:deno",
  standaloneAllocationOwnerExport:"__v8x_context_owns",
  link:["v8x:deno"],
  standaloneMicrotaskNotifyImport:{module:"v8x:deno",name:"__v8x_microtask_notify"},
});
if (!result.success) throw new Error(JSON.stringify(result.errors));
writeFileSync(resolve(process.argv[3]),result.binary);
console.log(`Built namespace test context: ${result.binary.byteLength} bytes`);
