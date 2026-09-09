// Copyright (c) 2026 Loopdive GmbH. Licensed under Apache-2.0 WITH LLVM-exception.
// macOS: identical, unchanged deno_core hello_world executables in fresh processes.
import {
  copyFileSync,
  mkdirSync,
  readFileSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { join, basename } from "node:path";
import { performance } from "node:perf_hooks";

if (process.platform !== "darwin")
  throw Error("This runner requires macOS /usr/bin/time -l");
const required = (name) => {
  const value = process.env[name];
  if (!value) throw Error(`Missing ${name}`);
  return value;
};
const out = required("DENO_BENCH_OUTPUT");
mkdirSync(out, { recursive: true });
const repeats = Number(process.env.DENO_BENCH_REPEATS ?? 5);
if (!Number.isInteger(repeats) || repeats < 3)
  throw Error("Use at least three repeats");
const expected = readFileSync(
  new URL("./js2wasm-poc-expected.stdout", import.meta.url),
  "utf8",
);
const core = required("DENO_BENCH_CORE_AOT");
const provider = required("DENO_BENCH_PROVIDER_AOT");
const engines = [
  { name: "v8", path: required("DENO_BENCH_V8"), artifacts: [] },
  { name: "quickjs", path: required("DENO_BENCH_QUICKJS"), artifacts: [] },
  {
    name: "js2wasm",
    path: required("DENO_BENCH_JS2WASM"),
    artifacts: [core, provider],
  },
];

const sharedLibraries = JSON.parse(process.env.DENO_BENCH_SHARED_LIBRARIES ?? "[]");
if (!Array.isArray(sharedLibraries) || sharedLibraries.some(p => typeof p !== "string"))
  throw Error("DENO_BENCH_SHARED_LIBRARIES must be a JSON array of paths");
const command = (name, args) => {
  const result = spawnSync(name, args, {encoding: "utf8"});
  if (result.status !== 0) throw Error(name + ": " + result.stderr);
};

const fileRecord = (path) => ({
  path,
  bytes: statSync(path).size,
  sha256: createHash("sha256").update(readFileSync(path)).digest("hex"),
});
for (const engine of engines) {
  engine.original = fileRecord(engine.path);
  const stripped = join(out, `hello-world-${engine.name}`);
  copyFileSync(engine.path, stripped);
  const strip = spawnSync("strip", ["-x", stripped], { encoding: "utf8" });
  if (strip.status !== 0) throw Error(strip.stderr);

  if (engine.name === "js2wasm" && sharedLibraries.length) {
    const copies = sharedLibraries.map(path => {
      const target = join(out, basename(path));
      copyFileSync(path, target);
      command("strip", ["-x", target]);
      return target;
    });
    for (const target of [stripped, ...copies]) {
      const linkage = spawnSync("otool", ["-L", target], {encoding:"utf8"});
      if (linkage.status !== 0) throw Error(linkage.stderr);
      for (const line of linkage.stdout.split("\n").slice(1)) {
        const old = line.trim().split(" (")[0];
        if (copies.some(p => basename(p) === basename(old)))
          command("install_name_tool", ["-change", old, "@loader_path/" + basename(old), target]);
      }
      command("codesign", ["--force", "--sign", "-", target]);
    }
    engine.sharedLibraries = copies.map(fileRecord);
  } else engine.sharedLibraries = [];

  engine.stripped = fileRecord(stripped);
  engine.artifacts = engine.artifacts.map(fileRecord);
  engine.payloadBytes =
    engine.stripped.bytes +
    engine.artifacts.reduce((sum, file) => sum + file.bytes, 0) + engine.sharedLibraries.reduce((sum, file) => sum + file.bytes, 0);
}
const runs = [];
for (let round = 0; round < repeats; round++) {
  for (let slot = 0; slot < engines.length; slot++) {
    const engine = engines[(round + slot) % engines.length];
    const env = { ...process.env };
    for (const key of Object.keys(env))
      if (key.startsWith("V8X_JS2WASM_")) delete env[key];
    if (engine.name === "js2wasm") {
      env.V8X_JS2WASM_DENO_CORE_AOT_MODULE = core;
      env.V8X_JS2WASM_RUNTIME_EVAL_AOT_MODULE = provider;
    }
    const start = performance.now();
    const run = spawnSync("/usr/bin/time", ["-l", engine.stripped.path], {
      env,
      encoding: "utf8",
      maxBuffer: 8 * 1024 * 1024,
    });
    const wallMs = performance.now() - start;
    const prefix = join(out, `${round + 1}-${engine.name}`);
    writeFileSync(`${prefix}.stdout`, run.stdout ?? "");
    writeFileSync(`${prefix}.stderr`, run.stderr ?? "");
    const peak = /([0-9]+)\s+maximum resident set size/.exec(run.stderr ?? "");
    const physical = /([0-9]+)\s+peak memory footprint/.exec(run.stderr ?? "");
    const valid = run.status === 0 && run.stdout === expected && peak !== null;
    runs.push({
      engine: engine.name,
      round: round + 1,
      wallMs,
      peakRssBytes: peak ? Number(peak[1]) : null,
      peakPhysicalFootprintBytes: physical ? Number(physical[1]) : null,
      exitCode: run.status,
      signal: run.signal,
      valid,
    });
    writeFileSync(
      join(out, "results.json"),
      JSON.stringify(
        {
          date: new Date().toISOString(),
          platform: process.platform,
          arch: process.arch,
          repeats,
          engines,
          runs,
        },
        null,
        2,
      ),
    );
    console.log(
      engine.name,
      round + 1,
      valid ? "PASS" : "FAIL",
      `${wallMs.toFixed(1)} ms`,
      peak?.[1],
    );
    if (!valid)
      throw Error(`Correctness failed for ${engine.name}; see ${prefix}`);
  }
}
const median = (values) => {
  const sorted = values.toSorted((a, b) => a - b);
  const middle = Math.floor(sorted.length / 2);
  return sorted.length % 2
    ? sorted[middle]
    : (sorted[middle - 1] + sorted[middle]) / 2;
};
const stats = engines.map((engine) => {
  const rows = runs.filter((run) => run.engine === engine.name);
  return {
    name: engine.name,
    payload: engine.payloadBytes,
    wall: median(rows.map((r) => r.wallMs)),
    rss: median(rows.map((r) => r.peakRssBytes)),
    physical: rows.every(r => r.peakPhysicalFootprintBytes !== null) ? median(rows.map(r => r.peakPhysicalFootprintBytes)) : null,
  };
});
const ratio = (value, base, kind) =>
  value <= base
    ? `${(base / value).toFixed(1)}× ${kind === "time" ? "faster" : "smaller"}`
    : `${(value / base).toFixed(1)}× ${kind === "time" ? "slower" : "larger"}`;
const rows = [
  [
    "Deployment payload",
    "payload",
    (v) => `${(v / 1048576).toFixed(1)} MiB`,
    "size",
  ],
  ["Peak process RSS", "rss", (v) => `${(v / 1048576).toFixed(1)} MiB`, "size"],
  ...(stats.every(s => s.physical !== null) ? [["macOS physical footprint", "physical", (v) => `${(v / 1048576).toFixed(1)} MiB`, "size"]] : []),
  ["Start, run example, exit", "wall", (v) => `${v.toFixed(1)} ms`, "time"],
];
const report = [
  "# Deno core fresh-process comparison",
  "",
  `${repeats} runs per engine, rotated order; every run must exit 0 with the exact upstream example output. Medians below.`,
  "",
  "| Metric | V8 | QuickJS | js2wasm |",
  "| --- | ---: | ---: | ---: |",
  ...rows.map(
    ([label, key, format, kind]) =>
      `| ${label} | ${stats.map((stat, index) => format(stat[key]) + (index ? ` (${ratio(stat[key], stats[0][key], kind)})` : "")).join(" | ")} |`,
  ),
  "",
  "Payload includes the stripped executable, deployed shared libraries, and required js2wasm core/provider precompiled artifacts. macOS physical footprint is OS accounting that excludes clean file-backed pages, not marginal tenant memory. Peak RSS is whole-process high-water memory, not additional-instance memory. Elapsed time includes process launch, initialization, the upstream op/printing/error workload, and exit; it excludes all build/AOT compilation. Filesystem caches are not flushed. This is not warm-kernel throughput or a multi-tenant density measurement.",
  "",
  "Scope: pinned deno_core hello_world, not the full Deno CLI/API surface. Detailed inputs, hashes, per-run outputs and timings are retained in results.json and adjacent logs.",
  "",
].join("\n");
writeFileSync(join(out, "summary.md"), report);
console.log(report);
