import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { stripTypeScriptTypes } from "node:module";
import vm from "node:vm";
import { test } from "node:test";
import { aotHelloWorldSource } from "./aot-hello-world.mjs";

const path = process.env.DENO_HELLO_WORLD_SOURCE;
if (!path) throw new Error("DENO_HELLO_WORLD_SOURCE must name the pinned upstream Rust example");
const rust = readFileSync(path, "utf8");
const match = /\.execute_script\(\s*"<usage>"\s*,\s*r#"([\s\S]*?)"#/g;
const matches = [...rust.matchAll(match)];
assert.equal(matches.length, 1);
const source = matches[0][1];
const generated = aotHelloWorldSource(source);
function realm() {
  const output = [];
  const Deno = { core: { print: (s) => output.push(s), ops: {
    op_sum: (a) => {
      if (!Array.isArray(a)) throw new TypeError("expected array");
      return a.reduce((x, y) => x + y, 0);
    },
  } } };
  return { output, context: vm.createContext({ Deno }) };
}
function candidate() {
  const r = realm();
  vm.runInContext(stripTypeScriptTypes(generated.replace("export function", "function")), r.context);
  return r;
}
test("compiles the exact source body and matches completion/output", () => {
  assert.ok(generated.includes(source));
  const control = realm();
  assert.equal(vm.runInContext(source, control.context), undefined);
  const r = candidate();
  assert.equal(r.context.runAotHostScript(source), undefined);
  assert.deepEqual(r.output, control.output);
  assert.equal(r.output.length, 6);
});
test("unknown source is refused without executing or consuming the program", () => {
  const r = candidate();
  assert.throws(() => r.context.runAotHostScript("throw 123"), /rejects unknown script/);
  assert.deepEqual(r.output, []);
  r.context.runAotHostScript(source);
  assert.equal(r.output.length, 6);
});
test("repeated execution is explicitly refused", () => {
  const r = candidate();
  r.context.runAotHostScript(source);
  assert.throws(() => r.context.runAotHostScript(source), /already executed/);
  assert.equal(r.output.length, 6);
});
test("unreviewed source shape cannot silently become a function wrapper", () => {
  assert.throws(() => aotHelloWorldSource(source + "\n1;"), /unsupported closed-world/);
  assert.throws(() => aotHelloWorldSource(undefined), /unsupported closed-world/);
});
