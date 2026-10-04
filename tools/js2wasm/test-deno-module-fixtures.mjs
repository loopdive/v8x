import assert from "node:assert/strict";
import { test } from "node:test";
import { functionSource, rawBinding } from "./deno-module-fixtures.mjs";

test("extracts the named original test without swallowing its successor", () => {
  const source = '#[test]\nasync fn target() {\n  let source = r#"\n  x();\n"#;\n}\n#[test]\nfn other() {}';
  const body = functionSource(source, "target");
  assert.equal(rawBinding(body, "source"), "\n  x();\n");
  assert(!body.includes("fn other"));
});
test("missing tests and changed raw layouts are loud refusals", () => {
  assert.throws(() => functionSource("fn other() {}", "target"), /missing or ambiguous original Deno test/);
  assert.throws(() => rawBinding('let source = "rewritten";', "source"), /missing original raw binding/);
});
test("comments and duplicate declarations do not certify a fixture", () => {
  assert.throws(() => functionSource("// fn target() {}", "target"), /missing or ambiguous/);
  assert.throws(() => functionSource("fn target() {}\nfn target() {}", "target"), /missing or ambiguous/);
});
