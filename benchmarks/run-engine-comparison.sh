#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
output_dir="${V8X_BENCH_OUTPUT_DIR:-$repo_root/target/engine-comparison}"
instances="${V8X_BENCH_INSTANCES:-100}"
repeats="${V8X_BENCH_REPEATS:-5}"
dynamic_add_calls="${V8X_BENCH_SPEED_DYNAMIC_ADD_CALLS:-200000}"
constant_add_calls="${V8X_BENCH_SPEED_CONSTANT_ADD_CALLS:-200000}"
complex_calls="${V8X_BENCH_SPEED_COMPLEX_CALLS:-20000}"
complex_rounds=512
artifact="${V8X_JS2WASM_AOT_MODULE:-$output_dir/engine-footprint.cwasm}"
raw_wasm="$output_dir/engine-footprint.wasm"
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
  if [[ "${V8X_JS2WASM_AOT_OPTIMIZE:-}" != "4" ]]; then
    echo "Set V8X_JS2WASM_AOT_OPTIMIZE=4 to confirm the reused artifact was compiled with js2wasm optimize level 4." >&2
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
  V8X_JS2WASM_OPTIMIZE=4 \
  V8X_JS2WASM_WASM_OUTPUT="$raw_wasm" \
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

echo "Building real V8 runtime..."
CARGO_TARGET_DIR="$output_dir/target-v8" \
  cargo test \
    --manifest-path "$repo_root/benchmarks/v8-baseline/Cargo.toml" \
    --locked --release --test engine_footprint --no-run

js2wasm_binary="$(find_test_binary "$output_dir/target-js2wasm")"
quickjs_binary="$(find_test_binary "$output_dir/target-quickjs")"
v8_binary="$(find_test_binary "$output_dir/target-v8")"
js2wasm_stripped="$output_dir/engine-footprint-js2wasm"
quickjs_stripped="$output_dir/engine-footprint-quickjs"
v8_stripped="$output_dir/engine-footprint-v8"
strip_copy "$js2wasm_binary" "$js2wasm_stripped"
strip_copy "$quickjs_binary" "$quickjs_stripped"
strip_copy "$v8_binary" "$v8_stripped"

js2wasm_runtime_bytes="$(wc -c < "$js2wasm_stripped" | tr -d ' ')"
quickjs_runtime_bytes="$(wc -c < "$quickjs_stripped" | tr -d ' ')"
v8_runtime_bytes="$(wc -c < "$v8_stripped" | tr -d ' ')"
artifact_bytes="$(wc -c < "$artifact" | tr -d ' ')"
js2wasm_combined_bytes="$((js2wasm_runtime_bytes + artifact_bytes))"

: > "$raw_log"
{
  echo "V8X_ENGINE_CONFIG instances=$instances repeats=$repeats dynamic_add_calls=$dynamic_add_calls constant_add_calls=$constant_add_calls complex_calls=$complex_calls complex_rounds=$complex_rounds js2wasm_optimize=4"
  echo "V8X_ENGINE_SIZE engine=v8 executable_bytes=$v8_runtime_bytes artifact_bytes=0 combined_bytes=$v8_runtime_bytes"
  echo "V8X_ENGINE_SIZE engine=quickjs executable_bytes=$quickjs_runtime_bytes artifact_bytes=0 combined_bytes=$quickjs_runtime_bytes"
  echo "V8X_ENGINE_SIZE engine=js2wasm executable_bytes=$js2wasm_runtime_bytes artifact_bytes=$artifact_bytes combined_bytes=$js2wasm_combined_bytes"
} | tee -a "$raw_log"

run_engine() {
  local engine="$1"
  local binary
  local -a command_env=(env)
  case "$engine" in
    v8) binary="$v8_binary" ;;
    quickjs) binary="$quickjs_binary" ;;
    js2wasm)
      binary="$js2wasm_binary"
      command_env+=("V8X_JS2WASM_AOT_MODULE=$artifact")
      ;;
    *) echo "unknown engine: $engine" >&2; return 2 ;;
  esac

  echo "V8X_ENGINE_RUN engine=$engine repeat=$repeat phase=speed" \
    | tee -a "$raw_log"
  "${command_env[@]}" \
    "V8X_BENCH_SPEED_DYNAMIC_ADD_CALLS=$dynamic_add_calls" \
    "V8X_BENCH_SPEED_CONSTANT_ADD_CALLS=$constant_add_calls" \
    "V8X_BENCH_SPEED_COMPLEX_CALLS=$complex_calls" \
    "$binary" --ignored --exact measure_engine_speed --nocapture \
    | tee -a "$raw_log"

  echo "V8X_ENGINE_RUN engine=$engine repeat=$repeat phase=footprint" \
    | tee -a "$raw_log"
  "${command_env[@]}" \
    "V8X_BENCH_INSTANCES=$instances" \
    "$binary" --ignored --exact measure_engine_instances --nocapture \
    | tee -a "$raw_log"
}

engines=(v8 quickjs js2wasm)
for ((repeat = 1; repeat <= repeats; repeat++)); do
  offset="$(((repeat - 1) % ${#engines[@]}))"
  for ((slot = 0; slot < ${#engines[@]}; slot++)); do
    index="$(((offset + slot) % ${#engines[@]}))"
    run_engine "${engines[$index]}"
  done
done

node "$repo_root/benchmarks/summarize-engine-comparison.mjs" "$raw_log" \
  | tee "$output_dir/summary.md"
echo "Raw results: $raw_log"
echo "Summary: $output_dir/summary.md"
