//! Experimental js2wasm module backend.
//!
//! `engine_js2wasm` embeds compiler-free Wasmtime and accepts only precompiled
//! artifacts. `engine_js2wasm_runtime` adds the external compiler and a
//! content-addressed native-artifact cache for graphs discovered after startup.
//! Both profiles share one engine plus each precompiled module across isolates,
//! and keep a private store/instance alive with each owning V8 module handle.
//! The first Deno-shaped host seam is `Deno.cwd()`: the compiled TypeScript
//! wrapper reconstructs its string from two direct UTF-16 imports, avoiding a
//! JavaScript-host `externref` ABI or a WASI/component boundary.

#[cfg(all(
  feature = "js2wasm_deno_poc_replay",
  any(
    feature = "js2wasm_runtime_compile",
    feature = "engine_quickjs",
    feature = "link_quickjs",
    feature = "engine_jsc",
    feature = "vendor_jsc",
    feature = "system_jsc",
  ),
))]
compile_error!(
  "`js2wasm_deno_poc_replay` is compiler-free and cannot be combined with \
   `js2wasm_runtime_compile`, QuickJS, or JSC backend features"
);

#[cfg(feature = "js2wasm_deno_poc_replay")]
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::Read;
#[cfg(feature = "js2wasm_runtime_compile")]
use std::io::Write;
use std::path::{Path, PathBuf};
#[cfg(feature = "js2wasm_runtime_compile")]
use std::process::Command;
#[cfg(feature = "js2wasm_runtime_compile")]
use std::sync::atomic::AtomicU64;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};
#[cfg(feature = "js2wasm_runtime_compile")]
use std::time::{SystemTime, UNIX_EPOCH};
use wasmtime::{
  Caller, Config, Engine, Instance, InstancePre, Linker, Module, Store,
};
#[cfg(feature = "js2wasm_runtime_compile")]
use wasmtime::OptLevel;

#[cfg(feature = "js2wasm_runtime_compile")]
static NEXT_TEMP_ID: AtomicU64 = AtomicU64::new(0);

const CWD_LENGTH_IMPORT: &str = "__v8x_op_cwd_utf16_length";
const CWD_CODE_UNIT_IMPORT: &str = "__v8x_op_cwd_utf16_code_unit";
const DENO_SUM_BEGIN_IMPORT: &str = "__v8x_deno_sum_begin";
const DENO_SUM_VALUE_IMPORT: &str = "__v8x_deno_sum_value";
const DENO_SUM_END_IMPORT: &str = "__v8x_deno_sum_end";
const DENO_ERROR_KIND_IMPORT: &str = "__v8x_deno_error_kind";
const DENO_ERROR_LENGTH_IMPORT: &str = "__v8x_deno_error_utf16_length";
const DENO_ERROR_CODE_UNIT_IMPORT: &str = "__v8x_deno_error_utf16_code_unit";
const DENO_PRINT_BEGIN_IMPORT: &str = "__v8x_deno_print_begin";
const DENO_PRINT_CODE_UNIT_IMPORT: &str = "__v8x_deno_print_code_unit";
const DENO_PRINT_END_IMPORT: &str = "__v8x_deno_print_end";
const DENO_SCRIPT_LENGTH_IMPORT: &str = "__v8x_deno_script_utf16_length";
const DENO_SCRIPT_CODE_UNIT_IMPORT: &str = "__v8x_deno_script_utf16_code_unit";
const DENO_TEST_FN_CALL_IMPORT: &str = "__v8x_deno_test_fn_call";
const DENO_TEST_FN_RESULT_LENGTH_IMPORT: &str =
  "__v8x_deno_test_fn_result_utf16_length";
const DENO_TEST_FN_RESULT_CODE_UNIT_IMPORT: &str =
  "__v8x_deno_test_fn_result_utf16_code_unit";
const DENO_IMPORT_MODULE: &str = "v8x:deno";
const RUNTIME_EVAL_IMPORT_MODULE: &str = "js2wasm:runtime-eval";
const RUNTIME_EVAL_JSON_IMPORT_MODULE: &str = "v8x:runtime-eval-json";
const RUNTIME_EVAL_IMPORTS: &[&str] = &[
  "__runtime_apply_interpreted",
  "__runtime_new_function",
  "__runtime_indirect_eval",
  "__runtime_direct_eval",
  "__v8x_runtime_eval_json",
];
const RUNTIME_EVAL_PROVIDER_EXPORTS: &[&str] = &[
  "__runtime_apply_interpreted",
  "__runtime_new_function",
  "__runtime_indirect_eval",
  "__runtime_direct_eval",
];
#[cfg(not(feature = "js2wasm_deno_poc_replay"))]
const RUNTIME_EVAL_AOT_MODULE_ENV: &str = "V8X_JS2WASM_RUNTIME_EVAL_AOT_MODULE";
#[cfg(feature = "js2wasm_deno_poc_replay")]
const DENO_POC_MANIFEST_ENV: &str = "V8X_JS2WASM_DENO_POC_MANIFEST";
#[cfg(feature = "js2wasm_deno_poc_replay")]
const DENO_POC_APP_AOT_MODULE_ENV: &str = "V8X_JS2WASM_DENO_CORE_AOT_MODULE";
#[cfg(feature = "js2wasm_deno_poc_replay")]
const DENO_POC_PROVIDER_AOT_MODULE_ENV: &str =
  "V8X_JS2WASM_RUNTIME_EVAL_AOT_MODULE";
#[cfg(feature = "js2wasm_deno_poc_replay")]
const POC_EXPECTED_DENO_REF: &str = "1d4e6c1cb855b62a7fb572c6c138e4e8b4e7fa44";
#[cfg(feature = "js2wasm_deno_poc_replay")]
const POC_EXPECTED_JS2_REF: &str = "9bda388e593cbf9631dc7c4f2c4016685d357587";
#[cfg(feature = "js2wasm_deno_poc_replay")]
const POC_EXPECTED_WASMTIME_VERSION: &str = "47.0.3";
#[cfg(feature = "js2wasm_deno_poc_replay")]
const POC_EXPECTED_TARGET_OS: &str = "linux";
#[cfg(feature = "js2wasm_deno_poc_replay")]
const POC_EXPECTED_TARGET_ARCH: &str = "x86_64";
#[cfg(feature = "js2wasm_deno_poc_replay")]
const POC_EXPECTED_TARGET_TRIPLE: &str = "x86_64-unknown-linux-gnu";
#[cfg(feature = "js2wasm_deno_poc_replay")]
const POC_EXPECTED_COMPILE_OPTIONS_SHA256: &str =
  "a31c09c7e31b4852799975e9c8cb8d132aad6ecab79bbf8c98d5848f7c3bde9e";
#[cfg(feature = "js2wasm_deno_poc_replay")]
const POC_V8X_REF: Option<&str> = option_env!("V8X_JS2WASM_POC_V8X_REF");
#[cfg(feature = "js2wasm_deno_poc_replay")]
const POC_CONTRACT_SHA256: Option<&str> =
  option_env!("V8X_JS2WASM_POC_CONTRACT_SHA256");
#[cfg(feature = "js2wasm_deno_poc_replay")]
const POC_SOURCE_LOCK: [(&str, u64, &str); 6] = [
  (
    "00_primordials.js",
    19_076,
    "5a2dfbdc4bb81412575d035901a11788001c7e0110e3f736d16289891af44a52",
  ),
  (
    "00_infra.js",
    17_520,
    "33984000be930f3b02a2d1149ac0319724e8d95891623c8cc74699da4ce97287",
  ),
  (
    "02_timers.js",
    10_932,
    "305596528c679be30d0ac61fa049ec0f1777c287054d119ff4b341575afac7f9",
  ),
  (
    "01_core.js",
    39_939,
    "6e67972322cc5385a2b642a4f7e941fccb6f992c9de662a5111d11fd0aaf1a3a",
  ),
  (
    "mod.js",
    342,
    "6850db621a5325d8737ad87d2d24cbc35b7010d5e5f36c88dc53c16610cc40e5",
  ),
  (
    "hello_world_usage.js",
    339,
    "33bf6b9698833319ad98c0cf88f2fb4dd7634859816ec784aa8902b3eeba1804",
  ),
];
#[cfg(feature = "js2wasm_deno_poc_replay")]
const POC_REPLAY_FORBIDDEN_ENV: &[&str] = &[
  "V8X_JS2WASM_AOT_MODULE",
  "V8X_JS2WASM_ARTIFACT_OUTPUT",
  "V8X_JS2WASM_CACHE_DIR",
  "V8X_JS2WASM_COMPILER",
  "V8X_JS2WASM_COMPILER_ID",
  "V8X_JS2WASM_COMPILER_SCRIPT",
  "V8X_JS2WASM_DENO_CORE_AOT_ATTESTATION",
  "V8X_JS2WASM_DENO_CORE_AOT_OUTPUT",
  "V8X_JS2WASM_DENO_CORE_WASM",
  "V8X_JS2WASM_RUNTIME_EVAL_AOT_ATTESTATION",
  "V8X_JS2WASM_RUNTIME_EVAL_AOT_OUTPUT",
  "V8X_JS2WASM_RUNTIME_EVAL_WASM",
  "V8X_JS2WASM_WORKDIR",
];
#[cfg(feature = "js2wasm_runtime_compile")]
const RUNTIME_EVAL_WASM_ENV: &str = "V8X_JS2WASM_RUNTIME_EVAL_WASM";
#[cfg(feature = "js2wasm_runtime_compile")]
const RUNTIME_EVAL_AOT_OUTPUT_ENV: &str = "V8X_JS2WASM_RUNTIME_EVAL_AOT_OUTPUT";
const GRAPH_BINDING_SUFFIX: &str = ".graph-sha256";
const GRAPH_BINDING_DOMAIN: &[u8] = b"v8x/js2wasm graph binding v1\0";
const CWD_LENGTH_PROBE: &str = "__v8x_probe_cwd_utf16_length";
const CWD_CHECKSUM_PROBE: &str = "__v8x_probe_cwd_utf16_checksum";
const DENO_CORE_BOOTSTRAP_PROBE: &str = "__v8x_probe_deno_core_bootstrap";
const RUNTIME_EVAL_STATE_PROBE: &str = "__v8x_probe_runtime_eval_state";
const RUNTIME_EVAL_STATE_PROBE_ENV: &str =
  "V8X_JS2WASM_VERIFY_RUNTIME_EVAL_STATE";
#[cfg(not(feature = "js2wasm_deno_poc_replay"))]
const DENO_CORE_WRAPPERS_STAGE: &str = "__v8x_stage_deno_core_wrappers";
#[cfg(not(feature = "js2wasm_deno_poc_replay"))]
const DENO_CORE_MODULE_STAGE: &str = "__v8x_stage_deno_core_module";
#[cfg(not(feature = "js2wasm_deno_poc_replay"))]
const DENO_CORE_USAGE_STAGE: &str = "__v8x_stage_deno_hello_world_usage";
#[cfg(not(feature = "js2wasm_deno_poc_replay"))]
const DENO_CORE_STAGE_STATE_PROBE: &str = "__v8x_probe_deno_stage_state";
#[cfg(not(feature = "js2wasm_deno_poc_replay"))]
const DENO_CORE_RUNTIME_USAGE_STAGE_PROBE: &str =
  "__v8x_probe_deno_runtime_usage_stage";
const DENO_CORE_SET_TICK_INFO: &str = "__v8x_set_deno_tick_info";
const DENO_CORE_SET_IMMEDIATE_INFO: &str = "__v8x_set_deno_immediate_info";
const DENO_CORE_SET_TIMER_INFO: &str = "__v8x_set_deno_timer_info";
const DENO_RUN_CLASSIC_SCRIPT: &str = "__v8x_run_classic_script";
const DENO_SCRIPT_RESULT_LENGTH: &str = "__v8x_script_result_utf16_length";
const DENO_SCRIPT_RESULT_CODE_UNIT: &str =
  "__v8x_script_result_utf16_code_unit";
#[cfg(feature = "js2wasm_runtime_compile")]
const DENO_CORE_AOT_OUTPUT_ENV: &str = "V8X_JS2WASM_DENO_CORE_AOT_OUTPUT";
#[cfg(feature = "js2wasm_deno_poc")]
const DENO_CORE_AOT_ATTESTATION_ENV: &str =
  "V8X_JS2WASM_DENO_CORE_AOT_ATTESTATION";
#[cfg(feature = "js2wasm_deno_poc")]
const RUNTIME_EVAL_AOT_ATTESTATION_ENV: &str =
  "V8X_JS2WASM_RUNTIME_EVAL_AOT_ATTESTATION";
#[cfg(feature = "js2wasm_runtime_compile")]
const RUNTIME_CACHE_DIR_ENV: &str = "V8X_JS2WASM_CACHE_DIR";
#[cfg(feature = "js2wasm_runtime_compile")]
const RUNTIME_COMPILER_ID_ENV: &str = "V8X_JS2WASM_COMPILER_ID";
#[cfg(feature = "js2wasm_runtime_compile")]
const RUNTIME_CACHE_DOMAIN: &[u8] = b"v8x/js2wasm runtime cache v1\0";
const DEFERRED_BOOTSTRAP_IMPORTS: &[(&str, &str)] = &[
  ("env", "Promise_new"),
  ("env", "Promise_all"),
  ("env", "Promise_allSettled"),
  ("env", "Promise_any"),
  ("env", "Promise_race"),
];

const DENO_HOST_IMPORTS: &[&str] = &[
  CWD_LENGTH_IMPORT,
  CWD_CODE_UNIT_IMPORT,
  DENO_SUM_BEGIN_IMPORT,
  DENO_SUM_VALUE_IMPORT,
  DENO_SUM_END_IMPORT,
  DENO_ERROR_KIND_IMPORT,
  DENO_ERROR_LENGTH_IMPORT,
  DENO_ERROR_CODE_UNIT_IMPORT,
  DENO_PRINT_BEGIN_IMPORT,
  DENO_PRINT_CODE_UNIT_IMPORT,
  DENO_PRINT_END_IMPORT,
  DENO_SCRIPT_LENGTH_IMPORT,
  DENO_SCRIPT_CODE_UNIT_IMPORT,
  DENO_TEST_FN_CALL_IMPORT,
  DENO_TEST_FN_RESULT_LENGTH_IMPORT,
  DENO_TEST_FN_RESULT_CODE_UNIT_IMPORT,
];

#[cfg(feature = "js2wasm_deno_poc_replay")]
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DenoPocManifest {
  schema_version: u32,
  contract_sha256: String,
  raw_contract_sha256: String,
  deno_ref: String,
  js2_ref: String,
  v8x_ref: String,
  wasmtime_version: String,
  target: DenoPocTarget,
  engine_config: DenoPocEngineConfig,
  compile_options_sha256: String,
  sources: [DenoPocSource; 6],
  artifacts: DenoPocArtifacts,
}

#[cfg(feature = "js2wasm_deno_poc_replay")]
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DenoPocTarget {
  os: String,
  arch: String,
  triple: String,
}

#[cfg(feature = "js2wasm_deno_poc_replay")]
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DenoPocEngineConfig {
  wasm_function_references: bool,
  wasm_gc: bool,
  wasm_tail_call: bool,
  wasm_exceptions: bool,
}

#[cfg(feature = "js2wasm_deno_poc_replay")]
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DenoPocSource {
  path: String,
  bytes: u64,
  sha256: String,
}

#[cfg(feature = "js2wasm_deno_poc_replay")]
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DenoPocArtifacts {
  app: DenoPocArtifact,
  runtime_eval_provider: DenoPocArtifact,
}

#[cfg(feature = "js2wasm_deno_poc_replay")]
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DenoPocArtifact {
  role: String,
  raw_path: String,
  raw_sha256: String,
  aot_sha256: String,
  attestation_sha256: String,
}

/// An AOT payload read once and hash-checked against the replay lock. The
/// bytes, rather than their path, are the only thing later handed to
/// `Module::deserialize`, so replacing the file after validation is harmless.
#[cfg(feature = "js2wasm_deno_poc_replay")]
#[derive(Debug)]
struct VerifiedDenoPocArtifact {
  bytes: Vec<u8>,
  sha256: String,
}

#[cfg(feature = "js2wasm_deno_poc_replay")]
struct DenoPocReplayArtifacts {
  app: VerifiedDenoPocArtifact,
  runtime_eval_provider: VerifiedDenoPocArtifact,
}

#[cfg(feature = "js2wasm_deno_poc_replay")]
fn is_lowercase_sha256(value: &str) -> bool {
  value.len() == 64
    && value
      .bytes()
      .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[cfg(feature = "js2wasm_deno_poc_replay")]
fn is_full_lowercase_git_ref(value: &str) -> bool {
  value.len() == 40
    && value
      .bytes()
      .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[cfg(feature = "js2wasm_deno_poc_replay")]
fn require_poc_sha256(field: &str, value: &str) -> Result<(), String> {
  if is_lowercase_sha256(value) {
    Ok(())
  } else {
    Err(format!(
      "Deno POC manifest field {field} must be 64 lowercase hexadecimal SHA-256 digits"
    ))
  }
}

#[cfg(feature = "js2wasm_deno_poc_replay")]
fn append_canonical_poc_json(
  value: &serde_json::Value,
  output: &mut String,
) -> Result<(), String> {
  match value {
    serde_json::Value::Null => output.push_str("null"),
    serde_json::Value::Bool(value) => {
      output.push_str(if *value { "true" } else { "false" })
    }
    serde_json::Value::Number(value) => output.push_str(&value.to_string()),
    serde_json::Value::String(value) => {
      output.push_str(&serde_json::to_string(value).map_err(|error| {
        format!("serialize Deno POC contract string: {error}")
      })?)
    }
    serde_json::Value::Array(values) => {
      output.push('[');
      for (index, value) in values.iter().enumerate() {
        if index != 0 {
          output.push(',');
        }
        append_canonical_poc_json(value, output)?;
      }
      output.push(']');
    }
    serde_json::Value::Object(values) => {
      output.push('{');
      let mut entries = values.iter().collect::<Vec<_>>();
      entries.sort_unstable_by(|(left, _), (right, _)| left.cmp(right));
      for (index, (key, value)) in entries.into_iter().enumerate() {
        if index != 0 {
          output.push(',');
        }
        output.push_str(&serde_json::to_string(key).map_err(|error| {
          format!("serialize Deno POC contract object key: {error}")
        })?);
        output.push(':');
        append_canonical_poc_json(value, output)?;
      }
      output.push('}');
    }
  }
  Ok(())
}

#[cfg(feature = "js2wasm_deno_poc_replay")]
fn poc_contract_sha256(value: &serde_json::Value) -> Result<String, String> {
  let mut preimage = value.clone();
  let object = preimage.as_object_mut().ok_or_else(|| {
    "Deno POC manifest must be a JSON object to compute contract_sha256"
      .to_string()
  })?;
  object.remove("contract_sha256");
  let mut canonical_json = String::new();
  append_canonical_poc_json(&preimage, &mut canonical_json)?;
  Ok(bytes_digest(canonical_json.as_bytes()))
}

#[cfg(feature = "js2wasm_deno_poc_replay")]
fn validate_poc_artifact(
  artifact: &DenoPocArtifact,
  expected_role: &str,
  expected_raw_path: &str,
) -> Result<(), String> {
  if artifact.role != expected_role {
    return Err(format!(
      "Deno POC manifest artifact role must be {expected_role:?}, found {:?}",
      artifact.role,
    ));
  }
  if artifact.raw_path != expected_raw_path {
    return Err(format!(
      "Deno POC manifest {expected_role} raw_path must be {expected_raw_path:?}, found {:?}",
      artifact.raw_path,
    ));
  }
  require_poc_sha256(
    &format!("artifacts.{expected_role}.raw_sha256"),
    &artifact.raw_sha256,
  )?;
  require_poc_sha256(
    &format!("artifacts.{expected_role}.aot_sha256"),
    &artifact.aot_sha256,
  )?;
  require_poc_sha256(
    &format!("artifacts.{expected_role}.attestation_sha256"),
    &artifact.attestation_sha256,
  )
}

#[cfg(feature = "js2wasm_deno_poc_replay")]
fn expected_poc_attestation_sha256(
  manifest: &DenoPocManifest,
  artifact: &DenoPocArtifact,
) -> Result<String, String> {
  let attestation = serde_json::json!({
    "schema_version": 1,
    "role": artifact.role,
    "wasmtime_version": manifest.wasmtime_version,
    "target": {
      "os": manifest.target.os,
      "arch": manifest.target.arch,
      "triple": manifest.target.triple,
    },
    "engine_config": {
      "wasm_function_references": manifest.engine_config.wasm_function_references,
      "wasm_gc": manifest.engine_config.wasm_gc,
      "wasm_tail_call": manifest.engine_config.wasm_tail_call,
      "wasm_exceptions": manifest.engine_config.wasm_exceptions,
    },
    "raw_sha256": artifact.raw_sha256,
    "aot_sha256": artifact.aot_sha256,
  });
  let mut canonical_json = String::new();
  append_canonical_poc_json(&attestation, &mut canonical_json)?;
  Ok(bytes_digest(canonical_json.as_bytes()))
}

#[cfg(feature = "js2wasm_deno_poc_replay")]
fn validate_poc_engine_config(
  engine_config: &DenoPocEngineConfig,
) -> Result<(), String> {
  for (name, enabled) in [
    (
      "wasm_function_references",
      engine_config.wasm_function_references,
    ),
    ("wasm_gc", engine_config.wasm_gc),
    ("wasm_tail_call", engine_config.wasm_tail_call),
    ("wasm_exceptions", engine_config.wasm_exceptions),
  ] {
    if !enabled {
      return Err(format!(
        "Deno POC manifest engine_config.{name} must be true"
      ));
    }
  }
  Ok(())
}

#[cfg(feature = "js2wasm_deno_poc_replay")]
fn validate_deno_poc_manifest(
  manifest: &DenoPocManifest,
  computed_contract_sha256: &str,
  compiled_v8x_ref: Option<&str>,
  compiled_contract_sha256: Option<&str>,
) -> Result<(), String> {
  if manifest.schema_version != 1 {
    return Err(format!(
      "Deno POC manifest schema_version must be 1, found {}",
      manifest.schema_version,
    ));
  }
  require_poc_sha256("contract_sha256", &manifest.contract_sha256)?;
  require_poc_sha256("raw_contract_sha256", &manifest.raw_contract_sha256)?;
  if manifest.contract_sha256 != computed_contract_sha256 {
    return Err(format!(
      "Deno POC manifest contract_sha256 does not match its canonical lock preimage: expected {computed_contract_sha256}, found {}",
      manifest.contract_sha256,
    ));
  }
  if manifest.deno_ref != POC_EXPECTED_DENO_REF {
    return Err(format!(
      "Deno POC manifest deno_ref mismatch: expected {POC_EXPECTED_DENO_REF}, found {}",
      manifest.deno_ref,
    ));
  }
  if manifest.js2_ref != POC_EXPECTED_JS2_REF {
    return Err(format!(
      "Deno POC manifest js2_ref mismatch: expected {POC_EXPECTED_JS2_REF}, found {}",
      manifest.js2_ref,
    ));
  }
  let compiled_v8x_ref = compiled_v8x_ref.ok_or_else(|| {
    "Deno POC replay was built without V8X_JS2WASM_POC_V8X_REF; rebuild with the exact v8x Git revision"
      .to_string()
  })?;
  if !is_full_lowercase_git_ref(compiled_v8x_ref) {
    return Err(
      "compile-time V8X_JS2WASM_POC_V8X_REF must be a full lowercase Git revision"
        .to_string(),
    );
  }
  if !is_full_lowercase_git_ref(&manifest.v8x_ref) {
    return Err(
      "Deno POC manifest v8x_ref must be a full lowercase Git revision"
        .to_string(),
    );
  }
  if manifest.v8x_ref != compiled_v8x_ref {
    return Err(format!(
      "Deno POC manifest v8x_ref mismatch: crate was built for {compiled_v8x_ref}, manifest has {}",
      manifest.v8x_ref,
    ));
  }
  if manifest.wasmtime_version != POC_EXPECTED_WASMTIME_VERSION {
    return Err(format!(
      "Deno POC manifest wasmtime_version mismatch: expected {POC_EXPECTED_WASMTIME_VERSION}, found {}",
      manifest.wasmtime_version,
    ));
  }
  if manifest.target.os != POC_EXPECTED_TARGET_OS
    || manifest.target.arch != POC_EXPECTED_TARGET_ARCH
    || manifest.target.triple != POC_EXPECTED_TARGET_TRIPLE
  {
    return Err(format!(
      "Deno POC manifest target mismatch: expected {}/{}/{}, found {}/{}/{}",
      POC_EXPECTED_TARGET_OS,
      POC_EXPECTED_TARGET_ARCH,
      POC_EXPECTED_TARGET_TRIPLE,
      manifest.target.os,
      manifest.target.arch,
      manifest.target.triple,
    ));
  }
  validate_poc_engine_config(&manifest.engine_config)?;
  if manifest.compile_options_sha256 != POC_EXPECTED_COMPILE_OPTIONS_SHA256 {
    return Err(format!(
      "Deno POC manifest compile_options_sha256 mismatch: expected {POC_EXPECTED_COMPILE_OPTIONS_SHA256}, found {}",
      manifest.compile_options_sha256,
    ));
  }
  for (index, (expected, actual)) in
    POC_SOURCE_LOCK.iter().zip(&manifest.sources).enumerate()
  {
    let (expected_path, expected_bytes, expected_sha256) = *expected;
    if actual.path != expected_path {
      return Err(format!(
        "Deno POC manifest sources[{index}].path mismatch: expected {expected_path:?}, found {:?}",
        actual.path,
      ));
    }
    if actual.bytes != expected_bytes {
      return Err(format!(
        "Deno POC manifest sources[{index}].bytes mismatch for {expected_path}: expected {expected_bytes}, found {}",
        actual.bytes,
      ));
    }
    if actual.sha256 != expected_sha256 {
      return Err(format!(
        "Deno POC manifest sources[{index}].sha256 mismatch for {expected_path}: expected {expected_sha256}, found {}",
        actual.sha256,
      ));
    }
  }
  validate_poc_artifact(&manifest.artifacts.app, "app", "deno-core.wasm")?;
  validate_poc_artifact(
    &manifest.artifacts.runtime_eval_provider,
    "runtime_eval_provider",
    "runtime-eval-provider.wasm",
  )?;
  for artifact in [
    &manifest.artifacts.app,
    &manifest.artifacts.runtime_eval_provider,
  ] {
    let expected = expected_poc_attestation_sha256(manifest, artifact)?;
    if artifact.attestation_sha256 != expected {
      return Err(format!(
        "Deno POC manifest {} attestation_sha256 mismatch: expected {expected}, found {}",
        artifact.role, artifact.attestation_sha256,
      ));
    }
  }
  let compiled_contract_sha256 = compiled_contract_sha256.ok_or_else(|| {
    "Deno POC replay was built without V8X_JS2WASM_POC_CONTRACT_SHA256; rebuild with the exact final replay contract digest"
      .to_string()
  })?;
  if !is_lowercase_sha256(compiled_contract_sha256) {
    return Err(
      "compile-time V8X_JS2WASM_POC_CONTRACT_SHA256 must be 64 lowercase hexadecimal SHA-256 digits"
        .to_string(),
    );
  }
  if manifest.contract_sha256 != compiled_contract_sha256 {
    return Err(format!(
      "Deno POC manifest contract_sha256 mismatch: crate was built for {compiled_contract_sha256}, manifest has {}",
      manifest.contract_sha256,
    ));
  }
  Ok(())
}

#[cfg(feature = "js2wasm_deno_poc_replay")]
fn parse_deno_poc_manifest_with_refs(
  contents: &str,
  compiled_v8x_ref: Option<&str>,
  compiled_contract_sha256: Option<&str>,
) -> Result<DenoPocManifest, String> {
  let value: serde_json::Value = serde_json::from_str(contents)
    .map_err(|error| format!("parse Deno POC manifest JSON: {error}"))?;
  let computed_contract_sha256 = poc_contract_sha256(&value)?;
  let manifest: DenoPocManifest = serde_json::from_value(value)
    .map_err(|error| format!("parse Deno POC manifest JSON: {error}"))?;
  validate_deno_poc_manifest(
    &manifest,
    &computed_contract_sha256,
    compiled_v8x_ref,
    compiled_contract_sha256,
  )?;
  Ok(manifest)
}

#[cfg(feature = "js2wasm_deno_poc_replay")]
fn parse_deno_poc_manifest(contents: &str) -> Result<DenoPocManifest, String> {
  parse_deno_poc_manifest_with_refs(contents, POC_V8X_REF, POC_CONTRACT_SHA256)
}

#[cfg(feature = "js2wasm_deno_poc_replay")]
fn reject_poc_replay_compiler_envs<F>(mut is_set: F) -> Result<(), String>
where
  F: FnMut(&str) -> bool,
{
  if let Some(name) = POC_REPLAY_FORBIDDEN_ENV
    .iter()
    .copied()
    .find(|name| is_set(name))
  {
    return Err(format!(
      "Deno POC replay rejects compiler-related environment variable {name}"
    ));
  }
  Ok(())
}

#[cfg(feature = "js2wasm_deno_poc_replay")]
fn required_poc_replay_path(env: &str) -> Result<PathBuf, String> {
  let value = std::env::var_os(env).ok_or_else(|| {
    format!("Deno POC replay requires {env} to name a verified AOT artifact")
  })?;
  if value.is_empty() {
    return Err(format!(
      "Deno POC replay requires non-empty {env} to name a verified AOT artifact"
    ));
  }
  Ok(PathBuf::from(value))
}

#[cfg(feature = "js2wasm_deno_poc_replay")]
fn read_verified_poc_aot(
  artifact: &Path,
  expected_sha256: &str,
  label: &str,
) -> Result<VerifiedDenoPocArtifact, String> {
  let bytes = fs::read(artifact).map_err(|error| {
    format!(
      "read verified Deno POC {label} AOT artifact {}: {error}",
      artifact.display(),
    )
  })?;
  if bytes.is_empty() {
    return Err(format!(
      "verified Deno POC {label} AOT artifact {} is empty",
      artifact.display(),
    ));
  }
  let actual_sha256 = bytes_digest(&bytes);
  if actual_sha256 != expected_sha256 {
    return Err(format!(
      "Deno POC {label} AOT artifact hash mismatch for {}: expected {expected_sha256}, found {actual_sha256}",
      artifact.display(),
    ));
  }
  Ok(VerifiedDenoPocArtifact {
    bytes,
    sha256: actual_sha256,
  })
}

#[cfg(feature = "js2wasm_deno_poc_replay")]
impl DenoPocReplayArtifacts {
  fn from_env() -> Result<Self, String> {
    reject_poc_replay_compiler_envs(|name| std::env::var_os(name).is_some())?;
    let manifest_path =
      std::env::var_os(DENO_POC_MANIFEST_ENV).ok_or_else(|| {
        format!("Deno POC replay requires {DENO_POC_MANIFEST_ENV}")
      })?;
    if manifest_path.is_empty() {
      return Err(format!(
        "Deno POC replay requires non-empty {DENO_POC_MANIFEST_ENV}"
      ));
    }
    let manifest_path = PathBuf::from(manifest_path);
    let manifest_contents =
      fs::read_to_string(&manifest_path).map_err(|error| {
        format!(
          "read Deno POC manifest {}: {error}",
          manifest_path.display(),
        )
      })?;
    let manifest = parse_deno_poc_manifest(&manifest_contents)?;
    let app_path = required_poc_replay_path(DENO_POC_APP_AOT_MODULE_ENV)?;
    let provider_path =
      required_poc_replay_path(DENO_POC_PROVIDER_AOT_MODULE_ENV)?;
    // Read and verify both AOT payloads before any caller can deserialize one
    // of them. Replay never maps a path through `deserialize_file`.
    let app = read_verified_poc_aot(
      &app_path,
      &manifest.artifacts.app.aot_sha256,
      "application",
    )?;
    let runtime_eval_provider = read_verified_poc_aot(
      &provider_path,
      &manifest.artifacts.runtime_eval_provider.aot_sha256,
      "runtime-eval provider",
    )?;
    Ok(Self {
      app,
      runtime_eval_provider,
    })
  }
}

// The exact bootstrap fixture is tiny. Keep malformed or adversarial callers
// from asking the embedding process for an unbounded transaction allocation;
// exceeding this explicit protocol limit traps instead of aborting on OOM.
const MAX_DENO_SCALAR_ITEMS: usize = 1 << 20;

fn graph_binding_path(artifact: &Path) -> PathBuf {
  let mut path = artifact.as_os_str().to_os_string();
  path.push(GRAPH_BINDING_SUFFIX);
  PathBuf::from(path)
}

fn update_graph_digest(hasher: &mut Sha256, bytes: &[u8]) {
  let length = u64::try_from(bytes.len())
    .expect("a supported target cannot address more than u64::MAX bytes");
  hasher.update(length.to_le_bytes());
  hasher.update(bytes);
}

fn graph_digest(entry: &str, modules: &[SourceModule]) -> String {
  let mut hasher = Sha256::new();
  hasher.update(GRAPH_BINDING_DOMAIN);
  update_graph_digest(&mut hasher, entry.as_bytes());
  let module_count = u64::try_from(modules.len())
    .expect("a supported target cannot address more than u64::MAX modules");
  update_graph_digest(&mut hasher, &module_count.to_le_bytes());
  for module in modules {
    update_graph_digest(&mut hasher, module.specifier.as_bytes());
    update_graph_digest(&mut hasher, module.source.as_bytes());
  }
  format!("{:x}", hasher.finalize())
}

fn bytes_digest(bytes: &[u8]) -> String {
  format!("{:x}", Sha256::digest(bytes))
}

fn artifact_digest(artifact: &Path) -> Result<String, String> {
  let mut file = fs::File::open(artifact).map_err(|error| {
    format!(
      "read precompiled js2wasm artifact {} for graph binding: {error}",
      artifact.display(),
    )
  })?;
  let mut hasher = Sha256::new();
  let mut buffer = [0_u8; 64 * 1024];
  loop {
    let read = file.read(&mut buffer).map_err(|error| {
      format!(
        "read precompiled js2wasm artifact {} for graph binding: {error}",
        artifact.display(),
      )
    })?;
    if read == 0 {
      break;
    }
    hasher.update(&buffer[..read]);
  }
  Ok(format!("{:x}", hasher.finalize()))
}

fn parse_graph_binding(contents: &str) -> Result<(&str, &str), String> {
  let mut lines = contents.lines();
  let graph = lines
    .next()
    .and_then(|line| line.strip_prefix("graph-sha256 "))
    .ok_or_else(|| "missing graph-sha256 field".to_string())?;
  let artifact = lines
    .next()
    .and_then(|line| line.strip_prefix("artifact-sha256 "))
    .ok_or_else(|| "missing artifact-sha256 field".to_string())?;
  if lines.next().is_some() {
    return Err("unexpected additional fields".to_string());
  }
  let valid_digest = |digest: &str| {
    digest.len() == 64
      && digest
        .bytes()
        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
  };
  if !valid_digest(graph) || !valid_digest(artifact) {
    return Err(
      "SHA-256 fields must be 64 lowercase hexadecimal digits".to_string(),
    );
  }
  Ok((graph, artifact))
}

fn write_graph_binding(
  artifact: &Path,
  artifact_bytes: &[u8],
  entry: &str,
  modules: &[SourceModule],
) -> Result<(), String> {
  let binding = graph_binding_path(artifact);
  let graph = graph_digest(entry, modules);
  let artifact_digest = bytes_digest(artifact_bytes);
  fs::write(
    &binding,
    format!("graph-sha256 {graph}\nartifact-sha256 {artifact_digest}\n"),
  )
  .map_err(|error| {
    format!("write js2wasm graph binding {}: {error}", binding.display())
  })
}

fn bound_artifact_digest(
  artifact: &Path,
  expected_graph: &str,
) -> Result<String, String> {
  let binding = graph_binding_path(artifact);
  let actual = fs::read_to_string(&binding).map_err(|error| {
    format!(
      "read js2wasm graph binding {} for artifact {}: {error}",
      binding.display(),
      artifact.display(),
    )
  })?;
  let (bound_graph, bound_artifact) =
    parse_graph_binding(&actual).map_err(|error| {
      format!(
        "invalid js2wasm graph binding {}: {error}",
        binding.display()
      )
    })?;
  if bound_graph != expected_graph {
    return Err(format!(
      "js2wasm graph binding mismatch for artifact {}: expected {expected_graph}, found {bound_graph}",
      artifact.display(),
    ));
  }
  Ok(bound_artifact.to_string())
}

fn verify_graph_binding(
  artifact: &Path,
  entry: &str,
  modules: &[SourceModule],
) -> Result<(), String> {
  let expected_graph = graph_digest(entry, modules);
  let bound_artifact = bound_artifact_digest(artifact, &expected_graph)?;
  let actual_artifact = artifact_digest(artifact)?;
  if bound_artifact != actual_artifact {
    return Err(format!(
      "js2wasm artifact binding mismatch for artifact {}: expected {bound_artifact}, found {actual_artifact}",
      artifact.display(),
    ));
  }
  Ok(())
}

#[cfg_attr(not(feature = "js2wasm_runtime_compile"), allow(dead_code))]
pub(crate) struct SourceModule {
  pub(crate) specifier: String,
  pub(crate) source: String,
}

struct DenoHostState {
  cwd: Vec<u16>,
  cwd_op_calls: u64,
  script: Vec<u16>,
  deno_op_print: Option<usize>,
  deno_op_sum: Option<usize>,
  deno_test_fn: Option<usize>,
  deno_test_fn_result: Vec<u16>,
  pending_sum: Option<PendingSum>,
  pending_print: Option<PendingPrint>,
  last_error: Option<DenoBridgeError>,
}

pub(crate) enum DenoScriptResult {
  Undefined,
  Json(serde_json::Value),
  Thrown { name: String, message: String },
}

struct PendingSum {
  is_array: bool,
  values: Vec<Option<f64>>,
}

struct PendingPrint {
  is_error: bool,
  code_units: Vec<Option<u16>>,
}

struct DenoBridgeError {
  kind: u32,
  message: Vec<u16>,
}

fn protocol_error(message: impl std::fmt::Display) -> wasmtime::Error {
  wasmtime::format_err!("v8x/js2wasm Deno scalar bridge: {message}")
}

fn scalar_bool(value: i32, label: &str) -> wasmtime::Result<bool> {
  match value {
    0 => Ok(false),
    1 => Ok(true),
    _ => Err(protocol_error(format!(
      "{label} must be the i32 boolean 0 or 1, received {value}"
    ))),
  }
}

fn scalar_length(value: f64, label: &str) -> wasmtime::Result<usize> {
  if !value.is_finite() || value < 0.0 || value.fract() != 0.0 {
    return Err(protocol_error(format!(
      "{label} must be a finite non-negative integer, received {value}"
    )));
  }
  if value > MAX_DENO_SCALAR_ITEMS as f64 {
    return Err(protocol_error(format!(
      "{label} {value} exceeds the {MAX_DENO_SCALAR_ITEMS}-item protocol limit"
    )));
  }
  Ok(value as usize)
}

fn scalar_index(
  value: f64,
  length: usize,
  label: &str,
) -> wasmtime::Result<usize> {
  let index = scalar_length(value, label)?;
  if index >= length {
    return Err(protocol_error(format!(
      "{label} {index} is outside transaction length {length}"
    )));
  }
  Ok(index)
}

fn complete_transaction<T>(
  values: Vec<Option<T>>,
  label: &str,
) -> wasmtime::Result<Vec<T>> {
  values
    .into_iter()
    .enumerate()
    .map(|(index, value)| {
      value.ok_or_else(|| {
        protocol_error(format!("{label} is missing item {index}"))
      })
    })
    .collect()
}

fn set_bridge_error(state: &mut DenoHostState, kind: u32, message: String) {
  // The Wasm wrapper currently distinguishes the exact serde TypeError from
  // every other callback/bridge failure. Unknown future kinds remain loud as
  // ordinary Error rather than accidentally acquiring TypeError semantics.
  let kind = if kind == 1 { 1 } else { 2 };
  state.last_error = Some(DenoBridgeError {
    kind,
    message: message.encode_utf16().collect(),
  });
}

fn deno_sum_begin(
  mut caller: Caller<'_, DenoHostState>,
  is_array: i32,
  length: f64,
) -> wasmtime::Result<()> {
  let is_array = scalar_bool(is_array, "sum kind")?;
  let length = scalar_length(length, "sum length")?;
  if !is_array && length != 1 {
    return Err(protocol_error(format!(
      "scalar sum argument must contain exactly one value, received {length}"
    )));
  }
  let state = caller.data_mut();
  if state.pending_sum.is_some() || state.pending_print.is_some() {
    return Err(protocol_error(
      "sum_begin cannot nest inside another scalar transaction",
    ));
  }
  state.last_error = None;
  state.pending_sum = Some(PendingSum {
    is_array,
    values: (0..length).map(|_| None).collect(),
  });
  Ok(())
}

fn deno_sum_value(
  mut caller: Caller<'_, DenoHostState>,
  index: f64,
  value: f64,
) -> wasmtime::Result<()> {
  let state = caller.data_mut();
  let pending = state
    .pending_sum
    .as_mut()
    .ok_or_else(|| protocol_error("sum_value called without sum_begin"))?;
  let index = scalar_index(index, pending.values.len(), "sum index")?;
  if pending.values[index].is_some() {
    return Err(protocol_error(format!(
      "sum item {index} was supplied more than once"
    )));
  }
  // Every IEEE-754 value is a valid JavaScript Number, including NaN and
  // infinities. Only the index is constrained by this scalar protocol.
  pending.values[index] = Some(value);
  Ok(())
}

fn deno_sum_end(
  mut caller: Caller<'_, DenoHostState>,
) -> wasmtime::Result<f64> {
  let (function, is_array, values) = {
    let state = caller.data_mut();
    let pending = state
      .pending_sum
      .take()
      .ok_or_else(|| protocol_error("sum_end called without sum_begin"))?;
    let values = complete_transaction(pending.values, "sum transaction")?;
    (state.deno_op_sum, pending.is_array, values)
  };

  let Some(function) = function else {
    set_bridge_error(
      caller.data_mut(),
      2,
      "Deno.core.ops.op_sum is not bound to this Wasmtime store".to_string(),
    );
    return Ok(0.0);
  };

  // Do not retain a Caller/data_mut borrow while entering the Rust callback:
  // op2 is allowed to call back through the v8x isolate and its outer context.
  match invoke_prelinked_deno_sum(function, is_array, &values) {
    Ok(value) => {
      caller.data_mut().last_error = None;
      Ok(value)
    }
    Err((kind, message)) => {
      set_bridge_error(caller.data_mut(), kind, message);
      Ok(0.0)
    }
  }
}

fn deno_error_kind(caller: Caller<'_, DenoHostState>) -> f64 {
  caller
    .data()
    .last_error
    .as_ref()
    .map(|error| error.kind as f64)
    .unwrap_or(0.0)
}

fn deno_error_utf16_length(caller: Caller<'_, DenoHostState>) -> f64 {
  caller
    .data()
    .last_error
    .as_ref()
    .map(|error| error.message.len() as f64)
    .unwrap_or(0.0)
}

fn deno_error_utf16_code_unit(
  caller: Caller<'_, DenoHostState>,
  index: f64,
) -> wasmtime::Result<f64> {
  let error = caller
    .data()
    .last_error
    .as_ref()
    .ok_or_else(|| protocol_error("error code-unit read without an error"))?;
  let index = scalar_index(index, error.message.len(), "error index")?;
  Ok(f64::from(error.message[index]))
}

fn deno_print_begin(
  mut caller: Caller<'_, DenoHostState>,
  is_error: i32,
  length: f64,
) -> wasmtime::Result<()> {
  let is_error = scalar_bool(is_error, "print error flag")?;
  let length = scalar_length(length, "print length")?;
  let state = caller.data_mut();
  if state.pending_sum.is_some() || state.pending_print.is_some() {
    return Err(protocol_error(
      "print_begin cannot nest inside another scalar transaction",
    ));
  }
  state.last_error = None;
  state.pending_print = Some(PendingPrint {
    is_error,
    code_units: (0..length).map(|_| None).collect(),
  });
  Ok(())
}

fn deno_print_code_unit(
  mut caller: Caller<'_, DenoHostState>,
  index: f64,
  code_unit: f64,
) -> wasmtime::Result<()> {
  if !code_unit.is_finite()
    || code_unit < 0.0
    || code_unit > f64::from(u16::MAX)
    || code_unit.fract() != 0.0
  {
    return Err(protocol_error(format!(
      "print code unit must be an integer in 0..=65535, received {code_unit}"
    )));
  }
  let state = caller.data_mut();
  let pending = state.pending_print.as_mut().ok_or_else(|| {
    protocol_error("print_code_unit called without print_begin")
  })?;
  let index = scalar_index(index, pending.code_units.len(), "print index")?;
  if pending.code_units[index].is_some() {
    return Err(protocol_error(format!(
      "print code unit {index} was supplied more than once"
    )));
  }
  pending.code_units[index] = Some(code_unit as u16);
  Ok(())
}

fn deno_print_end(
  mut caller: Caller<'_, DenoHostState>,
) -> wasmtime::Result<()> {
  let (function, is_error, code_units) = {
    let state = caller.data_mut();
    let pending = state
      .pending_print
      .take()
      .ok_or_else(|| protocol_error("print_end called without print_begin"))?;
    let code_units =
      complete_transaction(pending.code_units, "print transaction")?;
    (state.deno_op_print, pending.is_error, code_units)
  };

  let Some(function) = function else {
    set_bridge_error(
      caller.data_mut(),
      2,
      "Deno.core.ops.op_print is not bound to this Wasmtime store".to_string(),
    );
    return Ok(());
  };

  match invoke_prelinked_deno_print(function, &code_units, is_error) {
    Ok(()) => {
      caller.data_mut().last_error = None;
      Ok(())
    }
    Err((kind, message)) => {
      set_bridge_error(caller.data_mut(), kind, message);
      Ok(())
    }
  }
}

fn deno_test_fn_call(
  mut caller: Caller<'_, DenoHostState>,
) -> wasmtime::Result<f64> {
  let function = caller.data().deno_test_fn;
  caller.data_mut().deno_test_fn_result.clear();
  let Some(function) = function else {
    set_bridge_error(
      caller.data_mut(),
      2,
      "the Rust-owned global has no test_fn Function".to_string(),
    );
    return Ok(-1.0);
  };
  match invoke_prelinked_deno_test_fn(function) {
    Ok(None) => {
      caller.data_mut().last_error = None;
      Ok(0.0)
    }
    Ok(Some(json)) => {
      let state = caller.data_mut();
      state.last_error = None;
      state.deno_test_fn_result = json.encode_utf16().collect();
      Ok(1.0)
    }
    Err((kind, message)) => {
      set_bridge_error(caller.data_mut(), kind, message);
      Ok(-1.0)
    }
  }
}

fn deno_test_fn_result_utf16_length(caller: Caller<'_, DenoHostState>) -> f64 {
  caller.data().deno_test_fn_result.len() as f64
}

fn deno_test_fn_result_utf16_code_unit(
  caller: Caller<'_, DenoHostState>,
  index: f64,
) -> wasmtime::Result<f64> {
  let result = &caller.data().deno_test_fn_result;
  let index = scalar_index(index, result.len(), "test_fn result index")?;
  Ok(f64::from(result[index]))
}

fn invoke_prelinked_deno_sum(
  function: usize,
  is_array: bool,
  values: &[f64],
) -> Result<f64, (u32, String)> {
  crate::js2wasm::invoke_prelinked_deno_sum(function, is_array, values)
}

fn invoke_prelinked_deno_print(
  function: usize,
  code_units: &[u16],
  is_error: bool,
) -> Result<(), (u32, String)> {
  crate::js2wasm::invoke_prelinked_deno_print(function, code_units, is_error)
}

fn invoke_prelinked_deno_test_fn(
  function: usize,
) -> Result<Option<String>, (u32, String)> {
  crate::js2wasm::invoke_prelinked_deno_test_fn(function)
}

#[derive(Hash, PartialEq, Eq)]
enum ModuleCacheKey {
  #[cfg(not(feature = "js2wasm_deno_poc_replay"))]
  TrustedFile(PathBuf),
  GraphBoundFile {
    artifact: PathBuf,
    graph_sha256: String,
    artifact_sha256: String,
  },
  #[cfg(feature = "js2wasm_deno_poc_replay")]
  DenoPocReplay {
    role: &'static str,
    aot_sha256: String,
  },
  #[cfg(feature = "js2wasm_runtime_compile")]
  DevelopmentBytes(Vec<u8>),
}

#[derive(Clone)]
enum PreparedModule {
  Prelinked(InstancePre<DenoHostState>),
  RuntimeEval(Module),
}

struct SharedDenoRuntime {
  engine: Engine,
  linker: Linker<DenoHostState>,
  modules: Mutex<HashMap<ModuleCacheKey, PreparedModule>>,
  runtime_eval_provider: Mutex<Option<Module>>,
  #[cfg(feature = "js2wasm_deno_poc_replay")]
  poc_replay: DenoPocReplayArtifacts,
  module_loads: AtomicUsize,
  instantiations: AtomicUsize,
  runtime_eval_provider_loads: AtomicUsize,
  runtime_eval_instantiations: AtomicUsize,
  #[cfg(feature = "js2wasm_runtime_compile")]
  cache_hits: AtomicUsize,
  #[cfg(feature = "js2wasm_runtime_compile")]
  compilations: AtomicUsize,
}

static SHARED_DENO_RUNTIME: OnceLock<Result<SharedDenoRuntime, String>> =
  OnceLock::new();

/// Diagnostic counters exposed only to verify the experimental backend.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Js2WasmRuntimeStats {
  pub cached_modules: usize,
  pub module_loads: usize,
  pub instantiations: usize,
  pub cache_hits: usize,
  pub compilations: usize,
  pub runtime_eval_provider_loads: usize,
  pub runtime_eval_instantiations: usize,
}

impl SharedDenoRuntime {
  fn new() -> Result<Self, String> {
    // Lock and read both executable replay artifacts before constructing any
    // Wasmtime Module. This deliberately happens before the shared runtime is
    // published through OnceLock, so every replay consumer sees the same
    // verified byte buffers.
    #[cfg(feature = "js2wasm_deno_poc_replay")]
    let poc_replay = DenoPocReplayArtifacts::from_env()?;
    let mut config = Config::new();
    config
      .wasm_function_references(true)
      .wasm_gc(true)
      .wasm_tail_call(true)
      .wasm_exceptions(true);
    // js2 can emit multi-megabyte functions for closed interpreter graphs.
    // Cranelift's default speed optimizations exhaust bounded packaging hosts;
    // unoptimized code preserves Wasm semantics and remains compatible with
    // the compiler-free engine that deserializes the resulting AOT artifact.
    #[cfg(feature = "js2wasm_runtime_compile")]
    config.cranelift_opt_level(OptLevel::None);
    let engine = Engine::new(&config)
      .map_err(|error| format!("configure embedded Wasmtime: {error}"))?;
    let mut linker = Linker::new(&engine);
    linker
      .func_wrap(
        DENO_IMPORT_MODULE,
        CWD_LENGTH_IMPORT,
        |mut caller: Caller<'_, DenoHostState>| -> f64 {
          caller.data_mut().cwd_op_calls += 1;
          caller.data().cwd.len() as f64
        },
      )
      .map_err(|error| format!("bind {CWD_LENGTH_IMPORT}: {error}"))?;
    linker
      .func_wrap(
        DENO_IMPORT_MODULE,
        CWD_CODE_UNIT_IMPORT,
        |mut caller: Caller<'_, DenoHostState>, index: f64| -> f64 {
          caller.data_mut().cwd_op_calls += 1;
          let index = if index.is_finite() && index >= 0.0 {
            index as usize
          } else {
            usize::MAX
          };
          caller.data().cwd.get(index).copied().unwrap_or_default() as f64
        },
      )
      .map_err(|error| format!("bind {CWD_CODE_UNIT_IMPORT}: {error}"))?;
    linker
      .func_wrap(DENO_IMPORT_MODULE, DENO_SUM_BEGIN_IMPORT, deno_sum_begin)
      .map_err(|error| format!("bind {DENO_SUM_BEGIN_IMPORT}: {error}"))?;
    linker
      .func_wrap(DENO_IMPORT_MODULE, DENO_SUM_VALUE_IMPORT, deno_sum_value)
      .map_err(|error| format!("bind {DENO_SUM_VALUE_IMPORT}: {error}"))?;
    linker
      .func_wrap(DENO_IMPORT_MODULE, DENO_SUM_END_IMPORT, deno_sum_end)
      .map_err(|error| format!("bind {DENO_SUM_END_IMPORT}: {error}"))?;
    linker
      .func_wrap(DENO_IMPORT_MODULE, DENO_ERROR_KIND_IMPORT, deno_error_kind)
      .map_err(|error| format!("bind {DENO_ERROR_KIND_IMPORT}: {error}"))?;
    linker
      .func_wrap(
        DENO_IMPORT_MODULE,
        DENO_ERROR_LENGTH_IMPORT,
        deno_error_utf16_length,
      )
      .map_err(|error| format!("bind {DENO_ERROR_LENGTH_IMPORT}: {error}"))?;
    linker
      .func_wrap(
        DENO_IMPORT_MODULE,
        DENO_ERROR_CODE_UNIT_IMPORT,
        deno_error_utf16_code_unit,
      )
      .map_err(|error| {
        format!("bind {DENO_ERROR_CODE_UNIT_IMPORT}: {error}")
      })?;
    linker
      .func_wrap(
        DENO_IMPORT_MODULE,
        DENO_PRINT_BEGIN_IMPORT,
        deno_print_begin,
      )
      .map_err(|error| format!("bind {DENO_PRINT_BEGIN_IMPORT}: {error}"))?;
    linker
      .func_wrap(
        DENO_IMPORT_MODULE,
        DENO_PRINT_CODE_UNIT_IMPORT,
        deno_print_code_unit,
      )
      .map_err(|error| {
        format!("bind {DENO_PRINT_CODE_UNIT_IMPORT}: {error}")
      })?;
    linker
      .func_wrap(DENO_IMPORT_MODULE, DENO_PRINT_END_IMPORT, deno_print_end)
      .map_err(|error| format!("bind {DENO_PRINT_END_IMPORT}: {error}"))?;
    linker
      .func_wrap(
        DENO_IMPORT_MODULE,
        DENO_SCRIPT_LENGTH_IMPORT,
        |caller: Caller<'_, DenoHostState>| -> f64 {
          caller.data().script.len() as f64
        },
      )
      .map_err(|error| format!("bind {DENO_SCRIPT_LENGTH_IMPORT}: {error}"))?;
    linker
      .func_wrap(
        DENO_IMPORT_MODULE,
        DENO_SCRIPT_CODE_UNIT_IMPORT,
        |caller: Caller<'_, DenoHostState>, index: f64| -> f64 {
          let index = if index.is_finite() && index >= 0.0 {
            index as usize
          } else {
            usize::MAX
          };
          caller.data().script.get(index).copied().unwrap_or_default() as f64
        },
      )
      .map_err(|error| {
        format!("bind {DENO_SCRIPT_CODE_UNIT_IMPORT}: {error}")
      })?;
    linker
      .func_wrap(
        DENO_IMPORT_MODULE,
        DENO_TEST_FN_CALL_IMPORT,
        deno_test_fn_call,
      )
      .map_err(|error| format!("bind {DENO_TEST_FN_CALL_IMPORT}: {error}"))?;
    linker
      .func_wrap(
        DENO_IMPORT_MODULE,
        DENO_TEST_FN_RESULT_LENGTH_IMPORT,
        deno_test_fn_result_utf16_length,
      )
      .map_err(|error| {
        format!("bind {DENO_TEST_FN_RESULT_LENGTH_IMPORT}: {error}")
      })?;
    linker
      .func_wrap(
        DENO_IMPORT_MODULE,
        DENO_TEST_FN_RESULT_CODE_UNIT_IMPORT,
        deno_test_fn_result_utf16_code_unit,
      )
      .map_err(|error| {
        format!("bind {DENO_TEST_FN_RESULT_CODE_UNIT_IMPORT}: {error}")
      })?;
    Ok(Self {
      engine,
      linker,
      modules: Mutex::new(HashMap::new()),
      runtime_eval_provider: Mutex::new(None),
      #[cfg(feature = "js2wasm_deno_poc_replay")]
      poc_replay,
      module_loads: AtomicUsize::new(0),
      instantiations: AtomicUsize::new(0),
      runtime_eval_provider_loads: AtomicUsize::new(0),
      runtime_eval_instantiations: AtomicUsize::new(0),
      #[cfg(feature = "js2wasm_runtime_compile")]
      cache_hits: AtomicUsize::new(0),
      #[cfg(feature = "js2wasm_runtime_compile")]
      compilations: AtomicUsize::new(0),
    })
  }

  #[cfg(not(feature = "js2wasm_deno_poc_replay"))]
  fn precompiled_file(
    &self,
    artifact: &Path,
  ) -> Result<PreparedModule, String> {
    let artifact = artifact.canonicalize().map_err(|error| {
      format!(
        "resolve precompiled js2wasm artifact {}: {error}",
        artifact.display()
      )
    })?;
    let key = ModuleCacheKey::TrustedFile(artifact.clone());
    let mut modules = self
      .modules
      .lock()
      .map_err(|_| "lock shared js2wasm module cache".to_string())?;
    if let Some(module) = modules.get(&key) {
      return Ok(module.clone());
    }

    // Deployment artifacts are generated by the trusted build pipeline for
    // this exact Wasmtime version/configuration and remain immutable while
    // mapped. Never use this path for user-supplied files.
    let module = unsafe { Module::deserialize_file(&self.engine, &artifact) }
      .map_err(|error| {
      format!(
        "load trusted precompiled js2wasm artifact {}: {error:#}",
        artifact.display()
      )
    })?;
    self.trace_imports(&module);
    let prepared = self.prepare_module(&module)?;
    modules.insert(key, prepared.clone());
    self.module_loads.fetch_add(1, Ordering::Relaxed);
    Ok(prepared)
  }

  fn precompiled_graph_file(
    &self,
    artifact: &Path,
    entry: &str,
    modules: &[SourceModule],
  ) -> Result<PreparedModule, String> {
    let graph_sha256 = graph_digest(entry, modules);
    let artifact_sha256 = bound_artifact_digest(artifact, &graph_sha256)?;
    let artifact = artifact.canonicalize().map_err(|error| {
      format!(
        "resolve precompiled js2wasm artifact {}: {error}",
        artifact.display()
      )
    })?;
    let key = ModuleCacheKey::GraphBoundFile {
      artifact: artifact.clone(),
      graph_sha256,
      artifact_sha256: artifact_sha256.clone(),
    };
    let mut modules = self
      .modules
      .lock()
      .map_err(|_| "lock shared js2wasm module cache".to_string())?;
    if let Some(module) = modules.get(&key) {
      return Ok(module.clone());
    }

    let artifact_bytes = fs::read(&artifact).map_err(|error| {
      format!(
        "read precompiled js2wasm artifact {} for graph binding: {error}",
        artifact.display(),
      )
    })?;
    let actual_artifact = bytes_digest(&artifact_bytes);
    if artifact_sha256 != actual_artifact {
      return Err(format!(
        "js2wasm artifact binding mismatch for artifact {}: expected {artifact_sha256}, found {actual_artifact}",
        artifact.display(),
      ));
    }
    // Deserialize the same bytes that were just verified. Unlike the exact
    // Deno diagnostic path, this copies executable data out of the source file,
    // so a later path replacement cannot mutate the cached Module's mapping.
    let module = unsafe { Module::deserialize(&self.engine, &artifact_bytes) }
      .map_err(|error| {
        format!(
          "load trusted precompiled js2wasm artifact {}: {error:#}",
          artifact.display()
        )
      })?;
    self.trace_imports(&module);
    let prepared = self.prepare_module(&module)?;
    modules.insert(key, prepared.clone());
    self.module_loads.fetch_add(1, Ordering::Relaxed);
    Ok(prepared)
  }

  #[cfg(feature = "js2wasm_deno_poc_replay")]
  fn precompiled_deno_poc_replay(
    &self,
    role: &'static str,
    artifact: &VerifiedDenoPocArtifact,
  ) -> Result<PreparedModule, String> {
    let key = ModuleCacheKey::DenoPocReplay {
      role,
      aot_sha256: artifact.sha256.clone(),
    };
    let mut modules = self
      .modules
      .lock()
      .map_err(|_| "lock shared js2wasm module cache".to_string())?;
    if let Some(module) = modules.get(&key) {
      return Ok(module.clone());
    }

    // The bytes were read and hash-checked from the replay lock before this
    // runtime existed. Do not replace this with deserialize_file: native AOT
    // mappings would reopen a mutable path after the verifier has run.
    let module = unsafe { Module::deserialize(&self.engine, &artifact.bytes) }
      .map_err(|error| {
        format!("load verified Deno POC {role} AOT artifact: {error:#}")
      })?;
    self.trace_imports(&module);
    let prepared = self.prepare_module(&module)?;
    modules.insert(key, prepared.clone());
    self.module_loads.fetch_add(1, Ordering::Relaxed);
    Ok(prepared)
  }

  #[cfg(feature = "js2wasm_runtime_compile")]
  fn precompile(&self, wasm: &[u8]) -> Result<Vec<u8>, String> {
    self
      .engine
      .precompile_module(wasm)
      .map_err(|error| format!("precompile js2wasm output: {error:#}"))
  }

  #[cfg(feature = "js2wasm_runtime_compile")]
  fn development_bytes(
    &self,
    artifact: &[u8],
  ) -> Result<PreparedModule, String> {
    let key = ModuleCacheKey::DevelopmentBytes(artifact.to_vec());
    let mut modules = self
      .modules
      .lock()
      .map_err(|_| "lock shared js2wasm module cache".to_string())?;
    if let Some(module) = modules.get(&key) {
      return Ok(module.clone());
    }
    // These bytes were created immediately above by this process's trusted
    // development compiler using the same Engine configuration.
    let module = unsafe { Module::deserialize(&self.engine, artifact) }
      .map_err(|error| {
        format!("load development js2wasm artifact: {error:#}")
      })?;
    self.trace_imports(&module);
    let prepared = self.prepare_module(&module)?;
    modules.insert(key, prepared.clone());
    self.module_loads.fetch_add(1, Ordering::Relaxed);
    Ok(prepared)
  }

  fn trace_imports(&self, module: &Module) {
    if std::env::var_os("V8X_JS2WASM_TRACE_IMPORTS").is_some() {
      for import in module.imports() {
        eprintln!("v8x/js2wasm import {}::{}", import.module(), import.name());
      }
    }
  }

  fn prepare_module(&self, module: &Module) -> Result<PreparedModule, String> {
    let mut needs_runtime_eval = false;
    for import in module.imports() {
      let known_deno_import = import.module() == DENO_IMPORT_MODULE
        && DENO_HOST_IMPORTS.contains(&import.name());
      let runtime_eval_import = (import.module() == RUNTIME_EVAL_IMPORT_MODULE
        && RUNTIME_EVAL_IMPORTS.contains(&import.name()))
        || (import.module() == RUNTIME_EVAL_JSON_IMPORT_MODULE
          && import.name() == "__v8x_runtime_eval_json");
      needs_runtime_eval |= runtime_eval_import;
      let deferred_bootstrap_import = DEFERRED_BOOTSTRAP_IMPORTS
        .iter()
        .any(|candidate| *candidate == (import.module(), import.name()));
      if !known_deno_import
        && !runtime_eval_import
        && !deferred_bootstrap_import
      {
        return Err(format!(
          "unimplemented js2wasm host import {}::{}",
          import.module(),
          import.name(),
        ));
      }
    }

    if needs_runtime_eval {
      return Ok(PreparedModule::RuntimeEval(module.clone()));
    }

    // The exact core bootstrap retains Promise/eval imports as function
    // values, but does not invoke them. Bind only that audited allowlist to
    // Wasmtime's signature-preserving trap functions: boot can instantiate,
    // while the first real use remains an explicit failure rather than a
    // success-shaped default/no-op.
    let mut linker = self.linker.clone();
    linker
      .define_unknown_imports_as_traps(module)
      .map_err(|error| {
        format!("bind deferred js2wasm bootstrap imports: {error:#}")
      })?;
    let instance_pre = linker
      .instantiate_pre(module)
      .map_err(|error| format!("resolve js2wasm host imports: {error:#}"))?;
    Ok(PreparedModule::Prelinked(instance_pre))
  }

  fn validate_runtime_eval_provider(
    &self,
    module: &Module,
  ) -> Result<(), String> {
    let imports: Vec<_> = module
      .imports()
      .map(|import| format!("{}::{}", import.module(), import.name()))
      .collect();
    if !imports.is_empty() {
      return Err(format!(
        "js2wasm runtime-eval provider must have zero imports, found {}",
        imports.join(", "),
      ));
    }
    let exports: HashSet<_> = module
      .exports()
      .map(|export| export.name().to_string())
      .collect();
    let missing: Vec<_> = RUNTIME_EVAL_PROVIDER_EXPORTS
      .iter()
      .copied()
      .filter(|name| !exports.contains(*name))
      .collect();
    if !missing.is_empty() {
      return Err(format!(
        "js2wasm runtime-eval provider is missing exports: {}",
        missing.join(", "),
      ));
    }
    Ok(())
  }

  #[cfg(feature = "js2wasm_runtime_compile")]
  fn runtime_eval_provider_from_wasm(
    &self,
    wasm_path: &Path,
  ) -> Result<Module, String> {
    let wasm = fs::read(wasm_path).map_err(|error| {
      format!(
        "read js2wasm runtime-eval provider {}: {error}",
        wasm_path.display(),
      )
    })?;
    let source_digest = bytes_digest(&wasm);
    let mut hasher = Sha256::new();
    hasher.update(b"v8x/js2wasm runtime-eval provider cache v1\0");
    update_graph_digest(&mut hasher, source_digest.as_bytes());
    update_graph_digest(&mut hasher, env!("CARGO_PKG_VERSION").as_bytes());
    update_graph_digest(&mut hasher, b"wasmtime-47.0.3");
    update_graph_digest(&mut hasher, std::env::consts::OS.as_bytes());
    update_graph_digest(&mut hasher, std::env::consts::ARCH.as_bytes());
    let key = format!("{:x}", hasher.finalize());
    let artifact =
      runtime_cache_dir().join(format!("runtime-eval-{key}.cwasm"));

    if let Ok(bound_artifact) = bound_artifact_digest(&artifact, &source_digest)
      && let Ok(bytes) = fs::read(&artifact)
      && bytes_digest(&bytes) == bound_artifact
      && let Ok(module) = unsafe { Module::deserialize(&self.engine, &bytes) }
    {
      persist_runtime_eval_provider_artifact(&wasm, &bytes)?;
      self.cache_hits.fetch_add(1, Ordering::Relaxed);
      return Ok(module);
    }

    let bytes = self.precompile(&wasm)?;
    publish_bound_artifact(&artifact, &bytes, &source_digest)?;
    persist_runtime_eval_provider_artifact(&wasm, &bytes)?;
    unsafe { Module::deserialize(&self.engine, &bytes) }.map_err(|error| {
      format!(
        "load cached js2wasm runtime-eval provider {}: {error:#}",
        artifact.display(),
      )
    })
  }

  fn runtime_eval_provider(&self) -> Result<Module, String> {
    let mut cached = self
      .runtime_eval_provider
      .lock()
      .map_err(|_| "lock js2wasm runtime-eval provider cache".to_string())?;
    if let Some(module) = cached.as_ref() {
      return Ok(module.clone());
    }

    #[cfg(feature = "js2wasm_deno_poc_replay")]
    let module = unsafe {
      // `SharedDenoRuntime::new` has already read and verified this exact
      // payload from the manifest-selected provider path. Keep the provider
      // in memory through deserialization so replay never reopens an AOT
      // pathname after validation.
      Module::deserialize(
        &self.engine,
        &self.poc_replay.runtime_eval_provider.bytes,
      )
    }
    .map_err(|error| {
      format!(
        "load verified Deno POC runtime-eval provider AOT artifact: {error:#}"
      )
    })?;

    #[cfg(not(feature = "js2wasm_deno_poc_replay"))]
    let module = if let Some(path) =
      std::env::var_os(RUNTIME_EVAL_AOT_MODULE_ENV)
    {
      let path = PathBuf::from(path);
      // This is executable native code and must be supplied by the same
      // trusted packaging pipeline as the application artifact.
      unsafe { Module::deserialize_file(&self.engine, &path) }.map_err(
        |error| {
          format!(
            "load trusted js2wasm runtime-eval provider {}: {error:#}",
            path.display(),
          )
        },
      )?
    } else {
      #[cfg(feature = "js2wasm_runtime_compile")]
      {
        let path = std::env::var_os(RUNTIME_EVAL_WASM_ENV).ok_or_else(|| {
          format!(
            "module imports {RUNTIME_EVAL_IMPORT_MODULE}; set {RUNTIME_EVAL_AOT_MODULE_ENV} to a trusted precompiled provider or {RUNTIME_EVAL_WASM_ENV} to the provider Wasm in the runtime profile"
          )
        })?;
        self.runtime_eval_provider_from_wasm(Path::new(&path))?
      }
      #[cfg(not(feature = "js2wasm_runtime_compile"))]
      {
        return Err(format!(
          "module imports {RUNTIME_EVAL_IMPORT_MODULE}; compiler-free builds require {RUNTIME_EVAL_AOT_MODULE_ENV} to point to a trusted precompiled provider"
        ));
      }
    };
    self.validate_runtime_eval_provider(&module)?;
    self
      .runtime_eval_provider_loads
      .fetch_add(1, Ordering::Relaxed);
    *cached = Some(module.clone());
    Ok(module)
  }

  fn stats(&self) -> Result<Js2WasmRuntimeStats, String> {
    let modules = self.modules.lock().map_err(|_| {
      "lock shared js2wasm module cache for diagnostics".to_string()
    })?;
    Ok(Js2WasmRuntimeStats {
      cached_modules: modules.len(),
      module_loads: self.module_loads.load(Ordering::Relaxed),
      instantiations: self.instantiations.load(Ordering::Relaxed),
      #[cfg(feature = "js2wasm_runtime_compile")]
      cache_hits: self.cache_hits.load(Ordering::Relaxed),
      #[cfg(not(feature = "js2wasm_runtime_compile"))]
      cache_hits: 0,
      #[cfg(feature = "js2wasm_runtime_compile")]
      compilations: self.compilations.load(Ordering::Relaxed),
      #[cfg(not(feature = "js2wasm_runtime_compile"))]
      compilations: 0,
      runtime_eval_provider_loads: self
        .runtime_eval_provider_loads
        .load(Ordering::Relaxed),
      runtime_eval_instantiations: self
        .runtime_eval_instantiations
        .load(Ordering::Relaxed),
    })
  }
}

fn shared_runtime() -> Result<&'static SharedDenoRuntime, String> {
  SHARED_DENO_RUNTIME
    .get_or_init(SharedDenoRuntime::new)
    .as_ref()
    .map_err(Clone::clone)
}

/// Returns counters for structural sharing tests of the experimental backend.
#[doc(hidden)]
pub fn js2wasm_runtime_stats() -> Result<Js2WasmRuntimeStats, String> {
  shared_runtime()?.stats()
}

fn test_source_modules(modules: &[(&str, &str)]) -> Vec<SourceModule> {
  modules
    .iter()
    .map(|(specifier, source)| SourceModule {
      specifier: (*specifier).to_string(),
      source: (*source).to_string(),
    })
    .collect()
}

/// Writes a graph-binding sidecar for the supplied artifact bytes in tests.
#[doc(hidden)]
pub fn js2wasm_write_graph_binding_for_test(
  artifact: &Path,
  artifact_bytes: &[u8],
  entry: &str,
  modules: &[(&str, &str)],
) -> Result<(), String> {
  write_graph_binding(
    artifact,
    artifact_bytes,
    entry,
    &test_source_modules(modules),
  )
}

/// Verifies a graph-binding sidecar without deserializing its artifact.
#[doc(hidden)]
pub fn js2wasm_verify_graph_binding_for_test(
  artifact: &Path,
  entry: &str,
  modules: &[(&str, &str)],
) -> Result<(), String> {
  verify_graph_binding(artifact, entry, &test_source_modules(modules))
}

/// Diagnostic entry point for the pinned Deno-core bootstrap integration.
#[cfg(all(
  feature = "js2wasm_runtime_compile",
  not(feature = "js2wasm_deno_poc_replay"),
))]
#[doc(hidden)]
pub fn js2wasm_bootstrap_raw_module_for_test(
  artifact: &Path,
) -> Result<(), String> {
  let wasm = fs::read(artifact).map_err(|error| {
    format!(
      "read exact Deno core artifact {}: {error}",
      artifact.display()
    )
  })?;
  let shared = shared_runtime()?;
  let prepared = if let Some(precompiled) =
    std::env::var_os("V8X_JS2WASM_DENO_CORE_AOT_MODULE")
  {
    shared.precompiled_file(Path::new(&precompiled))?
  } else {
    let precompiled = shared.precompile(&wasm)?;
    persist_precompiled_deno_core_artifact(&wasm, &precompiled)?;
    shared.development_bytes(&precompiled)?
  };
  let cwd = std::env::current_dir()
    .map_err(|error| format!("resolve test working directory: {error}"))?;
  DenoRuntime::instantiate(shared, &prepared, cwd.clone())?;
  DenoRuntime::instantiate(shared, &prepared, cwd)?;
  Ok(())
}

/// Precompile one Deno application artifact.
///
/// The packaging runner invokes this in its own process so its compiler memory
/// is returned to the OS before the much larger runtime-eval provider is built.
/// The strict POC profile additionally writes its existing paired attestation.
#[cfg(feature = "js2wasm_runtime_compile")]
#[doc(hidden)]
pub fn js2wasm_precompile_deno_core_for_test(
  artifact: &Path,
) -> Result<(), String> {
  let wasm = fs::read(artifact).map_err(|error| {
    format!(
      "read exact Deno core artifact {}: {error}",
      artifact.display()
    )
  })?;
  let precompiled = shared_runtime()?.precompile(&wasm)?;
  persist_precompiled_deno_core_artifact(&wasm, &precompiled)
}

/// Precompile one runtime-eval provider artifact.
///
/// Keeping this out of the application validation process bounds peak memory:
/// no application module or store remains resident while Cranelift processes
/// the provider's large closed interpreter graph. The strict POC profile
/// additionally writes its existing paired attestation.
#[cfg(feature = "js2wasm_runtime_compile")]
#[doc(hidden)]
pub fn js2wasm_precompile_runtime_eval_provider_for_test(
  artifact: &Path,
) -> Result<(), String> {
  let wasm = fs::read(artifact).map_err(|error| {
    format!(
      "read exact js2wasm runtime-eval provider {}: {error}",
      artifact.display()
    )
  })?;
  let precompiled = shared_runtime()?.precompile(&wasm)?;
  persist_runtime_eval_provider_artifact(&wasm, &precompiled)
}

/// Instantiate the prelinked core-bootstrap transaction used by the
/// experimental classic-script bridge. Production uses a trusted artifact;
/// development builds may precompile the exact raw module in-process.
pub(crate) fn deno_core_bootstrap_runtime_from_env()
-> Result<DenoRuntime, String> {
  let shared = shared_runtime()?;

  #[cfg(feature = "js2wasm_deno_poc_replay")]
  let prepared =
    shared.precompiled_deno_poc_replay("app", &shared.poc_replay.app)?;

  #[cfg(not(feature = "js2wasm_deno_poc_replay"))]
  let prepared = if let Some(artifact) =
    std::env::var_os("V8X_JS2WASM_DENO_CORE_AOT_MODULE")
  {
    shared.precompiled_file(Path::new(&artifact))?
  } else {
    #[cfg(feature = "js2wasm_runtime_compile")]
    {
      let artifact = std::env::var_os("V8X_JS2WASM_DENO_CORE_WASM")
        .ok_or_else(|| {
          "set V8X_JS2WASM_DENO_CORE_AOT_MODULE to a trusted precompiled artifact (or V8X_JS2WASM_DENO_CORE_WASM in a development build)".to_string()
        })?;
      let wasm = fs::read(&artifact).map_err(|error| {
        format!(
          "read exact Deno core artifact {}: {error}",
          Path::new(&artifact).display()
        )
      })?;
      let precompiled = shared.precompile(&wasm)?;
      persist_precompiled_deno_core_artifact(&wasm, &precompiled)?;
      shared.development_bytes(&precompiled)?
    }
    #[cfg(not(feature = "js2wasm_runtime_compile"))]
    {
      return Err(
        "compiler-free engine_js2wasm builds require V8X_JS2WASM_DENO_CORE_AOT_MODULE to point to a trusted Wasmtime-precompiled artifact"
          .to_string(),
      );
    }
  };
  let cwd = std::env::current_dir().map_err(|error| {
    format!("resolve Deno bootstrap working directory: {error}")
  })?;
  DenoRuntime::instantiate(shared, &prepared, cwd)
}

#[cfg(feature = "js2wasm_runtime_compile")]
fn persist_precompiled_deno_core_artifact(
  raw_wasm: &[u8],
  artifact: &[u8],
) -> Result<(), String> {
  #[cfg(feature = "js2wasm_deno_poc")]
  {
    return persist_deno_poc_precompile_pair(
      DENO_CORE_AOT_OUTPUT_ENV,
      DENO_CORE_AOT_ATTESTATION_ENV,
      "app",
      raw_wasm,
      artifact,
    );
  }
  #[cfg(not(feature = "js2wasm_deno_poc"))]
  let _ = raw_wasm;
  #[cfg(not(feature = "js2wasm_deno_poc"))]
  {
    let Some(output) = std::env::var_os(DENO_CORE_AOT_OUTPUT_ENV) else {
      return Ok(());
    };
    atomic_write(Path::new(&output), artifact)
  }
}

/// One private Wasmtime store and instance owned by a v8x module handle.
pub(crate) struct DenoRuntime {
  store: Store<DenoHostState>,
  instance: Instance,
  _runtime_eval_provider: Option<Instance>,
}

impl DenoRuntime {
  fn instantiate(
    shared: &SharedDenoRuntime,
    prepared: &PreparedModule,
    cwd: PathBuf,
  ) -> Result<Self, String> {
    let cwd = cwd.to_string_lossy().encode_utf16().collect();
    let mut store = Store::new(
      &shared.engine,
      DenoHostState {
        cwd,
        cwd_op_calls: 0,
        script: Vec::new(),
        deno_op_print: None,
        deno_op_sum: None,
        deno_test_fn: None,
        deno_test_fn_result: Vec::new(),
        pending_sum: None,
        pending_print: None,
        last_error: None,
      },
    );
    let (instance, runtime_eval_provider) = match prepared {
      PreparedModule::Prelinked(instance_pre) => {
        let instance = instance_pre
          .instantiate(&mut store)
          .map_err(|error| format!("instantiate js2wasm artifact: {error}"))?;
        (instance, None)
      }
      PreparedModule::RuntimeEval(module) => {
        let provider_module = shared.runtime_eval_provider()?;
        let provider = shared
          .linker
          .instantiate(&mut store, &provider_module)
          .map_err(|error| {
            format!("instantiate js2wasm runtime-eval provider: {error:#}")
          })?;
        let mut linker = shared.linker.clone();
        linker.allow_shadowing(true);
        linker
          .define_unknown_imports_as_traps(module)
          .map_err(|error| {
            format!("bind deferred js2wasm imports: {error:#}")
          })?;
        linker
          .instance(&mut store, RUNTIME_EVAL_IMPORT_MODULE, provider)
          .map_err(|error| {
            format!("bind js2wasm runtime-eval provider exports: {error:#}")
          })?;
        linker
          .instance(&mut store, RUNTIME_EVAL_JSON_IMPORT_MODULE, provider)
          .map_err(|error| {
            format!("bind js2wasm runtime-eval JSON export: {error:#}")
          })?;
        let instance =
          linker.instantiate(&mut store, module).map_err(|error| {
            format!("instantiate js2wasm artifact: {error:#}")
          })?;
        shared
          .runtime_eval_instantiations
          .fetch_add(1, Ordering::Relaxed);
        (instance, Some(provider))
      }
    };
    shared.instantiations.fetch_add(1, Ordering::Relaxed);
    let mut runtime = Self {
      store,
      instance,
      _runtime_eval_provider: runtime_eval_provider,
    };
    runtime.run_deferred_module_init()?;
    runtime.verify_cwd_probe()?;
    runtime.verify_deno_core_bootstrap_probe()?;
    runtime.verify_runtime_eval_state_probe()?;
    Ok(runtime)
  }

  pub(crate) fn bind_deno_ops(
    &mut self,
    print: Option<usize>,
    sum: Option<usize>,
  ) -> Result<(), String> {
    if print == Some(0) || sum == Some(0) {
      return Err(
        "cannot bind an empty Deno op Function handle to Wasmtime".to_string(),
      );
    }
    let state = self.store.data_mut();
    match (state.deno_op_print, state.deno_op_sum) {
      (None, None) => {
        state.deno_op_print = print;
        state.deno_op_sum = sum;
        Ok(())
      }
      (previous_print, previous_sum)
        if previous_print == print && previous_sum == sum =>
      {
        Ok(())
      }
      _ => Err(
        "Deno op Function handles were rebound to different values".to_string(),
      ),
    }
  }

  pub(crate) fn bind_test_fn(
    &mut self,
    function: Option<usize>,
  ) -> Result<(), String> {
    if function == Some(0) {
      return Err("cannot bind an empty test_fn Function handle".to_string());
    }
    self.store.data_mut().deno_test_fn = function;
    Ok(())
  }

  pub(crate) fn run_classic_script(
    &mut self,
    source: &str,
  ) -> Result<DenoScriptResult, String> {
    self.store.data_mut().script = source.encode_utf16().collect();
    let status = self
      .require_function(DENO_RUN_CLASSIC_SCRIPT)?
      .typed::<(), f64>(&self.store)
      .map_err(|error| format!("type {DENO_RUN_CLASSIC_SCRIPT}: {error}"))?
      .call(&mut self.store, ())
      .map_err(|error| format!("call {DENO_RUN_CLASSIC_SCRIPT}: {error:#}"))?;
    self.store.data_mut().script.clear();
    self.decode_classic_script_status(status)
  }

  #[cfg(not(feature = "js2wasm_deno_poc_replay"))]
  pub(crate) fn has_runtime_usage_stage(&mut self) -> Result<bool, String> {
    let Some(probe) = self
      .instance
      .get_func(&mut self.store, DENO_CORE_RUNTIME_USAGE_STAGE_PROBE)
    else {
      return Ok(false);
    };
    let value = probe
      .typed::<(), f64>(&self.store)
      .map_err(|error| {
        format!("type {DENO_CORE_RUNTIME_USAGE_STAGE_PROBE}: {error}")
      })?
      .call(&mut self.store, ())
      .map_err(|error| {
        format!("call {DENO_CORE_RUNTIME_USAGE_STAGE_PROBE}: {error:#}")
      })?;
    if value != 45.0 {
      return Err(format!(
        "Deno runtime usage-stage probe returned {value}, expected 45"
      ));
    }
    Ok(true)
  }

  #[cfg(not(feature = "js2wasm_deno_poc_replay"))]
  pub(crate) fn run_deno_core_usage(
    &mut self,
    source: &str,
  ) -> Result<DenoScriptResult, String> {
    self.store.data_mut().script = source.encode_utf16().collect();
    let status = self
      .require_function(DENO_CORE_USAGE_STAGE)?
      .typed::<(), f64>(&self.store)
      .map_err(|error| format!("type {DENO_CORE_USAGE_STAGE}: {error}"))?
      .call(&mut self.store, ())
      .map_err(|error| format!("call {DENO_CORE_USAGE_STAGE}: {error:#}"))?;
    self.store.data_mut().script.clear();
    let state = self
      .call_optional_number_export(DENO_CORE_STAGE_STATE_PROBE)?
      .ok_or_else(|| {
        format!(
          "artifact exports {DENO_CORE_USAGE_STAGE} without {DENO_CORE_STAGE_STATE_PROBE}"
        )
      })?;
    if state != 3.0 {
      return Err(format!(
        "Deno core usage stage left state {state}, expected 3"
      ));
    }
    self.decode_classic_script_status(status)
  }

  fn decode_classic_script_status(
    &mut self,
    status: f64,
  ) -> Result<DenoScriptResult, String> {
    if status == 0.0 {
      return Ok(DenoScriptResult::Undefined);
    }
    let length = self
      .require_function(DENO_SCRIPT_RESULT_LENGTH)?
      .typed::<(), f64>(&self.store)
      .map_err(|error| format!("type {DENO_SCRIPT_RESULT_LENGTH}: {error}"))?
      .call(&mut self.store, ())
      .map_err(|error| {
        format!("call {DENO_SCRIPT_RESULT_LENGTH}: {error:#}")
      })?;
    if !length.is_finite() || length < 0.0 || length.fract() != 0.0 {
      return Err(format!("classic-script result length is invalid: {length}"));
    }
    let code_unit = self
      .require_function(DENO_SCRIPT_RESULT_CODE_UNIT)?
      .typed::<f64, f64>(&self.store)
      .map_err(|error| {
        format!("type {DENO_SCRIPT_RESULT_CODE_UNIT}: {error}")
      })?;
    let mut units = Vec::with_capacity(length as usize);
    for index in 0..length as usize {
      let value =
        code_unit
          .call(&mut self.store, index as f64)
          .map_err(|error| {
            format!("call {DENO_SCRIPT_RESULT_CODE_UNIT}: {error:#}")
          })?;
      if !value.is_finite() || !(0.0..=65535.0).contains(&value) {
        return Err(format!(
          "classic-script result code unit {index} is invalid: {value}"
        ));
      }
      units.push(value as u16);
    }
    let encoded = String::from_utf16(&units)
      .map_err(|error| format!("decode classic-script result: {error}"))?;
    let value: serde_json::Value = serde_json::from_str(&encoded)
      .map_err(|error| format!("decode classic-script JSON: {error}"))?;
    if status == 1.0 {
      return Ok(DenoScriptResult::Json(value));
    }
    if status == -1.0 {
      let name = value
        .get("name")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("Error")
        .to_string();
      let message = value
        .get("message")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_string();
      return Ok(DenoScriptResult::Thrown { name, message });
    }
    Err(format!("classic-script evaluator returned status {status}"))
  }

  fn run_deferred_module_init(&mut self) -> Result<(), String> {
    let Some(init) = self.instance.get_func(&mut self.store, "__module_init")
    else {
      return Ok(());
    };
    init
      .typed::<(), ()>(&self.store)
      .map_err(|error| format!("type __module_init: {error}"))?
      .call(&mut self.store, ())
      .map_err(|error| format!("run __module_init: {error:#}"))
  }

  fn verify_cwd_probe(&mut self) -> Result<(), String> {
    let Some(length) =
      self.instance.get_func(&mut self.store, CWD_LENGTH_PROBE)
    else {
      return Ok(());
    };
    let checksum = self
      .instance
      .get_func(&mut self.store, CWD_CHECKSUM_PROBE)
      .ok_or_else(|| {
        format!(
          "artifact exports {CWD_LENGTH_PROBE} without {CWD_CHECKSUM_PROBE}"
        )
      })?;
    let length = length
      .typed::<(), f64>(&self.store)
      .map_err(|error| format!("type {CWD_LENGTH_PROBE}: {error}"))?
      .call(&mut self.store, ())
      .map_err(|error| format!("call {CWD_LENGTH_PROBE}: {error}"))?;
    let checksum = checksum
      .typed::<(), f64>(&self.store)
      .map_err(|error| format!("type {CWD_CHECKSUM_PROBE}: {error}"))?
      .call(&mut self.store, ())
      .map_err(|error| format!("call {CWD_CHECKSUM_PROBE}: {error}"))?;
    let expected_length = self.store.data().cwd.len() as f64;
    let expected_checksum = self
      .store
      .data()
      .cwd
      .iter()
      .enumerate()
      .map(|(index, unit)| (index as f64 + 1.0) * f64::from(*unit))
      .sum::<f64>();
    if length != expected_length || checksum != expected_checksum {
      return Err(format!(
        "Deno.cwd() bridge returned length/checksum {length}/{checksum}, expected {expected_length}/{expected_checksum} ({} host calls)",
        self.store.data().cwd_op_calls,
      ));
    }
    let expected_calls = 2 * (self.store.data().cwd.len() as u64 + 1);
    if self.store.data().cwd_op_calls != expected_calls {
      return Err(format!(
        "Deno.cwd() probe made {} typed host calls, expected {expected_calls} for a fresh instance",
        self.store.data().cwd_op_calls,
      ));
    }
    Ok(())
  }

  fn verify_deno_core_bootstrap_probe(&mut self) -> Result<(), String> {
    let probe = self
      .instance
      .get_func(&mut self.store, DENO_CORE_BOOTSTRAP_PROBE);
    #[cfg(feature = "js2wasm_deno_poc_replay")]
    let probe = probe.ok_or_else(|| {
      format!(
        "closed-world Deno replay artifact has no required {DENO_CORE_BOOTSTRAP_PROBE} export"
      )
    })?;
    #[cfg(not(feature = "js2wasm_deno_poc_replay"))]
    let Some(probe) = probe else {
      return Ok(());
    };
    let value = probe
      .typed::<(), f64>(&self.store)
      .map_err(|error| format!("type {DENO_CORE_BOOTSTRAP_PROBE}: {error}"))?
      .call(&mut self.store, ())
      .map_err(|error| {
        format!("call {DENO_CORE_BOOTSTRAP_PROBE}: {error:#}")
      })?;
    if value != 42.0 {
      return Err(format!(
        "Deno core bootstrap probe returned {value}, expected 42"
      ));
    }
    Ok(())
  }

  fn verify_runtime_eval_state_probe(&mut self) -> Result<(), String> {
    if std::env::var_os(RUNTIME_EVAL_STATE_PROBE_ENV).is_none() {
      return Ok(());
    }
    let Some(probe) = self
      .instance
      .get_func(&mut self.store, RUNTIME_EVAL_STATE_PROBE)
    else {
      return Ok(());
    };
    let value = probe
      .typed::<(), f64>(&self.store)
      .map_err(|error| format!("type {RUNTIME_EVAL_STATE_PROBE}: {error}"))?
      .call(&mut self.store, ())
      .map_err(|error| format!("call {RUNTIME_EVAL_STATE_PROBE}: {error:#}"))?;
    if value != 84.0 {
      return Err(format!(
        "runtime-eval shared-state probe returned {value}, expected 84"
      ));
    }
    Ok(())
  }

  #[cfg(not(feature = "js2wasm_deno_poc_replay"))]
  fn call_optional_number_export(
    &mut self,
    name: &str,
  ) -> Result<Option<f64>, String> {
    let Some(function) = self.instance.get_func(&mut self.store, name) else {
      return Ok(None);
    };
    let value = function
      .typed::<(), f64>(&self.store)
      .map_err(|error| format!("type {name}: {error}"))?
      .call(&mut self.store, ())
      .map_err(|error| format!("call {name}: {error:#}"))?;
    Ok(Some(value))
  }

  fn require_function(&mut self, name: &str) -> Result<wasmtime::Func, String> {
    self
      .instance
      .get_func(&mut self.store, name)
      .ok_or_else(|| format!("prelinked Deno artifact has no {name} export"))
  }

  pub(crate) fn set_deno_tick_info(
    &mut self,
    values: [u8; 2],
  ) -> Result<(), String> {
    let function = self.require_function(DENO_CORE_SET_TICK_INFO)?;
    let result = function
      .typed::<(f64, f64), f64>(&self.store)
      .map_err(|error| format!("type {DENO_CORE_SET_TICK_INFO}: {error}"))?
      .call(
        &mut self.store,
        (f64::from(values[0]), f64::from(values[1])),
      )
      .map_err(|error| format!("call {DENO_CORE_SET_TICK_INFO}: {error:#}"))?;
    if result != 52.0 {
      return Err(format!(
        "Deno tick-info setter returned {result}, expected 52"
      ));
    }
    Ok(())
  }

  pub(crate) fn set_deno_immediate_info(
    &mut self,
    values: [u32; 3],
  ) -> Result<(), String> {
    let function = self.require_function(DENO_CORE_SET_IMMEDIATE_INFO)?;
    let result = function
      .typed::<(f64, f64, f64), f64>(&self.store)
      .map_err(|error| format!("type {DENO_CORE_SET_IMMEDIATE_INFO}: {error}"))?
      .call(
        &mut self.store,
        (
          f64::from(values[0]),
          f64::from(values[1]),
          f64::from(values[2]),
        ),
      )
      .map_err(|error| {
        format!("call {DENO_CORE_SET_IMMEDIATE_INFO}: {error:#}")
      })?;
    if result != 53.0 {
      return Err(format!(
        "Deno immediate-info setter returned {result}, expected 53"
      ));
    }
    Ok(())
  }

  pub(crate) fn set_deno_timer_info(
    &mut self,
    value: i32,
  ) -> Result<(), String> {
    let function = self.require_function(DENO_CORE_SET_TIMER_INFO)?;
    let result = function
      .typed::<f64, f64>(&self.store)
      .map_err(|error| format!("type {DENO_CORE_SET_TIMER_INFO}: {error}"))?
      .call(&mut self.store, f64::from(value))
      .map_err(|error| format!("call {DENO_CORE_SET_TIMER_INFO}: {error:#}"))?;
    if result != 51.0 {
      return Err(format!(
        "Deno timer-info setter returned {result}, expected 51"
      ));
    }
    Ok(())
  }

  #[cfg(not(feature = "js2wasm_deno_poc_replay"))]
  fn advance_deno_core_stage(
    &mut self,
    export: &str,
    expected_value: f64,
    expected_state: f64,
  ) -> Result<bool, String> {
    let Some(value) = self.call_optional_number_export(export)? else {
      return Ok(false);
    };
    if value != expected_value {
      return Err(format!(
        "Deno core stage {export} returned {value}, expected {expected_value}"
      ));
    }
    let state = self
      .call_optional_number_export(DENO_CORE_STAGE_STATE_PROBE)?
      .ok_or_else(|| {
        format!(
          "artifact exports {export} without {DENO_CORE_STAGE_STATE_PROBE}"
        )
      })?;
    if state != expected_state {
      return Err(format!(
        "Deno core stage {export} left state {state}, expected {expected_state}"
      ));
    }
    Ok(true)
  }

  #[cfg(not(feature = "js2wasm_deno_poc_replay"))]
  pub(crate) fn advance_deno_core_wrappers(&mut self) -> Result<bool, String> {
    self.advance_deno_core_stage(DENO_CORE_WRAPPERS_STAGE, 42.0, 1.0)
  }

  #[cfg(not(feature = "js2wasm_deno_poc_replay"))]
  pub(crate) fn advance_deno_core_module(&mut self) -> Result<(), String> {
    if self.advance_deno_core_stage(DENO_CORE_MODULE_STAGE, 43.0, 2.0)? {
      Ok(())
    } else {
      Err(format!(
        "prelinked Deno artifact has no {DENO_CORE_MODULE_STAGE} export"
      ))
    }
  }

  #[allow(dead_code)]
  #[cfg(not(feature = "js2wasm_deno_poc_replay"))]
  pub(crate) fn advance_deno_core_usage(&mut self) -> Result<(), String> {
    if self.advance_deno_core_stage(DENO_CORE_USAGE_STAGE, 44.0, 3.0)? {
      Ok(())
    } else {
      Err(format!(
        "prelinked Deno artifact has no {DENO_CORE_USAGE_STAGE} export"
      ))
    }
  }
}

pub(crate) fn compile_and_instantiate(
  entry: &str,
  modules: &[SourceModule],
) -> Result<DenoRuntime, String> {
  if modules.is_empty() {
    return Err("js2wasm module graph is empty".to_string());
  }
  let shared = shared_runtime()?;
  let prepared = if let Some(artifact) =
    std::env::var_os("V8X_JS2WASM_AOT_MODULE")
  {
    let artifact = Path::new(&artifact);
    shared.precompiled_graph_file(artifact, entry, modules)?
  } else {
    #[cfg(feature = "js2wasm_runtime_compile")]
    {
      runtime_compiled_graph(shared, entry, modules)?
    }
    #[cfg(not(feature = "js2wasm_runtime_compile"))]
    {
      let _ = (entry, modules);
      return Err(
          "compiler-free engine_js2wasm builds require V8X_JS2WASM_AOT_MODULE to point to a trusted Wasmtime-precompiled artifact"
            .to_string(),
        );
    }
  };
  let cwd = std::env::current_dir()
    .map_err(|error| format!("resolve Deno.cwd() host value: {error}"))?;
  DenoRuntime::instantiate(shared, &prepared, cwd)
}

#[cfg(feature = "js2wasm_runtime_compile")]
fn runtime_cache_dir() -> PathBuf {
  if let Some(path) = std::env::var_os(RUNTIME_CACHE_DIR_ENV) {
    return PathBuf::from(path);
  }
  #[cfg(target_os = "windows")]
  if let Some(path) = std::env::var_os("LOCALAPPDATA") {
    return PathBuf::from(path).join("v8x").join("js2wasm");
  }
  #[cfg(target_os = "macos")]
  if let Some(path) = std::env::var_os("HOME") {
    return PathBuf::from(path)
      .join("Library")
      .join("Caches")
      .join("v8x")
      .join("js2wasm");
  }
  if let Some(path) = std::env::var_os("XDG_CACHE_HOME") {
    return PathBuf::from(path).join("v8x").join("js2wasm");
  }
  if let Some(path) = std::env::var_os("HOME") {
    return PathBuf::from(path)
      .join(".cache")
      .join("v8x")
      .join("js2wasm");
  }
  std::env::temp_dir().join("v8x-js2wasm-cache")
}

#[cfg(feature = "js2wasm_runtime_compile")]
fn hash_file_if_present(
  hasher: &mut Sha256,
  path: &Path,
) -> Result<(), String> {
  match fs::read(path) {
    Ok(bytes) => {
      update_graph_digest(hasher, path.to_string_lossy().as_bytes());
      update_graph_digest(hasher, &bytes);
      Ok(())
    }
    Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
    Err(error) => Err(format!(
      "read js2wasm compiler identity input {}: {error}",
      path.display()
    )),
  }
}

#[cfg(feature = "js2wasm_runtime_compile")]
fn runtime_compiler_identity() -> Result<String, String> {
  if let Some(identity) = std::env::var_os(RUNTIME_COMPILER_ID_ENV) {
    return Ok(identity.to_string_lossy().into_owned());
  }

  let configured_compiler = std::env::var_os("V8X_JS2WASM_COMPILER");
  let compiler = configured_compiler.clone().unwrap_or_else(|| "node".into());
  let script = std::env::var_os("V8X_JS2WASM_COMPILER_SCRIPT");
  if configured_compiler.is_none() && script.is_none() {
    return Err(
      "set V8X_JS2WASM_COMPILER to a standalone graph compiler or V8X_JS2WASM_COMPILER_SCRIPT to compile-graph.ts (or set V8X_JS2WASM_AOT_MODULE)"
        .to_string(),
    );
  }
  let mut hasher = Sha256::new();
  update_graph_digest(&mut hasher, compiler.to_string_lossy().as_bytes());
  if let Some(script) = script {
    hash_file_if_present(&mut hasher, Path::new(&script))?;
  }

  let version = Command::new(&compiler).arg("--version").output();
  if let Ok(version) = version {
    update_graph_digest(&mut hasher, &version.stdout);
    update_graph_digest(&mut hasher, &version.stderr);
  }
  if let Some(workdir) = std::env::var_os("V8X_JS2WASM_WORKDIR") {
    let workdir = PathBuf::from(workdir);
    for name in ["package.json", "pnpm-lock.yaml", "package-lock.json"] {
      hash_file_if_present(&mut hasher, &workdir.join(name))?;
    }
  }
  Ok(format!("{:x}", hasher.finalize()))
}

#[cfg(feature = "js2wasm_runtime_compile")]
fn runtime_cache_key(
  entry: &str,
  modules: &[SourceModule],
) -> Result<String, String> {
  let mut hasher = Sha256::new();
  hasher.update(RUNTIME_CACHE_DOMAIN);
  update_graph_digest(&mut hasher, graph_digest(entry, modules).as_bytes());
  update_graph_digest(&mut hasher, runtime_compiler_identity()?.as_bytes());
  update_graph_digest(&mut hasher, env!("CARGO_PKG_VERSION").as_bytes());
  update_graph_digest(&mut hasher, b"wasmtime-47.0.3");
  update_graph_digest(&mut hasher, std::env::consts::OS.as_bytes());
  update_graph_digest(&mut hasher, std::env::consts::ARCH.as_bytes());
  Ok(format!("{:x}", hasher.finalize()))
}

#[cfg(feature = "js2wasm_runtime_compile")]
fn atomic_write(path: &Path, contents: &[u8]) -> Result<(), String> {
  let parent = path.parent().ok_or_else(|| {
    format!("js2wasm cache path {} has no parent", path.display())
  })?;
  fs::create_dir_all(parent).map_err(|error| {
    format!(
      "create js2wasm cache directory {}: {error}",
      parent.display()
    )
  })?;
  let id = NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed);
  let temporary = parent.join(format!(
    ".{}.{}.{id}.tmp",
    path.file_name().unwrap_or_default().to_string_lossy(),
    std::process::id(),
  ));
  fs::write(&temporary, contents).map_err(|error| {
    format!(
      "write temporary js2wasm cache file {}: {error}",
      temporary.display()
    )
  })?;
  match fs::rename(&temporary, path) {
    Ok(()) => Ok(()),
    // Windows does not replace an existing destination. Concurrent writers
    // for one content address are harmless when they produced identical data.
    Err(error)
      if error.kind() == std::io::ErrorKind::AlreadyExists
        && fs::read(path).is_ok_and(|existing| existing == contents) =>
    {
      let _ = fs::remove_file(&temporary);
      Ok(())
    }
    Err(error) => {
      let _ = fs::remove_file(&temporary);
      Err(format!(
        "publish js2wasm cache file {} as {}: {error}",
        temporary.display(),
        path.display(),
      ))
    }
  }
}

#[cfg(feature = "js2wasm_deno_poc")]
fn persist_deno_poc_precompile_pair(
  artifact_output_env: &str,
  attestation_output_env: &str,
  role: &str,
  raw_wasm: &[u8],
  artifact: &[u8],
) -> Result<(), String> {
  let artifact_output = std::env::var_os(artifact_output_env);
  let attestation_output = std::env::var_os(attestation_output_env);
  let (Some(artifact_output), Some(attestation_output)) =
    (artifact_output, attestation_output)
  else {
    if std::env::var_os(artifact_output_env).is_none()
      && std::env::var_os(attestation_output_env).is_none()
    {
      return Ok(());
    }
    return Err(format!(
      "trusted Deno POC precompile requires {artifact_output_env} and \
       {attestation_output_env} to be set together"
    ));
  };
  let artifact_output = Path::new(&artifact_output);
  let attestation_output = Path::new(&attestation_output);
  if artifact_output == attestation_output {
    return Err(format!(
      "trusted Deno POC artifact and attestation paths must differ: {}",
      artifact_output.display(),
    ));
  }

  let attestation = serde_json::json!({
    "schema_version": 1,
    "role": role,
    "wasmtime_version": "47.0.3",
    "target": {
      "os": std::env::consts::OS,
      "arch": std::env::consts::ARCH,
      "triple": env!("V8X_BUILD_TARGET_TRIPLE"),
    },
    "engine_config": {
      "wasm_function_references": true,
      "wasm_gc": true,
      "wasm_tail_call": true,
      "wasm_exceptions": true,
    },
    "raw_sha256": bytes_digest(raw_wasm),
    "aot_sha256": bytes_digest(artifact),
  });
  let mut attestation_bytes =
    serde_json::to_vec_pretty(&attestation).map_err(|error| {
      format!("serialize Deno POC precompile attestation: {error}")
    })?;
  attestation_bytes.push(b'\n');

  // Publish the native bytes first and the binding record second. A crash can
  // leave an untrusted artifact without an attestation, but never a completed
  // attestation that names bytes which have not yet been written.
  atomic_write(artifact_output, artifact)?;
  atomic_write(attestation_output, &attestation_bytes)
}

#[cfg(feature = "js2wasm_runtime_compile")]
fn publish_graph_artifact(
  artifact: &Path,
  bytes: &[u8],
  entry: &str,
  modules: &[SourceModule],
) -> Result<(), String> {
  publish_bound_artifact(artifact, bytes, &graph_digest(entry, modules))
}

#[cfg(feature = "js2wasm_runtime_compile")]
fn publish_bound_artifact(
  artifact: &Path,
  bytes: &[u8],
  source_digest: &str,
) -> Result<(), String> {
  atomic_write(artifact, bytes)?;
  let binding = format!(
    "graph-sha256 {source_digest}\nartifact-sha256 {}\n",
    bytes_digest(bytes),
  );
  atomic_write(&graph_binding_path(artifact), binding.as_bytes())
}

#[cfg(feature = "js2wasm_runtime_compile")]
fn persist_runtime_eval_provider_artifact(
  raw_wasm: &[u8],
  bytes: &[u8],
) -> Result<(), String> {
  #[cfg(feature = "js2wasm_deno_poc")]
  {
    return persist_deno_poc_precompile_pair(
      RUNTIME_EVAL_AOT_OUTPUT_ENV,
      RUNTIME_EVAL_AOT_ATTESTATION_ENV,
      "runtime_eval_provider",
      raw_wasm,
      bytes,
    );
  }
  #[cfg(not(feature = "js2wasm_deno_poc"))]
  let _ = raw_wasm;
  #[cfg(not(feature = "js2wasm_deno_poc"))]
  {
    let Some(output) = std::env::var_os(RUNTIME_EVAL_AOT_OUTPUT_ENV) else {
      return Ok(());
    };
    atomic_write(Path::new(&output), bytes)
  }
}

#[cfg(feature = "js2wasm_runtime_compile")]
fn runtime_compiled_graph(
  shared: &SharedDenoRuntime,
  entry: &str,
  modules: &[SourceModule],
) -> Result<PreparedModule, String> {
  let cache_key = runtime_cache_key(entry, modules)?;
  let artifact = runtime_cache_dir().join(format!("{cache_key}.cwasm"));

  if verify_graph_binding(&artifact, entry, modules).is_ok() {
    match shared.precompiled_graph_file(&artifact, entry, modules) {
      Ok(instance) => {
        shared.cache_hits.fetch_add(1, Ordering::Relaxed);
        return Ok(instance);
      }
      Err(error) => {
        eprintln!(
          "v8x/js2wasm: ignoring invalid runtime cache entry {}: {error}",
          artifact.display(),
        );
      }
    }
  }

  let wasm = compile_graph(entry, modules)?;
  shared.compilations.fetch_add(1, Ordering::Relaxed);
  let bytes = shared.precompile(&wasm)?;
  publish_graph_artifact(&artifact, &bytes, entry, modules)?;

  if let Some(output) = std::env::var_os("V8X_JS2WASM_ARTIFACT_OUTPUT") {
    publish_graph_artifact(Path::new(&output), &bytes, entry, modules)?;
  }
  shared.precompiled_graph_file(&artifact, entry, modules)
}

#[cfg(feature = "js2wasm_runtime_compile")]
fn compile_graph(
  entry: &str,
  modules: &[SourceModule],
) -> Result<Vec<u8>, String> {
  let temp = TempDir::new()?;
  let manifest_path = temp.path.join("modules.tsv");
  let wasm_path = temp.path.join("module.wasm");
  let mut manifest = fs::File::create(&manifest_path)
    .map_err(|error| format!("create js2wasm manifest: {error}"))?;

  let mut seen = HashSet::new();
  for (index, module) in modules.iter().enumerate() {
    if module.specifier.contains(['\t', '\n', '\r']) {
      return Err(format!(
        "js2wasm spike cannot encode module specifier {:?}",
        module.specifier
      ));
    }
    if !seen.insert(module.specifier.as_str()) {
      continue;
    }
    let source_path = temp.path.join(format!("source-{index}.ts"));
    fs::write(&source_path, &module.source)
      .map_err(|error| format!("write js2wasm source: {error}"))?;
    writeln!(manifest, "{}\t{}", module.specifier, source_path.display())
      .map_err(|error| format!("write js2wasm manifest: {error}"))?;
  }
  drop(manifest);

  let configured_compiler = std::env::var_os("V8X_JS2WASM_COMPILER");
  let compiler = configured_compiler.clone().unwrap_or_else(|| "node".into());
  let script = std::env::var_os("V8X_JS2WASM_COMPILER_SCRIPT");
  if configured_compiler.is_none() && script.is_none() {
    return Err(
      "set V8X_JS2WASM_COMPILER to a standalone graph compiler or V8X_JS2WASM_COMPILER_SCRIPT to compile-graph.ts (or set V8X_JS2WASM_AOT_MODULE)"
        .to_string(),
    );
  }
  let mut compile = Command::new(&compiler);
  if let Some(script) = script {
    if Path::new(&compiler)
      .file_name()
      .is_some_and(|name| name == "node")
    {
      compile.args(["--import", "tsx"]);
    }
    compile.arg(script);
  }
  compile
    .arg("--manifest")
    .arg(&manifest_path)
    .arg("--entry")
    .arg(entry)
    .arg("--output")
    .arg(&wasm_path);
  if let Some(workdir) = std::env::var_os("V8X_JS2WASM_WORKDIR") {
    compile.current_dir(workdir);
  }
  run(compile, "js2wasm compilation")?;
  fs::read(&wasm_path)
    .map_err(|error| format!("read compiled js2wasm artifact: {error}"))
}

#[cfg(feature = "js2wasm_runtime_compile")]
fn run(mut command: Command, phase: &str) -> Result<(), String> {
  let output = command
    .output()
    .map_err(|error| format!("start {phase}: {error}"))?;
  if output.status.success() {
    return Ok(());
  }

  let stderr = String::from_utf8_lossy(&output.stderr);
  let stdout = String::from_utf8_lossy(&output.stdout);
  let detail = if !stderr.trim().is_empty() {
    stderr.trim()
  } else if !stdout.trim().is_empty() {
    stdout.trim()
  } else {
    "process exited without output"
  };
  Err(format!("{phase} failed: {detail}"))
}

#[cfg(feature = "js2wasm_runtime_compile")]
struct TempDir {
  path: PathBuf,
}

#[cfg(feature = "js2wasm_runtime_compile")]
impl TempDir {
  fn new() -> Result<Self, String> {
    let timestamp = SystemTime::now()
      .duration_since(UNIX_EPOCH)
      .unwrap_or_default()
      .as_nanos();
    let id = NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
      "v8x-js2wasm-{}-{timestamp}-{id}",
      std::process::id()
    ));
    fs::create_dir(&path)
      .map_err(|error| format!("create js2wasm temp directory: {error}"))?;
    Ok(Self { path })
  }
}

#[cfg(feature = "js2wasm_runtime_compile")]
impl Drop for TempDir {
  fn drop(&mut self) {
    let _ = fs::remove_dir_all(&self.path);
  }
}

#[cfg(all(test, feature = "js2wasm_deno_poc_replay"))]
mod deno_poc_replay_tests {
  use super::*;

  const TEST_V8X_REF: &str = "0123456789abcdef0123456789abcdef01234567";

  fn source_entries() -> String {
    POC_SOURCE_LOCK
      .iter()
      .map(|(path, bytes, sha256)| {
        format!(r#"{{"path":"{path}","bytes":{bytes},"sha256":"{sha256}"}}"#)
      })
      .collect::<Vec<_>>()
      .join(",")
  }

  fn valid_manifest(
    app_aot_sha256: &str,
    provider_aot_sha256: &str,
  ) -> (String, String) {
    let raw_contract_sha256 =
      bytes_digest(b"test Deno POC raw artifact contract");
    let app_raw_sha256 = bytes_digest(b"test app raw Wasm provenance");
    let provider_raw_sha256 =
      bytes_digest(b"test provider raw Wasm provenance");
    let attestation_sha256 =
      |role: &str, raw_sha256: &str, aot_sha256: &str| {
        let value = serde_json::json!({
          "schema_version": 1,
          "role": role,
          "wasmtime_version": POC_EXPECTED_WASMTIME_VERSION,
          "target": {
            "os": POC_EXPECTED_TARGET_OS,
            "arch": POC_EXPECTED_TARGET_ARCH,
            "triple": POC_EXPECTED_TARGET_TRIPLE,
          },
          "engine_config": {
            "wasm_function_references": true,
            "wasm_gc": true,
            "wasm_tail_call": true,
            "wasm_exceptions": true,
          },
          "raw_sha256": raw_sha256,
          "aot_sha256": aot_sha256,
        });
        let mut canonical = String::new();
        append_canonical_poc_json(&value, &mut canonical)
          .expect("canonicalize test precompile attestation");
        bytes_digest(canonical.as_bytes())
      };
    let app_attestation_sha256 =
      attestation_sha256("app", &app_raw_sha256, app_aot_sha256);
    let provider_attestation_sha256 = attestation_sha256(
      "runtime_eval_provider",
      &provider_raw_sha256,
      provider_aot_sha256,
    );
    let mut value: serde_json::Value = serde_json::from_str(&format!(
      r#"{{"schema_version":1,"raw_contract_sha256":"{raw_contract_sha256}","deno_ref":"{POC_EXPECTED_DENO_REF}","js2_ref":"{POC_EXPECTED_JS2_REF}","v8x_ref":"{TEST_V8X_REF}","wasmtime_version":"{POC_EXPECTED_WASMTIME_VERSION}","target":{{"os":"{POC_EXPECTED_TARGET_OS}","arch":"{POC_EXPECTED_TARGET_ARCH}","triple":"{POC_EXPECTED_TARGET_TRIPLE}"}},"engine_config":{{"wasm_function_references":true,"wasm_gc":true,"wasm_tail_call":true,"wasm_exceptions":true}},"compile_options_sha256":"{POC_EXPECTED_COMPILE_OPTIONS_SHA256}","sources":[{}],"artifacts":{{"app":{{"role":"app","raw_path":"deno-core.wasm","raw_sha256":"{app_raw_sha256}","aot_sha256":"{app_aot_sha256}","attestation_sha256":"{app_attestation_sha256}"}},"runtime_eval_provider":{{"role":"runtime_eval_provider","raw_path":"runtime-eval-provider.wasm","raw_sha256":"{provider_raw_sha256}","aot_sha256":"{provider_aot_sha256}","attestation_sha256":"{provider_attestation_sha256}"}}}}}}"#,
      source_entries(),
    ))
    .expect("construct valid Deno POC replay lock");
    let contract_sha256 =
      poc_contract_sha256(&value).expect("hash valid Deno POC replay lock");
    value
      .as_object_mut()
      .expect("replay lock is an object")
      .insert(
        "contract_sha256".to_string(),
        serde_json::Value::String(contract_sha256.clone()),
      );
    (
      serde_json::to_string(&value)
        .expect("serialize valid Deno POC replay lock"),
      contract_sha256,
    )
  }

  fn relock_manifest(mut manifest: String) -> String {
    let mut value: serde_json::Value =
      serde_json::from_str(&manifest).expect("parse test Deno POC lock");
    let contract_sha256 =
      poc_contract_sha256(&value).expect("hash test Deno POC lock");
    value
      .as_object_mut()
      .expect("replay lock is an object")
      .insert(
        "contract_sha256".to_string(),
        serde_json::Value::String(contract_sha256),
      );
    manifest = serde_json::to_string(&value).expect("serialize test lock");
    manifest
  }

  fn parse_test_manifest(
    contents: &str,
    compiled_v8x_ref: Option<&str>,
    compiled_contract_sha256: Option<&str>,
  ) -> Result<DenoPocManifest, String> {
    parse_deno_poc_manifest_with_refs(
      contents,
      compiled_v8x_ref,
      compiled_contract_sha256,
    )
  }

  fn temporary_path(label: &str) -> PathBuf {
    let nonce = std::time::SystemTime::now()
      .duration_since(std::time::UNIX_EPOCH)
      .unwrap()
      .as_nanos();
    std::env::temp_dir().join(format!(
      "v8x-deno-poc-replay-{label}-{}-{nonce}",
      std::process::id(),
    ))
  }

  #[test]
  fn parses_the_complete_pinned_manifest_without_raw_wasm_files() {
    let (manifest, contract_sha256) = valid_manifest(
      &bytes_digest(b"application AOT"),
      &bytes_digest(b"provider AOT"),
    );
    let parsed = parse_test_manifest(
      &manifest,
      Some(TEST_V8X_REF),
      Some(&contract_sha256),
    )
    .expect("parse complete replay lock");
    assert_eq!(parsed.sources.len(), 6);
    assert!(is_lowercase_sha256(&parsed.contract_sha256));
    assert!(is_lowercase_sha256(&parsed.raw_contract_sha256));
    assert_eq!(parsed.artifacts.app.raw_path, "deno-core.wasm");
    assert!(is_lowercase_sha256(
      &parsed.artifacts.app.attestation_sha256
    ));
    assert_eq!(
      parsed.artifacts.runtime_eval_provider.raw_path,
      "runtime-eval-provider.wasm"
    );
  }

  #[test]
  fn rejects_unknown_or_mismatched_pinned_manifest_fields() {
    let (manifest, contract_sha256) = valid_manifest(
      &bytes_digest(b"application AOT"),
      &bytes_digest(b"provider AOT"),
    );
    let unknown = manifest.replacen(
      "\"schema_version\":1",
      "\"schema_version\":1,\"unexpected\":true",
      1,
    );
    assert!(
      parse_test_manifest(&unknown, Some(TEST_V8X_REF), Some(&contract_sha256))
        .unwrap_err()
        .contains("unknown field")
    );
    assert!(
      parse_test_manifest(
        r#"{"schema_version":1}"#,
        Some(TEST_V8X_REF),
        Some(&contract_sha256),
      )
      .unwrap_err()
      .contains("missing field")
    );
    let mut missing_role_value: serde_json::Value =
      serde_json::from_str(&manifest).expect("parse test Deno POC lock");
    missing_role_value["artifacts"]["app"]
      .as_object_mut()
      .expect("application artifact is an object")
      .remove("role");
    let missing_role = serde_json::to_string(&missing_role_value)
      .expect("serialize test Deno POC lock");
    assert!(
      parse_test_manifest(
        &missing_role,
        Some(TEST_V8X_REF),
        Some(&contract_sha256),
      )
      .unwrap_err()
      .contains("missing field `role`")
    );
    let wrong_role = relock_manifest(manifest.replacen(
      "\"role\":\"app\"",
      "\"role\":\"provider\"",
      1,
    ));
    assert!(
      parse_test_manifest(
        &wrong_role,
        Some(TEST_V8X_REF),
        Some(&contract_sha256)
      )
      .unwrap_err()
      .contains("artifact role must be \"app\"")
    );

    let invalid_attestation = relock_manifest(manifest.replacen(
      &format!(
        "\"attestation_sha256\":\"{}\"",
        serde_json::from_str::<serde_json::Value>(&manifest).unwrap()
          ["artifacts"]["app"]["attestation_sha256"]
          .as_str()
          .unwrap(),
      ),
      &format!("\"attestation_sha256\":\"{}\"", "0".repeat(64),),
      1,
    ));
    assert!(
      parse_test_manifest(
        &invalid_attestation,
        Some(TEST_V8X_REF),
        Some(&contract_sha256),
      )
      .unwrap_err()
      .contains("attestation_sha256 mismatch")
    );

    let wrong_source = relock_manifest(manifest.replacen(
      "5a2dfbdc4bb81412575d035901a11788001c7e0110e3f736d16289891af44a52",
      "0000000000000000000000000000000000000000000000000000000000000000",
      1,
    ));
    assert!(
      parse_test_manifest(
        &wrong_source,
        Some(TEST_V8X_REF),
        Some(&contract_sha256)
      )
      .unwrap_err()
      .contains("sources[0].sha256 mismatch")
    );

    let wrong_bytes =
      relock_manifest(manifest.replacen("\"bytes\":19076", "\"bytes\":1", 1));
    assert!(
      parse_test_manifest(
        &wrong_bytes,
        Some(TEST_V8X_REF),
        Some(&contract_sha256)
      )
      .unwrap_err()
      .contains("sources[0].bytes mismatch")
    );

    let wrong_options = relock_manifest(manifest.replacen(
      POC_EXPECTED_COMPILE_OPTIONS_SHA256,
      "0000000000000000000000000000000000000000000000000000000000000000",
      1,
    ));
    assert!(
      parse_test_manifest(
        &wrong_options,
        Some(TEST_V8X_REF),
        Some(&contract_sha256)
      )
      .unwrap_err()
      .contains("compile_options_sha256 mismatch")
    );

    let invalid_contract_digest = manifest.replacen(
      "\"contract_sha256\":\"",
      "\"contract_sha256\":\"not-a-digest-",
      1,
    );
    assert!(
      parse_test_manifest(
        &invalid_contract_digest,
        Some(TEST_V8X_REF),
        Some(&contract_sha256),
      )
      .unwrap_err()
      .contains("field contract_sha256")
    );

    let mut missing_raw_contract_value: serde_json::Value =
      serde_json::from_str(&manifest).expect("parse test Deno POC lock");
    missing_raw_contract_value
      .as_object_mut()
      .expect("replay lock is an object")
      .remove("raw_contract_sha256");
    let missing_raw_contract_digest =
      serde_json::to_string(&missing_raw_contract_value)
        .expect("serialize test Deno POC lock");
    assert!(
      parse_test_manifest(
        &missing_raw_contract_digest,
        Some(TEST_V8X_REF),
        Some(&contract_sha256),
      )
      .unwrap_err()
      .contains("missing field `raw_contract_sha256`")
    );
    let invalid_raw_contract_digest = manifest.replacen(
      "\"raw_contract_sha256\":\"",
      "\"raw_contract_sha256\":\"not-a-digest-",
      1,
    );
    assert!(
      parse_test_manifest(
        &invalid_raw_contract_digest,
        Some(TEST_V8X_REF),
        Some(&contract_sha256),
      )
      .unwrap_err()
      .contains("field raw_contract_sha256")
    );

    let wrong_target = relock_manifest(manifest.replacen(
      POC_EXPECTED_TARGET_TRIPLE,
      "x86_64-unknown-linux-musl",
      1,
    ));
    assert!(
      parse_test_manifest(
        &wrong_target,
        Some(TEST_V8X_REF),
        Some(&contract_sha256)
      )
      .unwrap_err()
      .contains("target mismatch")
    );

    let disabled_engine_flag = relock_manifest(manifest.replacen(
      "\"wasm_gc\":true",
      "\"wasm_gc\":false",
      1,
    ));
    assert!(
      parse_test_manifest(
        &disabled_engine_flag,
        Some(TEST_V8X_REF),
        Some(&contract_sha256),
      )
      .unwrap_err()
      .contains("engine_config.wasm_gc must be true")
    );

    let unknown_engine_flag = manifest.replacen(
      "\"wasm_exceptions\":true",
      "\"wasm_exceptions\":true,\"unexpected\":true",
      1,
    );
    assert!(
      parse_test_manifest(
        &unknown_engine_flag,
        Some(TEST_V8X_REF),
        Some(&contract_sha256),
      )
      .unwrap_err()
      .contains("unknown field")
    );

    assert!(
      parse_test_manifest(&manifest, None, Some(&contract_sha256))
        .unwrap_err()
        .contains("built without V8X_JS2WASM_POC_V8X_REF")
    );
    assert!(
      parse_test_manifest(&manifest, Some(TEST_V8X_REF), None)
        .unwrap_err()
        .contains("built without V8X_JS2WASM_POC_CONTRACT_SHA256")
    );
    assert!(
      parse_test_manifest(&manifest, Some(TEST_V8X_REF), Some("not-a-digest"))
        .unwrap_err()
        .contains("compile-time V8X_JS2WASM_POC_CONTRACT_SHA256")
    );
    let other_contract_sha256 =
      bytes_digest(b"another valid replay lock digest");
    assert!(
      parse_test_manifest(
        &manifest,
        Some(TEST_V8X_REF),
        Some(&other_contract_sha256),
      )
      .unwrap_err()
      .contains("crate was built for")
    );
  }

  #[test]
  fn rejects_paired_manifest_and_aot_substitution_before_aot_read() {
    let (original_manifest, original_contract_sha256) = valid_manifest(
      &bytes_digest(b"original application AOT"),
      &bytes_digest(b"provider AOT"),
    );
    let replacement_aot = b"replacement application AOT";
    let replacement_aot_sha256 = bytes_digest(replacement_aot);
    let (replacement_manifest, replacement_contract_sha256) =
      valid_manifest(&replacement_aot_sha256, &bytes_digest(b"provider AOT"));
    assert_ne!(original_contract_sha256, replacement_contract_sha256);
    let paired_substitution_manifest = replacement_manifest.replacen(
      &replacement_contract_sha256,
      &original_contract_sha256,
      1,
    );

    // The pair's per-artifact hash agrees, but preserving the original compiled
    // contract digest makes the lock's canonical preimage mismatch. This check
    // runs before the replay path opens either AOT artifact.
    assert!(
      parse_test_manifest(
        &paired_substitution_manifest,
        Some(TEST_V8X_REF),
        Some(&original_contract_sha256),
      )
      .unwrap_err()
      .contains("canonical lock preimage")
    );
    let artifact = temporary_path("paired-substitution");
    fs::write(&artifact, replacement_aot).unwrap();
    read_verified_poc_aot(&artifact, &replacement_aot_sha256, "application")
      .expect("substituted AOT agrees with substituted manifest hash");
    fs::remove_file(&artifact).unwrap();
    assert!(
      parse_test_manifest(
        &original_manifest,
        Some(TEST_V8X_REF),
        Some(&original_contract_sha256),
      )
      .is_ok()
    );
  }

  #[test]
  fn verifies_aot_bytes_and_rejects_tampering_or_missing_artifacts() {
    let artifact = temporary_path("aot");
    let expected = bytes_digest(b"verified AOT bytes");
    fs::write(&artifact, b"verified AOT bytes").unwrap();
    let verified =
      read_verified_poc_aot(&artifact, &expected, "application").unwrap();
    assert_eq!(verified.bytes, b"verified AOT bytes");

    fs::write(&artifact, b"").unwrap();
    assert!(
      read_verified_poc_aot(&artifact, &expected, "application")
        .unwrap_err()
        .contains("is empty")
    );

    fs::write(&artifact, b"tampered AOT bytes").unwrap();
    assert!(
      read_verified_poc_aot(&artifact, &expected, "application")
        .unwrap_err()
        .contains("hash mismatch")
    );
    fs::remove_file(&artifact).unwrap();

    assert!(
      read_verified_poc_aot(&artifact, &expected, "application")
        .unwrap_err()
        .contains("read verified Deno POC application AOT artifact")
    );
  }

  #[test]
  fn rejects_compiler_configuration_in_replay() {
    let error =
      reject_poc_replay_compiler_envs(|name| name == "V8X_JS2WASM_COMPILER")
        .unwrap_err();
    assert!(error.contains("V8X_JS2WASM_COMPILER"));
    assert!(reject_poc_replay_compiler_envs(|_| false).is_ok());
  }
}
