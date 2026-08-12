// Copyright 2026 Loopdive GmbH. Licensed under Apache-2.0 WITH LLVM-exception.

import { readFileSync } from "node:fs";

const input = process.argv[2];
if (!input) {
  throw new Error("usage: node summarize-engine-comparison.mjs RESULTS.txt");
}

const lines = readFileSync(input, "utf8").split("\n");
const engines = ["v8", "quickjs", "js2wasm"];

function record(line) {
  return Object.fromEntries(
    line
      .trim()
      .split(/\s+/)
      .slice(1)
      .map((field) => {
        const separator = field.indexOf("=");
        return [field.slice(0, separator), field.slice(separator + 1)];
      }),
  );
}

function records(prefix, predicate = () => true) {
  return lines
    .filter((line) => line.startsWith(prefix))
    .map(record)
    .filter(predicate);
}

function numeric(value, label) {
  const number = Number(value);
  if (!Number.isFinite(number)) throw new Error(`invalid ${label}: ${value}`);
  return number;
}

function median(values, label) {
  if (values.length === 0) throw new Error(`no samples for ${label}`);
  const sorted = values.map((value) => numeric(value, label)).sort((a, b) => a - b);
  const middle = Math.floor(sorted.length / 2);
  return sorted.length % 2 === 0
    ? (sorted[middle - 1] + sorted[middle]) / 2
    : sorted[middle];
}

const config = records("V8X_ENGINE_CONFIG ")[0];
if (!config) throw new Error("missing V8X_ENGINE_CONFIG record");
const repeats = numeric(config.repeats, "repeat count");

const sizes = Object.fromEntries(
  records("V8X_ENGINE_SIZE ").map((value) => [value.engine, value]),
);
const footprints = Object.fromEntries(
  engines.map((engine) => [
    engine,
    records(
      "V8X_ENGINE_BENCH ",
      (value) => value.engine === engine && value.kind === "summary",
    ),
  ]),
);
const speeds = Object.fromEntries(
  engines.map((engine) => [
    engine,
    Object.fromEntries(
      ["noop", "kernel"].map((workload) => [
        workload,
        records(
          "V8X_ENGINE_SPEED ",
          (value) => value.engine === engine && value.workload === workload,
        ),
      ]),
    ),
  ]),
);

for (const engine of engines) {
  if (!sizes[engine]) throw new Error(`missing size for ${engine}`);
  if (footprints[engine].length !== repeats) {
    throw new Error(
      `expected ${repeats} footprint samples for ${engine}, got ${footprints[engine].length}`,
    );
  }
  for (const workload of ["noop", "kernel"]) {
    if (speeds[engine][workload].length !== repeats) {
      throw new Error(
        `expected ${repeats} ${workload} samples for ${engine}, got ${speeds[engine][workload].length}`,
      );
    }
  }
}

const footprintMetrics = [
  {
    label: "Combined linked payload",
    value: (engine) => numeric(sizes[engine].combined_bytes, `${engine} payload`),
    format: formatBytes,
    comparison: "size",
  },
  {
    label: "One live instance RSS (shared init included)",
    value: (engine) =>
      1024 *
      median(
        footprints[engine].map((sample) => sample.one_instance_total_rss_kib),
        `${engine} one-instance RSS`,
      ),
    format: formatBytes,
    comparison: "size",
  },
  {
    label: "Steady RSS per additional instance",
    value: (engine) =>
      median(
        footprints[engine].map((sample) => sample.steady_rss_bytes_per_instance),
        `${engine} steady RSS`,
      ),
    format: formatBytes,
    comparison: "size",
  },
  {
    label: "Steady virtual address space per instance",
    value: (engine) =>
      median(
        footprints[engine].map((sample) => sample.steady_vsz_bytes_per_instance),
        `${engine} steady virtual address space`,
      ),
    format: formatBytes,
    comparison: "size",
  },
  {
    label: "Steady module/isolate creation time",
    value: (engine) =>
      median(
        footprints[engine].map((sample) => sample.steady_creation_us_per_instance),
        `${engine} steady creation time`,
      ),
    format: formatMicroseconds,
    comparison: "time",
  },
];

function formatBytes(value) {
  const kib = 1024;
  const mib = kib * 1024;
  const gib = mib * 1024;
  if (value >= gib) return `${(value / gib).toFixed(1)} GiB`;
  if (value >= mib) return `${(value / mib).toFixed(1)} MiB`;
  return `${(value / kib).toFixed(1)} KiB`;
}

function formatMicroseconds(value) {
  if (value >= 1000) return `${(value / 1000).toFixed(1)} ms`;
  return `${value.toFixed(1)} µs`;
}

function relativeFactor(value, baseline, comparison) {
  if (comparison === "time") {
    return value < baseline
      ? `${(baseline / value).toFixed(1)}× faster`
      : `${(value / baseline).toFixed(1)}× slower`;
  }
  return value < baseline
    ? `${(baseline / value).toFixed(1)}× smaller`
    : `${(value / baseline).toFixed(1)}× larger`;
}

function speedValue(engine, workload, field) {
  return median(
    speeds[engine][workload].map((sample) => sample[field]),
    `${engine} ${workload} ${field}`,
  );
}

const output = [];
output.push("# V8, QuickJS, and js2wasm comparison", "");
output.push(
  `Medians of ${repeats} fresh processes per engine. Footprint retains ${config.instances} live instances and measures the steady slope from instance 10.`,
  "",
  "## Footprint and startup",
  "",
  "Each QuickJS and js2wasm cell shows its factor relative to V8 in parentheses.",
  "",
  "| Metric | V8 | QuickJS | js2wasm |",
  "| --- | ---: | ---: | ---: |",
);
for (const metric of footprintMetrics) {
  const v8 = metric.value("v8");
  const quickjs = metric.value("quickjs");
  const js2wasm = metric.value("js2wasm");
  output.push(
    `| ${metric.label} | ${metric.format(v8)} | ${metric.format(quickjs)} (${relativeFactor(quickjs, v8, metric.comparison)}) | ${metric.format(js2wasm)} (${relativeFactor(js2wasm, v8, metric.comparison)}) |`,
  );
}

const v8Noop = speedValue("v8", "noop", "ns_per_call");
const quickjsNoop = speedValue("quickjs", "noop", "ns_per_call");
const js2wasmNoop = speedValue("js2wasm", "noop", "ns_per_call");
const v8Kernel = speedValue("v8", "kernel", "ns_per_iteration");
const quickjsKernel = speedValue("quickjs", "kernel", "ns_per_iteration");
const js2wasmKernel = speedValue("js2wasm", "kernel", "ns_per_iteration");

output.push(
  "",
  "## Warm execution speed",
  "",
  "Absolute values are elapsed time. Parentheses show the speed factor relative to V8.",
  "",
  "| Warm workload | V8 | QuickJS | js2wasm |",
  "| --- | ---: | ---: | ---: |",
  `| Export-call boundary (${Number(config.noop_calls).toLocaleString("en-US")} calls) | ${v8Noop.toFixed(1)} ns/call | ${quickjsNoop.toFixed(1)} ns/call (${relativeFactor(quickjsNoop, v8Noop, "time")}) | ${js2wasmNoop.toFixed(1)} ns/call (${relativeFactor(js2wasmNoop, v8Noop, "time")}) |`,
  `| Numeric kernel (${(Number(config.kernel_calls) * Number(config.kernel_iterations)).toLocaleString("en-US")} loop iterations) | ${v8Kernel.toFixed(1)} ns/iteration | ${quickjsKernel.toFixed(1)} ns/iteration (${relativeFactor(quickjsKernel, v8Kernel, "time")}) | ${js2wasmKernel.toFixed(1)} ns/iteration (${relativeFactor(js2wasmKernel, v8Kernel, "time")}) |`,
  "",
  "The call-boundary row uses each backend's native host API: rusty_v8 for V8, the rusty_v8-shaped v8x API for QuickJS, and a typed Wasmtime export for js2wasm. The numeric-kernel row makes that one-call setup negligible but is still a microbenchmark, not Deno application throughput.",
  "",
  "Virtual address space is reserved address range, not committed physical memory. This benchmark module has no linear memory; the js2wasm row is Wasmtime's default WasmGC heap reservation rather than guest allocation.",
);

process.stdout.write(`${output.join("\n")}\n`);
