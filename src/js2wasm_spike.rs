//! Experimental js2wasm module backend.
//!
//! js2wasm remains a build-time compiler. At runtime v8x embeds Wasmtime,
//! instantiates the resulting WasmGC module once, and keeps that instance alive
//! with the owning V8 module handle. The first Deno-shaped host seam is
//! `Deno.cwd()`: the compiled TypeScript wrapper reconstructs its string from
//! two primitive UTF-16 imports, avoiding a JavaScript-host `externref` ABI.

use std::collections::HashSet;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use wasmtime::{Caller, Config, Engine, Instance, Linker, Module, Store};

static NEXT_TEMP_ID: AtomicU64 = AtomicU64::new(0);

const CWD_LENGTH_IMPORT: &str = "__v8x_op_cwd_utf16_length";
const CWD_CODE_UNIT_IMPORT: &str = "__v8x_op_cwd_utf16_code_unit";
const DENO_IMPORT_MODULE: &str = "v8x:deno";
const CWD_LENGTH_PROBE: &str = "__v8x_probe_cwd_utf16_length";
const CWD_CHECKSUM_PROBE: &str = "__v8x_probe_cwd_utf16_checksum";

pub(crate) struct SourceModule {
  pub(crate) specifier: String,
  pub(crate) source: String,
}

struct DenoHostState {
  cwd: Vec<u16>,
  cwd_op_calls: u64,
}

/// One persistent Wasmtime store and instance owned by a v8x module handle.
pub(crate) struct DenoRuntime {
  store: Store<DenoHostState>,
  instance: Instance,
}

impl DenoRuntime {
  fn instantiate(binary: &[u8], cwd: PathBuf) -> Result<Self, String> {
    let mut config = Config::new();
    config
      .wasm_function_references(true)
      .wasm_gc(true)
      .wasm_tail_call(true)
      .wasm_exceptions(true);
    let engine = Engine::new(&config)
      .map_err(|error| format!("configure embedded Wasmtime: {error}"))?;
    let module = Module::new(&engine, binary).map_err(|error| {
      format!("compile js2wasm artifact in Wasmtime: {error:#}")
    })?;
    if std::env::var_os("V8X_JS2WASM_TRACE_IMPORTS").is_some() {
      for import in module.imports() {
        eprintln!("v8x/js2wasm import {}::{}", import.module(), import.name());
      }
    }
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

    let cwd = cwd.to_string_lossy().encode_utf16().collect();
    let mut store = Store::new(
      &engine,
      DenoHostState {
        cwd,
        cwd_op_calls: 0,
      },
    );
    let instance = linker
      .instantiate(&mut store, &module)
      .map_err(|error| format!("instantiate js2wasm artifact: {error}"))?;
    let mut runtime = Self { store, instance };
    runtime.verify_cwd_probe()?;
    Ok(runtime)
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
    if self.store.data().cwd_op_calls == 0 {
      return Err(
        "Deno.cwd() probe did not execute its typed host imports".to_string(),
      );
    }
    Ok(())
  }
}

pub(crate) fn compile_and_instantiate(
  entry: &str,
  modules: &[SourceModule],
) -> Result<DenoRuntime, String> {
  if modules.is_empty() {
    return Err("js2wasm module graph is empty".to_string());
  }
  let binary =
    if let Some(artifact) = std::env::var_os("V8X_JS2WASM_AOT_MODULE") {
      fs::read(&artifact).map_err(|error| {
        format!(
          "read ahead-of-time js2wasm artifact {}: {error}",
          Path::new(&artifact).display()
        )
      })?
    } else {
      compile_graph(entry, modules)?
    };
  let cwd = std::env::current_dir()
    .map_err(|error| format!("resolve Deno.cwd() host value: {error}"))?;
  DenoRuntime::instantiate(&binary, cwd)
}

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
  let binary = fs::read(&wasm_path)
    .map_err(|error| format!("read compiled js2wasm artifact: {error}"))?;
  if let Some(output) = std::env::var_os("V8X_JS2WASM_ARTIFACT_OUTPUT") {
    fs::write(&output, &binary).map_err(|error| {
      format!(
        "write ahead-of-time js2wasm artifact {}: {error}",
        Path::new(&output).display()
      )
    })?;
  }
  Ok(binary)
}

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

struct TempDir {
  path: PathBuf,
}

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

impl Drop for TempDir {
  fn drop(&mut self) {
    let _ = fs::remove_dir_all(&self.path);
  }
}
