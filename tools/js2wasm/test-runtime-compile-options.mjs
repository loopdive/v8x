// Copyright 2026 Loopdive GmbH. Licensed under Apache-2.0 WITH LLVM-exception.
import assert from "node:assert/strict";
import { test } from "node:test";
import { COMPILE_OPTIONS, runtimeCompileOptions, assertRuntimeSchedulerABI, assertRuntimeAllocationOwnerABI, compilerRefForProfile } from "./build-deno-core-artifact.mjs";
import { contextPromiseRejectionDispatcherSource, contextScriptCompletionSource } from "./context-value-bridge.mjs";

test("Script completion roots native references through the owning keeper", () => {
  const source = contextScriptCompletionSource("imported__v8x_value_keep");
  assert(source.includes("__v8x_context_script_completion(value: any): void"));
  assert(source.includes("__v8xScriptCompletion = undefined"));
  assert(source.includes("return imported__v8x_value_keep(__v8xScriptCompletion)"));
  assert(!source.includes("JSON"));
  assert.throws(() => contextScriptCompletionSource("keeper(); injected()"), /invalid completion keeper/);
});

test("event dispatcher roots both values through the supplied realm keeper", () => {
  const source = contextPromiseRejectionDispatcherSource("imported__v8x_value_keep");
  assert(source.includes("imported__v8x_value_keep(promise), imported__v8x_value_keep(reason)"));
  assert(source.includes("__v8x_deno_promise_reject_dispatch(event: number, promise: any, reason: any): void"));
  assert.throws(() => contextPromiseRejectionDispatcherSource("keeper(); injected()"), /invalid rejection root keeper/);
});

test("runtime compiler pin advances independently of the historical POC", () => {
  assert.equal(compilerRefForProfile("poc"), "8fd489a918dee3be51bb1e75d191f9815a830eb0");
  assert.equal(compilerRefForProfile("runtime"), "c98082082b163165ed6c5ba7f726c24d01ee1ba7");
  assert.throws(() => compilerRefForProfile("unknown"), /unknown compiler profile/);
});

test("historical POC options remain unchanged", () => {
  assert.deepEqual(COMPILE_OPTIONS, {
    target: "standalone", platform: "deno", externImportModule: "v8x:deno",
    allowJs: true, skipSemanticDiagnostics: true, deferTopLevelInit: true,
  });
  assert(Object.isFrozen(COMPILE_OPTIONS));
});
test("unstamped or uninspectable modules cannot pass the owner ABI gate", () => {
  const empty = new WebAssembly.Module(Uint8Array.of(0,97,115,109,1,0,0,0));
  assert.throws(() => assertRuntimeAllocationOwnerABI(empty), /lacks allocation-owner function __v8x_context_owns/);
  assert.throws(() => assertRuntimeAllocationOwnerABI({}), TypeError);
});
test("AOT runtime links only native scheduler capabilities and exports Symbol state", () => {
  const options = runtimeCompileOptions("aot");
  assert.deepEqual(options.link, ["v8x:deno"]);
  assert.deepEqual(options.standaloneMicrotaskNotifyImport, { module: "v8x:deno", name: "__v8x_microtask_notify" });
  assert.equal(options.standaloneSymbolState, "export");
  assert.equal(options.standaloneAllocationOwnerExport, "__v8x_context_owns");
  assert.equal(options.deferTopLevelInit, true);
});
test("explicit dynamic fallback retains the scheduler and shared provider state", () => {
  const options = runtimeCompileOptions("dynamic");
  assert.deepEqual(options.link, ["v8x:deno", "js2wasm:runtime-eval"]);
  assert.deepEqual(options.standaloneSymbolState, { module: "js2wasm:runtime-eval", reexport: true });
  assert.equal(options.standaloneMicrotaskNotifyImport.name, "__v8x_microtask_notify");
});
test("unknown modes refuse and callers cannot mutate subsequent build options", () => {
  assert.throws(() => runtimeCompileOptions("unknown"), /unknown runtime execution mode/);
  const options = runtimeCompileOptions();
  options.link.push("unreviewed-provider");
  options.standaloneMicrotaskNotifyImport.name = "wrong";
  assert.deepEqual(runtimeCompileOptions().link, ["v8x:deno"]);
  assert.equal(runtimeCompileOptions().standaloneMicrotaskNotifyImport.name, "__v8x_microtask_notify");
});
test("an empty or uninspectable module cannot pass the runtime scheduler gate", () => {
  const empty = new WebAssembly.Module(Uint8Array.of(0,97,115,109,1,0,0,0));
  assert.throws(() => assertRuntimeSchedulerABI(empty), /exactly one native microtask notification/);
  assert.throws(() => assertRuntimeSchedulerABI({}), TypeError);
});
test("a notification import alone does not prove a drainable queue", () => {
  const string = value => { const bytes = [...new TextEncoder().encode(value)]; return [bytes.length, ...bytes]; };
  const imports = [1, ...string("v8x:deno"), ...string("__v8x_microtask_notify"), 0, 0];
  const bytes = Uint8Array.from([0,97,115,109,1,0,0,0, 1,4,1,0x60,0,0, 2,imports.length,...imports]);
  const module = new WebAssembly.Module(bytes);
  assert.throws(() => assertRuntimeSchedulerABI(module), /lacks scheduler function __drain_one_microtask/);
});
