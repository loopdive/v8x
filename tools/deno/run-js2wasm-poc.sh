#!/usr/bin/env bash
# Copyright 2026 Loopdive GmbH. Licensed under Apache-2.0 WITH LLVM-exception.

# Build, package, and replay the deliberately bounded Deno/js2wasm POC.
#
# Inputs are three already-materialized Git worktrees. All must be clean and
# detached at their declared commits before raw source is read. The script only
# writes its --out-dir and Deno's Cargo.toml/Cargo.lock; it rejects every other
# Deno source modification after resolving the local v8x dependency.

set -euo pipefail

EXPECTED_JS2_REF="6d306d67543100dde8efcf89a70d068cd693927d"
EXPECTED_DENO_REF="1d4e6c1cb855b62a7fb572c6c138e4e8b4e7fa44"
EXPECTED_OUTPUT_NAME="js2wasm-poc-expected.stdout"
EXPECTED_PRISTINE_DENO_LOCK_SHA256="b0d7c2bcc3caefb833f4bad996d2e7e9fff3bb0cd424f384b5fb88b7cc71ba9f"
EXPECTED_DENO_LOCK_SHA256="a1878b906e0f129437e98e0b135384eb3532a533e9a5578844ca6e2cc6e8f210"
DENO_LOCK_PATCH_NAME="deno-js2wasm-poc-Cargo.lock.patch"
DIAGNOSTIC_ABORT_TEXT="v8x/js2wasm diagnostic ABI reached unresolved symbol:"

usage() {
  cat <<'USAGE'
Usage:
  tools/deno/run-js2wasm-poc.sh \
    --v8x=/absolute/path/to/v8x \
    --js2=/absolute/path/to/js2 \
    --deno=/absolute/path/to/deno \
    --out-dir=/absolute/empty/output/directory \
    [--node=/absolute/path/to/node] [--cargo=/absolute/path/to/cargo]

The runner requires Linux x86_64. It leaves build provenance in --out-dir and
runs the real Deno hello_world example in out-dir/replay, whose only files are
the manifest and the two native Wasmtime artifacts.
USAGE
}

fail() {
  printf 'v8x Deno POC runner: %s\n' "$*" >&2
  exit 1
}

require_absolute_dir() {
  local name="$1"
  local path="$2"
  [[ -n "$path" ]] || fail "missing --${name}=PATH"
  [[ "$path" = /* ]] || fail "--${name} must be an absolute path"
  [[ -d "$path" ]] || fail "--${name} is not a directory: $path"
  (cd "$path" && pwd -P)
}

require_tool() {
  local name="$1"
  local path="$2"
  [[ -n "$path" ]] || fail "could not find required tool: $name"
  [[ -x "$path" ]] || fail "$name is not executable: $path"
}

assert_clean_detached_checkout() {
  local label="$1"
  local repo="$2"
  local expected="$3"
  local actual
  actual="$(git -C "$repo" rev-parse HEAD)"
  [[ "$actual" = "$expected" ]] || fail "$label is $actual, expected $expected"
  if git -C "$repo" symbolic-ref --quiet HEAD >/dev/null; then
    fail "$label must be detached at $expected"
  fi
  local status
  status="$(git -C "$repo" status --porcelain=v1 --untracked-files=all)"
  [[ -z "$status" ]] || fail "$label checkout is not clean:\n$status"
}

assert_no_forbidden_build_environment() {
  local name
  for name in \
    JS2WASM_EVAL_ENGINE \
    TEST262_DISABLE_RUNTIME_EVAL_PROVIDER \
    TEST262_FULL_RUNTIME_EVAL \
    V8X_JS2WASM_AOT_MODULE \
    V8X_JS2WASM_ARTIFACT_OUTPUT \
    V8X_JS2WASM_CACHE_DIR \
    V8X_JS2WASM_COMPILER \
    V8X_JS2WASM_COMPILER_ID \
    V8X_JS2WASM_COMPILER_SCRIPT \
    V8X_JS2WASM_DENO_CORE_AOT_MODULE \
    V8X_JS2WASM_DENO_CORE_AOT_OUTPUT \
    V8X_JS2WASM_DENO_CORE_AOT_ATTESTATION \
    V8X_JS2WASM_DENO_CORE_WASM \
    V8X_JS2WASM_POC_CONTRACT_SHA256 \
    V8X_JS2WASM_DENO_POC_MANIFEST \
    V8X_JS2WASM_RUNTIME_EVAL_AOT_MODULE \
    V8X_JS2WASM_RUNTIME_EVAL_AOT_OUTPUT \
    V8X_JS2WASM_RUNTIME_EVAL_AOT_ATTESTATION \
    V8X_JS2WASM_RUNTIME_EVAL_WASM \
    V8X_JS2WASM_WORKDIR; do
    if printenv "$name" >/dev/null; then
      fail "ambient $name is forbidden; invoke the runner from a clean environment"
    fi
  done
}

assert_deno_mutation_boundary() {
  local repo="$1"
  local changed
  changed="$(git -C "$repo" status --porcelain=v1 --untracked-files=all)"
  while IFS= read -r line; do
    [[ -z "$line" ]] && continue
    local path="${line:3}"
    [[ "$path" = "Cargo.toml" || "$path" = "Cargo.lock" ]] || \
      fail "Deno mutation outside Cargo.toml/Cargo.lock: $line"
  done <<<"$changed"
  git -C "$repo" diff --exit-code -- libs/core
  git -C "$repo" diff --exit-code -- libs/core/examples/hello_world.rs
}

assert_replay_features() {
  local deno_dir="$1"
  local cargo_bin="$2"
  local target_dir="$3"
  local tree
  tree="$(cd "$deno_dir" && CARGO_TARGET_DIR="$target_dir" "$cargo_bin" tree --locked -e features -p deno_core)"
  if grep -Eiq '(^|[^[:alnum:]_])(engine_quickjs|link_quickjs|quickjs|js2wasm_runtime_compile|engine_js2wasm_runtime)([^[:alnum:]_]|$)' <<<"$tree"; then
    printf '%s\n' "$tree" >&2
    fail "compiler-free replay feature graph includes QuickJS or runtime compilation"
  fi
  if grep -Eq 'wasmtime-internal-cranelift|cranelift-codegen|cranelift-native|cranelift-frontend|wasmtime feature "cranelift"' <<<"$tree"; then
    printf '%s\n' "$tree" >&2
    fail "compiler-free replay feature graph includes Wasmtime's Cranelift compiler"
  fi
  grep -Fq 'js2wasm_deno_poc_replay' <<<"$tree" || {
    printf '%s\n' "$tree" >&2
    fail "compiler-free replay feature is absent from Deno's resolved v8x dependency"
  }
}

V8X_DIR=""
JS2_DIR=""
DENO_DIR=""
OUT_DIR=""
NODE_BIN="${NODE:-$(command -v node || true)}"
CARGO_BIN="${CARGO:-$(command -v cargo || true)}"

while (($# > 0)); do
  case "$1" in
    --v8x=*) V8X_DIR="${1#*=}" ;;
    --js2=*) JS2_DIR="${1#*=}" ;;
    --deno=*) DENO_DIR="${1#*=}" ;;
    --out-dir=*) OUT_DIR="${1#*=}" ;;
    --node=*) NODE_BIN="${1#*=}" ;;
    --cargo=*) CARGO_BIN="${1#*=}" ;;
    --help|-h) usage; exit 0 ;;
    *) usage >&2; fail "unknown argument: $1" ;;
  esac
  shift
done

V8X_DIR="$(require_absolute_dir v8x "$V8X_DIR")"
JS2_DIR="$(require_absolute_dir js2 "$JS2_DIR")"
DENO_DIR="$(require_absolute_dir deno "$DENO_DIR")"
[[ -n "$OUT_DIR" && "$OUT_DIR" = /* ]] || fail "--out-dir must be an absolute path"
require_tool node "$NODE_BIN"
require_tool cargo "$CARGO_BIN"
[[ "$(uname -s)" = "Linux" && "$(uname -m)" = "x86_64" ]] || \
  fail "this POC is intentionally pinned to Linux x86_64 (got $(uname -s)/$(uname -m))"
assert_no_forbidden_build_environment

if [[ -e "$OUT_DIR" ]]; then
  [[ -d "$OUT_DIR" ]] || fail "--out-dir is not a directory: $OUT_DIR"
  [[ -z "$(find "$OUT_DIR" -mindepth 1 -maxdepth 1 -print -quit)" ]] || \
    fail "--out-dir must be empty: $OUT_DIR"
else
  mkdir -p "$OUT_DIR"
fi
OUT_DIR="$(cd "$OUT_DIR" && pwd -P)"

V8X_REF="$(git -C "$V8X_DIR" rev-parse HEAD)"
assert_clean_detached_checkout v8x "$V8X_DIR" "$V8X_REF"
assert_clean_detached_checkout js2 "$JS2_DIR" "$EXPECTED_JS2_REF"
assert_clean_detached_checkout Deno "$DENO_DIR" "$EXPECTED_DENO_REF"
[[ "$(tr -d '[:space:]' < "$V8X_DIR/tools/deno/DENO_REF")" = "$EXPECTED_DENO_REF" ]] || \
  fail "tools/deno/DENO_REF does not contain $EXPECTED_DENO_REF"

RAW_APP="$OUT_DIR/deno-core.wasm"
RAW_PROVIDER="$OUT_DIR/runtime-eval-provider.wasm"
RAW_PROVENANCE="$OUT_DIR/raw-inputs.json"
APP_AOT="$OUT_DIR/deno-core.cwasm"
PROVIDER_AOT="$OUT_DIR/runtime-eval-provider.cwasm"
APP_ATTESTATION="${APP_AOT}.attestation.json"
PROVIDER_ATTESTATION="${PROVIDER_AOT}.attestation.json"
LOCK="$OUT_DIR/poc-lock.json"

(
  cd "$JS2_DIR"
  # The closed interpreter/provider graph peaks above V8's 3 GiB old-space
  # ceiling during serialization. Node exits before native AOT compilation, so
  # its 5 GiB ceiling never overlaps Cranelift's separate memory peak.
  "$NODE_BIN" --max-old-space-size=5120 --experimental-wasm-exnref --import tsx \
    "$V8X_DIR/tools/js2wasm/build-deno-core-artifact.mjs" \
    --v8x="$V8X_DIR" \
    --js2="$JS2_DIR" \
    --deno="$DENO_DIR" \
    --out="$RAW_APP" \
    --provider-out="$RAW_PROVIDER" \
    --provenance-out="$RAW_PROVENANCE"
)
[[ -s "$RAW_APP" && -s "$RAW_PROVIDER" && -s "$RAW_PROVENANCE" ]] || \
  fail "raw application, interpreter provider, or provenance output is missing"

# These are the only runtime-compilation processes. Keep each artifact in its
# own process so the app's compiler/module state is released before Cranelift
# handles the much larger interpreter provider. A third process loads both AOT
# files and proves the exact app still boots in two stores without recompiling.
V8X_JS2WASM_DENO_CORE_WASM="$RAW_APP" \
V8X_JS2WASM_DENO_CORE_AOT_OUTPUT="$APP_AOT" \
V8X_JS2WASM_DENO_CORE_AOT_ATTESTATION="$APP_ATTESTATION" \
CARGO_TARGET_DIR="$OUT_DIR/trusted-packaging-target" \
  "$CARGO_BIN" test --manifest-path "$V8X_DIR/Cargo.toml" \
    --release --locked --no-default-features --features js2wasm_deno_poc \
    --test js2wasm_spike precompiles_exact_deno_core_artifact -- --exact

V8X_JS2WASM_RUNTIME_EVAL_WASM="$RAW_PROVIDER" \
V8X_JS2WASM_RUNTIME_EVAL_AOT_OUTPUT="$PROVIDER_AOT" \
V8X_JS2WASM_RUNTIME_EVAL_AOT_ATTESTATION="$PROVIDER_ATTESTATION" \
CARGO_TARGET_DIR="$OUT_DIR/trusted-packaging-target" \
  "$CARGO_BIN" test --manifest-path "$V8X_DIR/Cargo.toml" \
    --release --locked --no-default-features --features js2wasm_deno_poc \
    --test js2wasm_spike \
    precompiles_exact_runtime_eval_provider_artifact -- --exact

V8X_JS2WASM_DENO_CORE_WASM="$RAW_APP" \
V8X_JS2WASM_DENO_CORE_AOT_MODULE="$APP_AOT" \
V8X_JS2WASM_RUNTIME_EVAL_AOT_MODULE="$PROVIDER_AOT" \
CARGO_TARGET_DIR="$OUT_DIR/trusted-packaging-target" \
  "$CARGO_BIN" test --manifest-path "$V8X_DIR/Cargo.toml" \
    --release --locked --no-default-features --features js2wasm_deno_poc \
    --test js2wasm_spike \
    boots_exact_deno_core_artifact_in_two_wasmtime_stores -- --exact
[[ -s "$APP_AOT" && -s "$PROVIDER_AOT" && -s "$APP_ATTESTATION" && -s "$PROVIDER_ATTESTATION" ]] || \
  fail "trusted precompile did not emit both native artifacts and attestations"

"$NODE_BIN" "$V8X_DIR/tools/deno/finalize-js2wasm-poc-lock.mjs" \
  --provenance="$RAW_PROVENANCE" \
  --app-raw="$RAW_APP" \
  --provider-raw="$RAW_PROVIDER" \
  --app-aot="$APP_AOT" \
  --provider-aot="$PROVIDER_AOT" \
  --app-attestation="$APP_ATTESTATION" \
  --provider-attestation="$PROVIDER_ATTESTATION" \
  --out="$LOCK"
[[ -s "$LOCK" ]] || fail "strict POC manifest was not written"

"$NODE_BIN" "$V8X_DIR/tools/deno/test-js2wasm-poc-lock-negative.mjs" \
  --finalizer="$V8X_DIR/tools/deno/finalize-js2wasm-poc-lock.mjs" \
  --provenance="$RAW_PROVENANCE" \
  --app-raw="$RAW_APP" \
  --provider-raw="$RAW_PROVIDER" \
  --app-aot="$APP_AOT" \
  --provider-aot="$PROVIDER_AOT" \
  --app-attestation="$APP_ATTESTATION" \
  --provider-attestation="$PROVIDER_ATTESTATION"

POC_CONTRACT_SHA256="$("$NODE_BIN" -e '
  const { readFileSync } = require("node:fs");
  const lock = JSON.parse(readFileSync(process.argv[1], "utf8"));
  if (typeof lock.contract_sha256 !== "string" || !/^[0-9a-f]{64}$/.test(lock.contract_sha256)) {
    throw new Error("poc-lock.json has no lowercase contract_sha256");
  }
  process.stdout.write(lock.contract_sha256);
' "$LOCK")"

# The exact pinned Deno workspace dependency is the only source modification.
# Keep the replacement narrowly literal so an upstream Cargo.toml change fails
# loudly rather than being matched loosely.
V8X_POC_PATH="$V8X_DIR" perl -0pi -e '
  $old = q{v8 = { version = "149.4.0", default-features = false, features = ["simdutf"] }};
  $new = q{v8 = { package = "v8x", path = "} . $ENV{V8X_POC_PATH} . q{", default-features = false, features = ["simdutf", "js2wasm_deno_poc_replay"] }};
  $count = s/\Q$old\E/$new/;
  die "expected exactly one pinned Deno v8 dependency line, replaced $count\n" unless $count == 1;
' "$DENO_DIR/Cargo.toml"

# Deno's lock predates Wasmtime 47. Apply the reviewed lock delta generated
# from this exact Deno revision instead of consulting today's registry resolver.
# The complete resulting lock digest makes dependency drift fail before build.
DENO_LOCK_PATCH="$V8X_DIR/tools/deno/$DENO_LOCK_PATCH_NAME"
[[ -f "$DENO_LOCK_PATCH" ]] || fail "missing pinned Deno lock patch: $DENO_LOCK_PATCH"
PRISTINE_DENO_LOCK_SHA256="$(sha256sum "$DENO_DIR/Cargo.lock" | cut -d ' ' -f 1)"
[[ "$PRISTINE_DENO_LOCK_SHA256" = "$EXPECTED_PRISTINE_DENO_LOCK_SHA256" ]] || \
  fail "pristine Deno Cargo.lock digest is $PRISTINE_DENO_LOCK_SHA256, expected $EXPECTED_PRISTINE_DENO_LOCK_SHA256"
git -C "$DENO_DIR" apply --unidiff-zero --check "$DENO_LOCK_PATCH"
git -C "$DENO_DIR" apply --unidiff-zero "$DENO_LOCK_PATCH"
ACTUAL_DENO_LOCK_SHA256="$(sha256sum "$DENO_DIR/Cargo.lock" | cut -d ' ' -f 1)"
[[ "$ACTUAL_DENO_LOCK_SHA256" = "$EXPECTED_DENO_LOCK_SHA256" ]] || \
  fail "patched Deno Cargo.lock digest is $ACTUAL_DENO_LOCK_SHA256, expected $EXPECTED_DENO_LOCK_SHA256"
assert_deno_mutation_boundary "$DENO_DIR"
assert_replay_features "$DENO_DIR" "$CARGO_BIN" "$OUT_DIR/deno-target"

# Lock-parser and tamper controls are compiler-free and do not consume the raw
# Wasm. They ensure the expected rejection cases stay a non-ignored CI gate.
V8X_JS2WASM_POC_V8X_REF="$V8X_REF" \
V8X_JS2WASM_POC_CONTRACT_SHA256="$POC_CONTRACT_SHA256" \
CARGO_TARGET_DIR="$OUT_DIR/replay-unit-target" \
  "$CARGO_BIN" test --manifest-path "$V8X_DIR/Cargo.toml" \
    --locked --no-default-features --features js2wasm_deno_poc_replay \
    --lib deno_poc_replay_tests

V8X_JS2WASM_POC_V8X_REF="$V8X_REF" \
V8X_JS2WASM_POC_CONTRACT_SHA256="$POC_CONTRACT_SHA256" \
CARGO_TARGET_DIR="$OUT_DIR/deno-target" \
  "$CARGO_BIN" build --manifest-path "$DENO_DIR/Cargo.toml" \
    --release --locked -p deno_core --example hello_world
assert_deno_mutation_boundary "$DENO_DIR"
assert_replay_features "$DENO_DIR" "$CARGO_BIN" "$OUT_DIR/deno-target"

HELLO_WORLD="$OUT_DIR/deno-target/release/examples/hello_world"
[[ -x "$HELLO_WORLD" ]] || fail "real Deno hello_world executable is missing: $HELLO_WORLD"
EXPECTED_OUTPUT="$V8X_DIR/tools/deno/$EXPECTED_OUTPUT_NAME"
[[ -f "$EXPECTED_OUTPUT" ]] || fail "expected-output fixture is missing: $EXPECTED_OUTPUT"

# The replay directory is intentionally the process working directory and has
# exactly the three files it is allowed to observe. Raw Wasm and source
# provenance remain outside it for build audit only; neither is passed to env.
REPLAY_DIR="$OUT_DIR/replay"
mkdir "$REPLAY_DIR"
mv "$LOCK" "$REPLAY_DIR/poc-lock.json"
mv "$APP_AOT" "$REPLAY_DIR/deno-core.cwasm"
mv "$PROVIDER_AOT" "$REPLAY_DIR/runtime-eval-provider.cwasm"
mapfile -t replay_files < <(find "$REPLAY_DIR" -mindepth 1 -maxdepth 1 -type f -printf '%f\n' | LC_ALL=C sort)
expected_replay_files=(deno-core.cwasm poc-lock.json runtime-eval-provider.cwasm)
[[ "${replay_files[*]}" = "${expected_replay_files[*]}" ]] || \
  fail "fresh replay directory does not contain exactly manifest plus two AOT artifacts"

ACTUAL_STDOUT="$OUT_DIR/actual.stdout"
ACTUAL_STDERR="$OUT_DIR/actual.stderr"
set +e
(
  cd "$REPLAY_DIR"
  env -i \
    PATH=/usr/bin:/bin \
    V8X_JS2WASM_DENO_POC_MANIFEST="$REPLAY_DIR/poc-lock.json" \
    V8X_JS2WASM_DENO_CORE_AOT_MODULE="$REPLAY_DIR/deno-core.cwasm" \
    V8X_JS2WASM_RUNTIME_EVAL_AOT_MODULE="$REPLAY_DIR/runtime-eval-provider.cwasm" \
    "$HELLO_WORLD"
) >"$ACTUAL_STDOUT" 2>"$ACTUAL_STDERR"
replay_status=$?
set -e
[[ "$replay_status" -eq 0 ]] || {
  cat "$ACTUAL_STDERR" >&2
  fail "fresh compiler-free Deno replay exited $replay_status"
}
cmp -s "$EXPECTED_OUTPUT" "$ACTUAL_STDOUT" || {
  diff -u "$EXPECTED_OUTPUT" "$ACTUAL_STDOUT" >&2 || true
  fail "real Deno hello_world stdout differs from the exact fixture"
}
if grep -Fq "$DIAGNOSTIC_ABORT_TEXT" "$ACTUAL_STDERR"; then
  cat "$ACTUAL_STDERR" >&2
  fail "replay reached a diagnostic ABI abort stub"
fi
[[ ! -s "$ACTUAL_STDERR" ]] || {
  cat "$ACTUAL_STDERR" >&2
  fail "real Deno hello_world wrote unexpected stderr"
}

printf 'v8x Deno POC passed: %s\n' "$REPLAY_DIR"
