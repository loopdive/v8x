//! Experimental js2wasm module backend.
//!
//! js2wasm remains a build-time compiler. At runtime v8x embeds compiler-free
//! Wasmtime, shares one engine plus each precompiled module across isolates,
//! and keeps a private store/instance alive with each owning V8 module handle.
//! The first Deno-shaped host seam is `Deno.cwd()`: the compiled TypeScript
//! wrapper reconstructs its string from two direct UTF-16 imports, avoiding a
//! JavaScript-host `externref` ABI or a WASI/component boundary.

use sha2::{Digest, Sha256};
use std::collections::HashMap;
#[cfg(feature = "js2wasm_runtime_compile")]
use std::collections::HashSet;
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
const DENO_IMPORT_MODULE: &str = "v8x:deno";
const GRAPH_BINDING_SUFFIX: &str = ".graph-sha256";
const GRAPH_BINDING_DOMAIN: &[u8] = b"v8x/js2wasm graph binding v1\0";
const CWD_LENGTH_PROBE: &str = "__v8x_probe_cwd_utf16_length";
const CWD_CHECKSUM_PROBE: &str = "__v8x_probe_cwd_utf16_checksum";
const DENO_CORE_BOOTSTRAP_PROBE: &str = "__v8x_probe_deno_core_bootstrap";
const DENO_CORE_WRAPPERS_STAGE: &str = "__v8x_stage_deno_core_wrappers";
const DENO_CORE_MODULE_STAGE: &str = "__v8x_stage_deno_core_module";
const DENO_CORE_USAGE_STAGE: &str = "__v8x_stage_deno_hello_world_usage";
const DENO_CORE_STAGE_STATE_PROBE: &str = "__v8x_probe_deno_stage_state";
const DENO_CORE_SET_TICK_INFO: &str = "__v8x_set_deno_tick_info";
const DENO_CORE_SET_IMMEDIATE_INFO: &str = "__v8x_set_deno_immediate_info";
const DENO_CORE_SET_TIMER_INFO: &str = "__v8x_set_deno_timer_info";
#[cfg(feature = "js2wasm_runtime_compile")]
const DENO_CORE_AOT_OUTPUT_ENV: &str = "V8X_JS2WASM_DENO_CORE_AOT_OUTPUT";
const DEFERRED_BOOTSTRAP_IMPORTS: &[(&str, &str)] = &[
  ("env", "Promise_new"),
  ("env", "Promise_all"),
  ("env", "Promise_allSettled"),
  ("env", "Promise_any"),
  ("env", "Promise_race"),
  ("js2wasm:runtime-eval", "__runtime_apply_interpreted"),
  ("js2wasm:runtime-eval", "__runtime_indirect_eval"),
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
];

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
  deno_op_print: Option<usize>,
  deno_op_sum: Option<usize>,
  pending_sum: Option<PendingSum>,
  pending_print: Option<PendingPrint>,
  last_error: Option<DenoBridgeError>,
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
  match crate::js2wasm::invoke_prelinked_deno_sum(function, is_array, &values) {
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

  match crate::js2wasm::invoke_prelinked_deno_print(
    function,
    &code_units,
    is_error,
  ) {
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

#[derive(Hash, PartialEq, Eq)]
enum ModuleCacheKey {
  TrustedFile(PathBuf),
  GraphBoundFile {
    artifact: PathBuf,
    graph_sha256: String,
    artifact_sha256: String,
  },
  #[cfg(feature = "js2wasm_runtime_compile")]
  DevelopmentBytes(Vec<u8>),
}

struct SharedDenoRuntime {
  engine: Engine,
  linker: Linker<DenoHostState>,
  modules: Mutex<HashMap<ModuleCacheKey, InstancePre<DenoHostState>>>,
  module_loads: AtomicUsize,
  instantiations: AtomicUsize,
}

static SHARED_DENO_RUNTIME: OnceLock<Result<SharedDenoRuntime, String>> =
  OnceLock::new();

/// Diagnostic counters exposed only to verify the experimental backend.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Js2WasmRuntimeStats {
  pub cached_modules: usize,
  pub module_loads: usize,
  pub instantiations: usize,
}

impl SharedDenoRuntime {
  fn new() -> Result<Self, String> {
    let mut config = Config::new();
    config
      .wasm_function_references(true)
      .wasm_gc(true)
      .wasm_tail_call(true)
      .wasm_exceptions(true);
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
    Ok(Self {
      engine,
      linker,
      modules: Mutex::new(HashMap::new()),
      module_loads: AtomicUsize::new(0),
      instantiations: AtomicUsize::new(0),
    })
  }

  fn precompiled_file(
    &self,
    artifact: &Path,
  ) -> Result<InstancePre<DenoHostState>, String> {
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
    let instance_pre = self.instance_pre(&module)?;
    modules.insert(key, instance_pre.clone());
    self.module_loads.fetch_add(1, Ordering::Relaxed);
    Ok(instance_pre)
  }

  fn precompiled_graph_file(
    &self,
    artifact: &Path,
    entry: &str,
    modules: &[SourceModule],
  ) -> Result<InstancePre<DenoHostState>, String> {
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
    let instance_pre = self.instance_pre(&module)?;
    modules.insert(key, instance_pre.clone());
    self.module_loads.fetch_add(1, Ordering::Relaxed);
    Ok(instance_pre)
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
  ) -> Result<InstancePre<DenoHostState>, String> {
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
    let instance_pre = self.instance_pre(&module)?;
    modules.insert(key, instance_pre.clone());
    self.module_loads.fetch_add(1, Ordering::Relaxed);
    Ok(instance_pre)
  }

  fn trace_imports(&self, module: &Module) {
    if std::env::var_os("V8X_JS2WASM_TRACE_IMPORTS").is_some() {
      for import in module.imports() {
        eprintln!("v8x/js2wasm import {}::{}", import.module(), import.name());
      }
    }
  }

  fn instance_pre(
    &self,
    module: &Module,
  ) -> Result<InstancePre<DenoHostState>, String> {
    for import in module.imports() {
      let known_deno_import = import.module() == DENO_IMPORT_MODULE
        && DENO_HOST_IMPORTS.contains(&import.name());
      let deferred_bootstrap_import = DEFERRED_BOOTSTRAP_IMPORTS
        .iter()
        .any(|candidate| *candidate == (import.module(), import.name()));
      if !known_deno_import && !deferred_bootstrap_import {
        return Err(format!(
          "unimplemented js2wasm host import {}::{}",
          import.module(),
          import.name(),
        ));
      }
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
    linker
      .instantiate_pre(module)
      .map_err(|error| format!("resolve js2wasm host imports: {error:#}"))
  }

  fn stats(&self) -> Result<Js2WasmRuntimeStats, String> {
    let modules = self.modules.lock().map_err(|_| {
      "lock shared js2wasm module cache for diagnostics".to_string()
    })?;
    Ok(Js2WasmRuntimeStats {
      cached_modules: modules.len(),
      module_loads: self.module_loads.load(Ordering::Relaxed),
      instantiations: self.instantiations.load(Ordering::Relaxed),
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
#[cfg(feature = "js2wasm_runtime_compile")]
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
  let precompiled = shared.precompile(&wasm)?;
  persist_precompiled_deno_core_artifact(&precompiled)?;
  let instance_pre = shared.development_bytes(&precompiled)?;
  let cwd = std::env::current_dir()
    .map_err(|error| format!("resolve test working directory: {error}"))?;
  DenoRuntime::instantiate(shared, &instance_pre, cwd.clone())?;
  DenoRuntime::instantiate(shared, &instance_pre, cwd)?;
  Ok(())
}

/// Instantiate the prelinked core-bootstrap transaction used by the
/// experimental classic-script bridge. Production uses a trusted artifact;
/// development builds may precompile the exact raw module in-process.
pub(crate) fn deno_core_bootstrap_runtime_from_env()
-> Result<DenoRuntime, String> {
  let shared = shared_runtime()?;
  let instance_pre = if let Some(artifact) =
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
      persist_precompiled_deno_core_artifact(&precompiled)?;
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
  DenoRuntime::instantiate(shared, &instance_pre, cwd)
}

#[cfg(feature = "js2wasm_runtime_compile")]
fn persist_precompiled_deno_core_artifact(
  artifact: &[u8],
) -> Result<(), String> {
  let Some(output) = std::env::var_os(DENO_CORE_AOT_OUTPUT_ENV) else {
    return Ok(());
  };
  fs::write(&output, artifact).map_err(|error| {
    format!(
      "write precompiled Deno core artifact {}: {error}",
      Path::new(&output).display()
    )
  })
}

/// One private Wasmtime store and instance owned by a v8x module handle.
pub(crate) struct DenoRuntime {
  store: Store<DenoHostState>,
  instance: Instance,
}

impl DenoRuntime {
  fn instantiate(
    shared: &SharedDenoRuntime,
    instance_pre: &InstancePre<DenoHostState>,
    cwd: PathBuf,
  ) -> Result<Self, String> {
    let cwd = cwd.to_string_lossy().encode_utf16().collect();
    let mut store = Store::new(
      &shared.engine,
      DenoHostState {
        cwd,
        cwd_op_calls: 0,
        deno_op_print: None,
        deno_op_sum: None,
        pending_sum: None,
        pending_print: None,
        last_error: None,
      },
    );
    let instance = instance_pre
      .instantiate(&mut store)
      .map_err(|error| format!("instantiate js2wasm artifact: {error}"))?;
    shared.instantiations.fetch_add(1, Ordering::Relaxed);
    let mut runtime = Self { store, instance };
    runtime.run_deferred_module_init()?;
    runtime.verify_cwd_probe()?;
    runtime.verify_deno_core_bootstrap_probe()?;
    Ok(runtime)
  }

  pub(crate) fn bind_deno_ops(
    &mut self,
    print: usize,
    sum: usize,
  ) -> Result<(), String> {
    if print == 0 || sum == 0 {
      return Err(
        "cannot bind an empty Deno op Function handle to Wasmtime".to_string(),
      );
    }
    let state = self.store.data_mut();
    match (state.deno_op_print, state.deno_op_sum) {
      (None, None) => {
        state.deno_op_print = Some(print);
        state.deno_op_sum = Some(sum);
        Ok(())
      }
      (Some(previous_print), Some(previous_sum))
        if previous_print == print && previous_sum == sum =>
      {
        Ok(())
      }
      _ => Err(
        "Deno op Function handles were rebound to different values".to_string(),
      ),
    }
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
    let Some(probe) = self
      .instance
      .get_func(&mut self.store, DENO_CORE_BOOTSTRAP_PROBE)
    else {
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

  pub(crate) fn advance_deno_core_wrappers(&mut self) -> Result<bool, String> {
    self.advance_deno_core_stage(DENO_CORE_WRAPPERS_STAGE, 42.0, 1.0)
  }

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
  let instance_pre = if let Some(artifact) =
    std::env::var_os("V8X_JS2WASM_AOT_MODULE")
  {
    let artifact = Path::new(&artifact);
    shared.precompiled_graph_file(artifact, entry, modules)?
  } else {
    #[cfg(feature = "js2wasm_runtime_compile")]
    {
      let wasm = compile_graph(entry, modules)?;
      let artifact = shared.precompile(&wasm)?;
      if let Some(output) = std::env::var_os("V8X_JS2WASM_ARTIFACT_OUTPUT") {
        fs::write(&output, &artifact).map_err(|error| {
          format!(
            "write precompiled js2wasm artifact {}: {error}",
            Path::new(&output).display()
          )
        })?;
        write_graph_binding(Path::new(&output), &artifact, entry, modules)?;
      }
      shared.development_bytes(&artifact)?
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
  DenoRuntime::instantiate(shared, &instance_pre, cwd)
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

  let script =
    std::env::var_os("V8X_JS2WASM_COMPILER_SCRIPT").ok_or_else(|| {
      "V8X_JS2WASM_COMPILER_SCRIPT must point to compile-graph.ts (or set V8X_JS2WASM_AOT_MODULE)".to_string()
    })?;
  let compiler =
    std::env::var_os("V8X_JS2WASM_COMPILER").unwrap_or_else(|| "node".into());
  let mut compile = Command::new(&compiler);
  if Path::new(&compiler)
    .file_name()
    .is_some_and(|name| name == "node")
  {
    compile.args(["--import", "tsx"]);
  }
  compile
    .arg(script)
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
