//! Deno bootstrap transaction mirrored into JS2/Wasm from the QuickJS host
//! compatibility profile.
//!
//! QuickJS supplies the broad rusty_v8 ABI needed by `deno_core`; the exact,
//! hash-pinned bootstrap scripts are additionally committed to one persistent
//! JS2/Wasm instance per QuickJS context. This keeps the common precompiled
//! transaction on JS2/Wasm without pretending its intentionally small V8 ABI
//! already implements every diagnostic conversion and inspector probe.

use std::cell::RefCell;
use std::collections::HashMap;

use crate::js2wasm_spike::DenoRuntime;

const PRELINKED_SCRIPTS: [(&str, u64); 4] = [
  ("ext:core/00_primordials.js", 0x49d0_171d_7d2c_3f4d),
  ("ext:core/00_infra.js", 0xe1a2_6738_75ca_364c),
  ("ext:core/02_timers.js", 0xcbd2_6ee0_c68d_cb66),
  ("ext:core/01_core.js", 0xd2f9_d9c6_2c03_7a70),
];
// tools/deno/deno-jsc-integration.patch makes one audited compatibility-only
// rewrite to 01_core.js (skip already-installed non-configurable globals).
// The JS2/Wasm artifact deliberately retains pristine DENO_REF input; the host
// side may therefore present either of these two exact sources at Script::Run.
const PATCHED_01_CORE_HASH: u64 = 0x9a86_06e5_0118_e568;

struct ContextState {
  phase: usize,
  runtime: DenoRuntime,
}

thread_local! {
  static CONTEXTS: RefCell<HashMap<usize, ContextState>> =
    RefCell::new(HashMap::new());
}

fn fnv1a64(source: &[u8]) -> u64 {
  source.iter().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
    (hash ^ u64::from(*byte)).wrapping_mul(0x0000_0100_0000_01b3)
  })
}

fn manifest_entry(specifier: &str) -> Option<(usize, u64)> {
  PRELINKED_SCRIPTS
    .iter()
    .enumerate()
    .find_map(|(phase, (candidate, hash))| {
      (*candidate == specifier).then_some((phase, *hash))
    })
}

fn artifact_is_configured() -> bool {
  std::env::var_os("V8X_JS2WASM_DENO_CORE_AOT_MODULE").is_some()
    || std::env::var_os("V8X_JS2WASM_DENO_CORE_WASM").is_some()
}

/// Validate and provision the JS2/Wasm transaction before QuickJS evaluates
/// the corresponding classic script. State advances only after QuickJS
/// succeeds, so a thrown bootstrap script cannot commit half a transaction.
pub(crate) fn prepare_script(
  context: usize,
  specifier: &str,
  source: &[u8],
) -> Result<bool, String> {
  let Some((phase, expected_hash)) = manifest_entry(specifier) else {
    return Ok(false);
  };
  let actual_hash = fnv1a64(source);
  let is_patched_01_core =
    specifier == "ext:core/01_core.js" && actual_hash == PATCHED_01_CORE_HASH;
  if actual_hash != expected_hash && !is_patched_01_core {
    return Err(format!(
      "prelinked script {specifier:?} has FNV-1a hash {actual_hash:#018x}, expected {expected_hash:#018x}"
    ));
  }
  // The compatibility profile is also the broad conformance gate. CI may run
  // it without an external js2 checkout; the dedicated `js2wasm_deno_poc`
  // gate is the non-vacuous artifact requirement. Whenever an artifact is
  // configured, however, every exact bootstrap Script::Run must mirror it.
  if !artifact_is_configured() {
    return Ok(false);
  }

  CONTEXTS.with(|contexts| {
    let mut contexts = contexts.borrow_mut();
    if phase == 0 && !contexts.contains_key(&context) {
      let runtime =
        crate::js2wasm_spike::deno_core_bootstrap_runtime_from_env()?;
      contexts.insert(context, ContextState { phase: 0, runtime });
    }
    let state = contexts.get(&context).ok_or_else(|| {
      format!(
        "prelinked Deno core script {specifier:?} arrived before {:?}",
        PRELINKED_SCRIPTS[0].0
      )
    })?;
    if state.phase != phase {
      return Err(format!(
        "prelinked Deno core script order mismatch: received {specifier:?} at phase {}, expected {:?}",
        state.phase,
        PRELINKED_SCRIPTS
          .get(state.phase)
          .map(|entry| entry.0)
          .unwrap_or("<complete>")
      ));
    }
    Ok(true)
  })
}

/// Commit a successfully evaluated bootstrap script to the persistent Wasmtime
/// instance. The final wrapper advances the artifact's explicit stage machine.
pub(crate) fn commit_script(
  context: usize,
  specifier: &str,
) -> Result<(), String> {
  let Some((phase, _)) = manifest_entry(specifier) else {
    return Ok(());
  };
  CONTEXTS.with(|contexts| {
    let mut contexts = contexts.borrow_mut();
    let state = contexts
      .get_mut(&context)
      .ok_or_else(|| "prelinked Deno context disappeared".to_string())?;
    if state.phase != phase {
      return Err(format!(
        "prelinked Deno commit phase changed from {phase} to {}",
        state.phase
      ));
    }
    if phase + 1 == PRELINKED_SCRIPTS.len() {
      let advanced = state.runtime.advance_deno_core_wrappers()?;
      if !advanced {
        return Err(
          "JS2/Wasm artifact has no staged Deno wrapper export".to_string(),
        );
      }
    }
    state.phase += 1;
    Ok(())
  })
}

pub(crate) fn forget_context(context: usize) {
  CONTEXTS.with(|contexts| {
    contexts.borrow_mut().remove(&context);
  });
}
