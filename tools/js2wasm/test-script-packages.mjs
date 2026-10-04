import assert from "node:assert/strict";
import { test } from "node:test";
import { scriptDigest, scriptCompileOptions, assertScriptABI } from "./script-packages.mjs";

test("Script binding includes exact source, specifier and length framing", () => {
  assert.equal(scriptDigest("a", "bc"), scriptDigest("a", "bc"));
  assert.notEqual(scriptDigest("a", "bc"), scriptDigest("ab", "c"));
  assert.notEqual(scriptDigest("a", "bc"), scriptDigest("b", "bc"));
  assert.notEqual(scriptDigest("a", "bc"), scriptDigest("a", "bd"));
  assert.match(scriptDigest("é", "42;"), /^[a-f0-9]{64}$/);
  assert.equal(scriptDigest("é", "42;"), "5a142fa31a2cc229f816531945f3e10a387ed44e968dcf7e80eff85e36a964b5");
});
test("trusted builder selects Script goal and native completion without dynamic fallback", () => {
  const options = scriptCompileOptions("file:///main.js");
  assert.equal(options.scriptGoal, true);
  assert.deepEqual(options.standaloneSymbolState, { module: "v8x:context" });
  assert.equal(options.deferTopLevelInit, true);
  assert.equal(options.standaloneScriptVarBindings, true);
  assert.equal(options.fileName, "script.ts");
  assert.equal(options.standaloneScriptCompletionImport.name, "__v8x_context_script_completion");
  assert.equal(options.standaloneScriptGetExport, "__v8x_script_get_export");
  assert.equal(options.standaloneScriptCallExport, "__v8x_script_call_export");
  assert.equal(options.standaloneScriptOwnNamesExport, "__v8x_script_own_names_export");
  assert.deepEqual(options.standaloneScriptReflectionExports, {ownSymbols: "__v8x_script_own_symbols_export", descriptor: "__v8x_script_descriptor_export"});
  assert.equal(options.standaloneGlobalThisImport.arrayPrototype, "__v8x_context_array_prototype");
  options.link.push("unexpected");
  assert.deepEqual(scriptCompileOptions("x").link, ["v8x:context"]);
});
test("empty and uninspectable artifacts cannot satisfy Script ABI", () => {
  const empty = new WebAssembly.Module(Uint8Array.of(0, 97, 115, 109, 1, 0, 0, 0));
  assert.throws(() => assertScriptABI(empty), /exactly one native completion sink/);
  assert.throws(() => assertScriptABI({}), TypeError);
});
