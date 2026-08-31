#!/usr/bin/env node
// Copyright 2026 Loopdive GmbH. Licensed under Apache-2.0 WITH LLVM-exception.

import { spawnSync } from "node:child_process";
import {
  copyFileSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { isAbsolute, join, resolve } from "node:path";

function fail(message) {
  throw new Error(`v8x Deno POC negative controls: ${message}`);
}

function parseArgs(argv) {
  const args = new Map();
  for (const arg of argv) {
    const match = /^--([^=]+)=(.*)$/.exec(arg);
    if (!match) fail(`expected --name=value, received ${arg}`);
    if (args.has(match[1])) fail(`duplicate --${match[1]}`);
    args.set(match[1], match[2]);
  }
  const names = [
    "finalizer",
    "provenance",
    "app-raw",
    "provider-raw",
    "app-aot",
    "provider-aot",
    "app-attestation",
    "provider-attestation",
  ];
  if (
    args.size !== names.length ||
    names.some((name) => !args.has(name) || !isAbsolute(args.get(name)))
  ) {
    fail(`required absolute arguments: ${names.join(", ")}`);
  }
  return Object.fromEntries(
    names.map((name) => [name.replaceAll("-", "_"), resolve(args.get(name))]),
  );
}

function clone(value) {
  return JSON.parse(JSON.stringify(value));
}

function appAttestationOverride(paths, directory, name, mutate) {
  const aotPath = join(directory, `${name}.cwasm`);
  copyFileSync(paths.app_aot, aotPath);
  const attestation = clone(
    JSON.parse(readFileSync(paths.app_attestation, "utf8")),
  );
  mutate(attestation);
  const attestationPath = `${aotPath}.attestation.json`;
  writeFileSync(
    attestationPath,
    `${JSON.stringify(attestation, null, 2)}\n`,
  );
  return { app_aot: aotPath, app_attestation: attestationPath };
}

function expectRejected(
  paths,
  directory,
  provenance,
  name,
  fragment,
  overrides = {},
) {
  const provenancePath = join(directory, `${name}.json`);
  const outputPath = join(directory, `${name}.lock.json`);
  writeFileSync(provenancePath, `${JSON.stringify(provenance, null, 2)}\n`);
  const child = spawnSync(
    process.execPath,
    [
      paths.finalizer,
      `--provenance=${provenancePath}`,
      `--app-raw=${overrides.app_raw ?? paths.app_raw}`,
      `--provider-raw=${overrides.provider_raw ?? paths.provider_raw}`,
      `--app-aot=${overrides.app_aot ?? paths.app_aot}`,
      `--provider-aot=${overrides.provider_aot ?? paths.provider_aot}`,
      `--app-attestation=${overrides.app_attestation ?? paths.app_attestation}`,
      `--provider-attestation=${
        overrides.provider_attestation ?? paths.provider_attestation
      }`,
      `--out=${outputPath}`,
    ],
    { encoding: "utf8" },
  );
  const diagnostics = `${child.stdout ?? ""}\n${child.stderr ?? ""}`;
  if (child.status === 0 || !diagnostics.includes(fragment)) {
    fail(
      `${name} was not rejected for ${JSON.stringify(fragment)}:\n${diagnostics}`,
    );
  }
}

function main() {
  const paths = parseArgs(process.argv.slice(2));
  const provenance = JSON.parse(readFileSync(paths.provenance, "utf8"));
  const directory = mkdtempSync(join(tmpdir(), "v8x-deno-poc-negative-"));
  try {
    const graph = clone(provenance);
    graph.source_graph.inputs[7].sha256 = "0".repeat(64);
    expectRejected(
      paths,
      directory,
      graph,
      "source-graph",
      "source graph digest",
    );

    const provider = clone(provenance);
    provider.compiler.runtime_eval_provider.source.sha256 = "0".repeat(64);
    expectRejected(
      paths,
      directory,
      provider,
      "provider-graph",
      "interpreter provider digest",
    );

    const engine = clone(provenance);
    engine.wasmtime.engine_config.wasm_exceptions = false;
    expectRejected(paths, directory, engine, "engine-config", "must be true");

    for (const [name, field, value, fragment] of [
      ["engine-debug-symbols", "debug_symbols", true, "must be false"],
      [
        "engine-address-map",
        "generate_address_map",
        true,
        "must be false",
      ],
      [
        "engine-backtrace-details",
        "wasm_backtrace_details",
        "enable",
        "must be \"disable\"",
      ],
    ]) {
      const modifiedEngine = clone(provenance);
      modifiedEngine.wasmtime.engine_config[field] = value;
      expectRejected(paths, directory, modifiedEngine, name, fragment);
    }

    const missingEngineConfig = clone(provenance);
    delete missingEngineConfig.wasmtime.engine_config.debug_symbols;
    expectRejected(
      paths,
      directory,
      missingEngineConfig,
      "missing-engine-config",
      "unexpected field inventory",
    );

    for (const [name, field, value, fragment] of [
      ["attestation-debug-symbols", "debug_symbols", true, "must be false"],
      [
        "attestation-address-map",
        "generate_address_map",
        true,
        "must be false",
      ],
      [
        "attestation-backtrace-details",
        "wasm_backtrace_details",
        "enable",
        "must be \"disable\"",
      ],
    ]) {
      const overrides = appAttestationOverride(
        paths,
        directory,
        name,
        (attestation) => {
          attestation.engine_config[field] = value;
        },
      );
      expectRejected(paths, directory, provenance, name, fragment, overrides);
    }

    const contract = clone(provenance);
    contract.contract.sha256 = "0".repeat(64);
    expectRejected(paths, directory, contract, "contract", "contract digest");

    const swappedAot = join(directory, "swapped-app.cwasm");
    copyFileSync(paths.provider_aot, swappedAot);
    copyFileSync(paths.app_attestation, `${swappedAot}.attestation.json`);
    expectRejected(
      paths,
      directory,
      provenance,
      "swapped-aot",
      "aot_sha256 does not bind",
      {
        app_aot: swappedAot,
        app_attestation: `${swappedAot}.attestation.json`,
      },
    );

    const tamperedAttestation = clone(
      JSON.parse(readFileSync(paths.app_attestation, "utf8")),
    );
    tamperedAttestation.aot_sha256 = "0".repeat(64);
    const tamperedAot = join(directory, "tampered-app.cwasm");
    copyFileSync(paths.app_aot, tamperedAot);
    const tamperedAttestationPath = `${tamperedAot}.attestation.json`;
    writeFileSync(
      tamperedAttestationPath,
      `${JSON.stringify(tamperedAttestation, null, 2)}\n`,
    );
    expectRejected(
      paths,
      directory,
      provenance,
      "tampered-attestation",
      "aot_sha256 does not bind",
      { app_aot: tamperedAot, app_attestation: tamperedAttestationPath },
    );

    const unknownFieldAttestation = clone(
      JSON.parse(readFileSync(paths.app_attestation, "utf8")),
    );
    unknownFieldAttestation.untrusted_extension = true;
    const unknownFieldAot = join(directory, "unknown-field-app.cwasm");
    copyFileSync(paths.app_aot, unknownFieldAot);
    const unknownFieldAttestationPath = `${unknownFieldAot}.attestation.json`;
    writeFileSync(
      unknownFieldAttestationPath,
      `${JSON.stringify(unknownFieldAttestation, null, 2)}\n`,
    );
    expectRejected(
      paths,
      directory,
      provenance,
      "unknown-attestation-field",
      "unexpected field inventory",
      {
        app_aot: unknownFieldAot,
        app_attestation: unknownFieldAttestationPath,
      },
    );
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
  process.stdout.write("v8x Deno POC negative lock controls passed\n");
}

try {
  main();
} catch (error) {
  console.error(error?.stack ?? error);
  process.exitCode = 1;
}
