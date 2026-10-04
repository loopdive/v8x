import { packageScript } from "./script-packages.mjs";
import assert from "node:assert/strict";

export function validateFunctionBody(body, parameters) {
  assert.equal(typeof body, "string");
  assert(Array.isArray(parameters));
  for (const name of parameters) {
    assert.equal(typeof name, "string");
    assert(/^[$_\p{ID_Start}][$_\u200C\u200D\p{ID_Continue}]*$/u.test(name),
      "CompileFunction parameters must be individual identifier names");
  }
  // Build-side syntax validation only. Never invoke the result. Parsing the
  // original Function body prevents an invalid closing-brace injection from
  // becoming a valid factory Script with compile-time side effects.
  new Function(...parameters, body);
}

// This is generic Function-body packaging, not a Deno-source rewrite. The
// header binds exact parameter boundaries while preserving the original body.
export function functionFactorySource(body, parameters) {
  const encoded = parameters.map(name => Buffer.from(name, "utf8").toString("hex"));
  return `/*v8x CompileFunction v1 ${JSON.stringify(encoded)}*/\n(function(${parameters.join(",")}) {\n${body}\n})`;
}

export function packageFunction(compiler, precompiler, specifier, body, parameters, output) {
  validateFunctionBody(body, parameters);
  return packageScript(compiler, precompiler, `v8x:CompileFunction:${specifier}`,
    functionFactorySource(body, parameters), output);
}
