import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { writeFileSync } from "node:fs";
import { resolve, join } from "node:path";
import { literalScripts } from "./rust-script-literals.mjs";
import { packageScript } from "./script-packages.mjs";

const [compiler, precompiler, deno, output] = process.argv.slice(2);
assert(output, "usage: build-webidl-script-packages.mjs JS2_CHECKOUT PACKAGING_TEST_BINARY PINNED_DENO_CHECKOUT OUTPUT_DIR");
const env = { ...process.env };
delete env.GIT_DIR;
delete env.GIT_WORK_TREE;
const git = args => execFileSync("git", ["-C", resolve(deno), ...args], { encoding: "utf8", env });
const revision = git(["rev-parse", "HEAD"]).trim();
assert.equal(revision, "1d4e6c1cb855b62a7fb572c6c138e4e8b4e7fa44");
const sourcePath = "libs/core/webidl.rs";
const rust = git(["show", `HEAD:${sourcePath}`]);
const scripts = literalScripts(rust);
assert.equal(scripts.length, 5, "pinned WebIDL must expose all five literal Script inputs");
const records = [];
for (const script of scripts) {
  const path = await packageScript(compiler, precompiler, script.specifier, script.source, output);
  records.push({ ...script, path });
  console.log(path);
}
writeFileSync(join(resolve(output), "webidl-inputs.json"), JSON.stringify({ revision, sourcePath,
  rustSha256: createHash("sha256").update(rust).digest("hex"), scripts: records }, null, 2) + "\n");
