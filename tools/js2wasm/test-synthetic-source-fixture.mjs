import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import vm from "node:vm";

test("source imports retain native synthetic object identity and live updates on V8", async () => {
  const context = vm.createContext({});
  const marker = {};
  const replacement = {};
  let calls = 0;
  const native = new vm.SyntheticModule(["value"], function () {
    calls++;
    this.setExport("value", marker);
  }, { context, identifier: "custom:native" });
  const entry = new vm.SourceTextModule(readFileSync(new URL(
    "../../tests/fixtures/js2wasm-synthetic-source/entry.js", import.meta.url), "utf8"),
  { context, identifier: "file:///synthetic-source/entry.js" });
  await entry.link(specifier => {
    assert.equal(specifier, "custom:native");
    return native;
  });
  assert.equal(calls, 0);
  await entry.evaluate();
  assert.equal(calls, 1);
  assert.equal(entry.namespace.observed, marker);
  assert.equal(entry.namespace.live(), marker);
  native.setExport("value", replacement);
  assert.equal(entry.namespace.observed, marker);
  assert.equal(entry.namespace.live(), replacement);
  assert.equal(entry.namespace.namespace(), native.namespace);
  assert.equal(Reflect.set(native.namespace, "value", marker), false);
  assert.equal(native.namespace.value, replacement);
  await entry.evaluate();
  assert.equal(calls, 1);
});
