#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
output_dir="${V8X_BENCH_OUTPUT_DIR:-$repo_root/target/engine-comparison}"
instances="${V8X_BENCH_INSTANCES:-100}"
repeats="${V8X_BENCH_REPEATS:-3}"
artifact="${V8X_JS2WASM_AOT_MODULE:-$output_dir/engine-footprint.cwasm}"
raw_log="$output_dir/results.txt"

mkdir -p "$output_dir"

find_test_binary() {
  local target_dir="$1"
  local candidate
  local found=""
  for candidate in "$target_dir"/release/deps/engine_footprint-*; do
    if [[ -f "$candidate" && -x "$candidate" && "$candidate" != *.d \
      && ( -z "$found" || "$candidate" -nt "$found" ) ]]; then
      found="$candidate"
    fi
  done
  if [[ -z "$found" ]]; then
    echo "engine_footprint test executable not found in $target_dir" >&2
    return 1
  fi
  printf '%s\n' "$found"
}

strip_copy() {
  local source="$1"
  local destination="$2"
  cp "$source" "$destination"
  case "$(uname -s)" in
    Darwin) strip -x "$destination" ;;
    Linux) strip --strip-all "$destination" ;;
    *) echo "unsupported platform for binary-size measurement" >&2; return 1 ;;
  esac
}

if [[ -n "${V8X_JS2WASM_AOT_MODULE:-}" ]]; then
  if [[ ! -f "$artifact" ]]; then
    echo "V8X_JS2WASM_AOT_MODULE does not exist: $artifact" >&2
    exit 2
  fi
else
  if [[ -z "${V8X_JS2WASM_COMPILER_SCRIPT:-}" ]]; then
    echo "Set V8X_JS2WASM_COMPILER_SCRIPT or provide V8X_JS2WASM_AOT_MODULE." >&2
    exit 2
  fi
  echo "Generating the trusted js2wasm/Wasmtime AOT artifact..."
  V8X_BENCH_INSTANCES=1 \
  V8X_BENCH_GENERATE_ONLY=1 \
  V8X_JS2WASM_ARTIFACT_OUTPUT="$artifact" \
  CARGO_TARGET_DIR="$output_dir/target-js2wasm-compiler" \
    cargo test --manifest-path "$repo_root/Cargo.toml" --locked --release \
      --no-default-features \
      --features js2wasm_runtime_compile,engine_footprint_bench \
      --test engine_footprint -- \
      --ignored --exact measure_engine_instances --nocapture
fi

echo "Building compiler-free js2wasm runtime..."
CARGO_TARGET_DIR="$output_dir/target-js2wasm" \
  cargo test --manifest-path "$repo_root/Cargo.toml" --locked --release \
    --no-default-features --features engine_js2wasm,engine_footprint_bench \
    --test engine_footprint --no-run

echo "Building QuickJS runtime..."
CARGO_TARGET_DIR="$output_dir/target-quickjs" \
  cargo test --manifest-path "$repo_root/Cargo.toml" --locked --release \
    --no-default-features --features quickjs,engine_footprint_bench \
    --test engine_footprint --no-run

js2wasm_binary="$(find_test_binary "$output_dir/target-js2wasm")"
quickjs_binary="$(find_test_binary "$output_dir/target-quickjs")"
js2wasm_stripped="$output_dir/engine-footprint-js2wasm"
quickjs_stripped="$output_dir/engine-footprint-quickjs"
strip_copy "$js2wasm_binary" "$js2wasm_stripped"
strip_copy "$quickjs_binary" "$quickjs_stripped"

js2wasm_runtime_bytes="$(wc -c < "$js2wasm_stripped" | tr -d ' ')"
quickjs_runtime_bytes="$(wc -c < "$quickjs_stripped" | tr -d ' ')"
artifact_bytes="$(wc -c < "$artifact" | tr -d ' ')"
js2wasm_combined_bytes="$((js2wasm_runtime_bytes + artifact_bytes))"

: > "$raw_log"
{
  echo "V8X_ENGINE_SIZE engine=quickjs executable_bytes=$quickjs_runtime_bytes artifact_bytes=0 combined_bytes=$quickjs_runtime_bytes"
  echo "V8X_ENGINE_SIZE engine=js2wasm executable_bytes=$js2wasm_runtime_bytes artifact_bytes=$artifact_bytes combined_bytes=$js2wasm_combined_bytes"
} | tee -a "$raw_log"

for ((repeat = 1; repeat <= repeats; repeat++)); do
  echo "V8X_ENGINE_RUN engine=quickjs repeat=$repeat" | tee -a "$raw_log"
  V8X_BENCH_INSTANCES="$instances" \
    "$quickjs_binary" --ignored --exact measure_engine_instances --nocapture \
    | tee -a "$raw_log"

  echo "V8X_ENGINE_RUN engine=js2wasm repeat=$repeat" | tee -a "$raw_log"
  V8X_BENCH_INSTANCES="$instances" \
  V8X_JS2WASM_AOT_MODULE="$artifact" \
    "$js2wasm_binary" --ignored --exact measure_engine_instances --nocapture \
    | tee -a "$raw_log"
done

echo "Raw results: $raw_log"
