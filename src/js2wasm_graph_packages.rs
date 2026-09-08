use super::*;

const INPUT_DIR: &str = "V8X_JS2WASM_AOT_GRAPH_DIR";
#[cfg(feature = "js2wasm_runtime_compile")]
const OUTPUT_DIR: &str = "V8X_JS2WASM_ARTIFACT_OUTPUT_DIR";

fn graph_path(
  directory: &Path,
  entry: &str,
  modules: &[SourceModule],
) -> PathBuf {
  directory.join(format!("{}.cwasm", graph_digest(entry, modules)))
}

/// A directory is an explicitly trusted deployment input, just like AOT_MODULE.
/// Source-derived filenames only select candidates; graph and byte bindings
/// are still verified before Wasmtime deserializes native code.
pub(super) fn configured_input(
  entry: &str,
  modules: &[SourceModule],
) -> Result<Option<PathBuf>, String> {
  let single = std::env::var_os("V8X_JS2WASM_AOT_MODULE");
  let directory = std::env::var_os(INPUT_DIR);
  match (single, directory) {
    (Some(_), Some(_)) => Err(format!(
      "configure either V8X_JS2WASM_AOT_MODULE or {INPUT_DIR}, not both"
    )),
    (Some(path), None) => Ok(Some(PathBuf::from(path))),
    (None, Some(directory)) => {
      Ok(Some(graph_path(Path::new(&directory), entry, modules)))
    }
    (None, None) => Ok(None),
  }
}

#[cfg(feature = "js2wasm_runtime_compile")]
pub(super) fn publish_outputs(
  bytes: &[u8],
  entry: &str,
  modules: &[SourceModule],
) -> Result<(), String> {
  if let Some(output) = std::env::var_os("V8X_JS2WASM_ARTIFACT_OUTPUT") {
    publish_graph_artifact(Path::new(&output), bytes, entry, modules)?;
  }
  if let Some(directory) = std::env::var_os(OUTPUT_DIR) {
    publish_graph_artifact(
      &graph_path(Path::new(&directory), entry, modules),
      bytes,
      entry,
      modules,
    )?;
  }
  Ok(())
}

#[cfg(feature = "js2wasm_runtime_compile")]
pub(super) fn publish_cached_outputs(
  path: &Path,
  entry: &str,
  modules: &[SourceModule],
) -> Result<(), String> {
  if std::env::var_os("V8X_JS2WASM_ARTIFACT_OUTPUT").is_none()
    && std::env::var_os(OUTPUT_DIR).is_none()
  {
    return Ok(());
  }
  let bytes = fs::read(path)
    .map_err(|e| format!("read graph package cache input: {e}"))?;
  publish_outputs(&bytes, entry, modules)
}

#[cfg(feature = "js2wasm_runtime_compile")]
pub(super) fn test_graph_packages_bind_entry_source_and_bytes() {
  let directory = TempDir::new().unwrap();
  let modules = vec![SourceModule {
    specifier: "ext:sample/a.js".into(),
    source: "export const a = 1;".into(),
  }];
  let first = graph_path(&directory.path, "ext:sample/a.js", &modules);
  let second = graph_path(&directory.path, "file:///app.js", &modules);
  assert_ne!(first, second);
  assert_eq!(first.parent(), Some(directory.path.as_path()));
  publish_graph_artifact(
    &first,
    b"first trusted package",
    "ext:sample/a.js",
    &modules,
  )
  .unwrap();
  publish_graph_artifact(
    &second,
    b"second trusted package",
    "file:///app.js",
    &modules,
  )
  .unwrap();
  verify_graph_binding(&first, "ext:sample/a.js", &modules).unwrap();
  verify_graph_binding(&second, "file:///app.js", &modules).unwrap();
  assert!(verify_graph_binding(&first, "file:///app.js", &modules).is_err());
  let changed = vec![SourceModule {
    specifier: modules[0].specifier.clone(),
    source: "export const a = 2;".into(),
  }];
  assert_ne!(
    first,
    graph_path(&directory.path, "ext:sample/a.js", &changed)
  );
  assert!(verify_graph_binding(&first, "ext:sample/a.js", &changed).is_err());
  fs::write(&first, b"modified package").unwrap();
  assert!(verify_graph_binding(&first, "ext:sample/a.js", &modules).is_err());
}
