import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";

const path = process.env.DENO_AOT_WASM;
if (!path) throw new Error("DENO_AOT_WASM must name the generated AOT core");
const module = new WebAssembly.Module(readFileSync(path));
test("AOT core has native host imports only, with no provider or memory", () => {
  const imports = WebAssembly.Module.imports(module);
  assert.equal(imports.length, 16);
  for (const entry of imports) {
    assert.equal(entry.module, "v8x:deno");
    assert.equal(entry.kind, "function");
  }
});
test("compiled unknown-script path refuses without invoking native ops", () => {
  const source = "throw 123";
  let opCalls = 0;
  const host = Object.fromEntries(WebAssembly.Module.imports(module).map(entry => [entry.name, () => { opCalls++; return 0; }]));
  host.__v8x_deno_script_utf16_length = () => source.length;
  host.__v8x_deno_script_utf16_code_unit = index => source.charCodeAt(index);
  host.__v8x_attach_context = () => 0;
  const { exports: e } = new WebAssembly.Instance(module, { "v8x:deno": host });
  e.__module_init();
  assert.equal(e.__v8x_run_classic_script(), -1);
  const length = e.__v8x_script_result_utf16_length();
  assert.ok(length > 0 && length < 4096);
  let json = "";
  for (let i = 0; i < length; i++) json += String.fromCharCode(e.__v8x_script_result_utf16_code_unit(i));
  assert.match(JSON.parse(json).message, /rejects unknown script/);
  assert.equal(opCalls, 0);
});
