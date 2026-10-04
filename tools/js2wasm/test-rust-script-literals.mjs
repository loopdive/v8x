import assert from "node:assert/strict";
import { test } from "node:test";
import { literalScripts } from "./rust-script-literals.mjs";

test("preserves exact raw source whitespace and resource names", () => {
  const scripts = literalScripts('runtime.execute_script("", r##"\n({next: 1})\n"##,).unwrap();\nruntime.execute_script("a", "42;");');
  assert.deepEqual(scripts, [{ specifier: "", source: "\n({next: 1})\n", line: 1 },
    { specifier: "a", source: "42;", line: 4 }]);
});
test("unsupported expressions cannot silently disappear from the input population", () => {
  assert.throws(() => literalScripts('r.execute_script("", source);'), /not a literal/);
  assert.throws(() => literalScripts('r.execute_script("", "a" + other);'), /unsupported expression/);
  assert.throws(() => literalScripts('r.execute_script(name, "a");'), /not a literal/);
});
