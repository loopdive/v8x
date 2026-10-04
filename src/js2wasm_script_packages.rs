use super::*;

const INPUT_DIR: &str = "V8X_JS2WASM_AOT_SCRIPT_DIR";

// Script goal and completion ABI are distinct from Module graph identity.
pub(super) fn digest(specifier: &str, source: &str) -> String {
  let mut hasher = Sha256::new();
  hasher.update(b"v8x/js2wasm Script completion ABI v1\0");
  update_graph_digest(&mut hasher, specifier.as_bytes());
  update_graph_digest(&mut hasher, source.as_bytes());
  format!("{:x}", hasher.finalize())
}

pub(super) fn configured_input(
  specifier: &str,
  source: &str,
) -> Option<PathBuf> {
  std::env::var_os(INPUT_DIR).map(|directory| {
    PathBuf::from(directory)
      .join(format!("{}.cwasm", digest(specifier, source)))
  })
}
