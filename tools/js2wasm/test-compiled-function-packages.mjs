import assert from "node:assert/strict";
import { test } from "node:test";
import { functionFactorySource, validateFunctionBody } from "./compiled-function-packages.mjs";
import { scriptDigest } from "./script-packages.mjs";

test("factory preserves body bytes and independently binds parameter boundaries", () => {
  const body = '"use strict";\nreturn value + 1;';
  const source = functionFactorySource(body, ["value"]);
  assert.equal(source, `/*v8x CompileFunction v1 ["76616c7565"]*/\n(function(value) {\n${body}\n})`);
  assert.notEqual(functionFactorySource(body, []), functionFactorySource(body, [""]));
  assert.notEqual(functionFactorySource(body, ["a,b"]), functionFactorySource(body, ["a", "b"]));
  assert.notEqual(scriptDigest("f", source), scriptDigest("g", source));
  assert.notEqual(source, functionFactorySource(body + " ", ["value"]));
});

test("factory compilation creates the function without executing its body", () => {
  const factory = functionFactorySource('throw new Error("deferred");', ["é"]);
  assert(factory.startsWith('/*v8x CompileFunction v1 ["c3a9"]*/'));
  const callable = (0, eval)(factory);
  assert.equal(typeof callable, "function");
  assert.throws(() => callable(1), /deferred/);
});

test("invalid bodies cannot escape the factory and parameters are not source fragments", () => {
  assert.doesNotThrow(() => validateFunctionBody('"use strict";return value;', ["value"]));
  assert.doesNotThrow(() => validateFunctionBody("throw new Error('not executed');", ["é"]));
  assert.throws(() => validateFunctionBody("});globalThis.shouldNotRun=1;(function(){", []), SyntaxError);
  for (const name of ["", "a,b", "a=globalThis.sideEffect()", "{a}", "/*x*/a"]) {
    assert.throws(() => validateFunctionBody("return 1;", [name]), /identifier names/);
  }
  assert.throws(() => validateFunctionBody('"use strict";return a;', ["a", "a"]), SyntaxError);
});
