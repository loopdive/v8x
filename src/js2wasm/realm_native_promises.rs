use super::*;

#[derive(Clone)]
pub(in crate::js2wasm) struct NativePromiseMirror {
  host: *const Promise,
  owner: Rc<RefCell<DenoRuntime>>,
  packet: RealmValue,
  promise: RealmValue,
}

pub(super) fn transfer(
  access: &mut dyn RealmAccess,
  owner: &Rc<RefCell<DenoRuntime>>,
  host: *const Promise,
) -> Result<RealmValue, String> {
  // Inspect settled payload graphs before publishing cyclic identity bindings.
  // Unsupported nested values must not leave a pending mirror behind.
  host_values::validate(owner, host.cast())?;
  let (settlement, result, handled) = unsafe { promise_state(host) }
    .map(|state| (state.settlement, state.result, state.handled))
    .ok_or_else(|| "native Promise disappeared".to_string())?;
  let packet = access.realm_handle("__v8x_value_native_promise_create", &[])?;
  let packet_id = access.realm_check(packet)?;
  let promise =
    access.realm_handle("__v8x_value_native_promise_value", &[packet_id])?;
  let mirror = NativePromiseMirror {
    host,
    owner: owner.clone(),
    packet,
    promise,
  };
  // Publish before transferring a settlement payload: it can refer back to
  // this Promise. Generic reverse conversion must return the original host.
  unsafe { isolate_state(current_isolate()) }
    .realm_objects
    .push(RealmObjectBinding {
      host: host.cast(),
      runtime: owner.clone(),
      value: promise,
    });
  unsafe { isolate_state(current_isolate()) }
    .native_promise_mirrors
    .push(mirror.clone());
  let initialized = (|| -> Result<(), String> {
    if handled {
      access
        .realm_promise_handler(access.realm_check(promise)?, true)?
        .ok_or_else(|| "native Promise copy lacks handler ABI".to_string())?;
    }
    if settlement != PromiseSettlement::Pending {
      synchronize(access, &mirror, result, settlement)?;
    }
    Ok(())
  })();
  if let Err(error) = initialized {
    let state = unsafe { isolate_state(current_isolate()) };
    state
      .realm_objects
      .retain(|entry| entry.host != host.cast());
    state
      .native_promise_mirrors
      .retain(|entry| entry.host != host);
    return Err(error);
  }
  Ok(promise)
}

fn synchronize(
  access: &mut dyn RealmAccess,
  mirror: &NativePromiseMirror,
  result: *const Value,
  settlement: PromiseSettlement,
) -> Result<bool, String> {
  let handled = access
    .realm_promise_handler(access.realm_check(mirror.promise)?, false)?
    .ok_or_else(|| "native Promise copy lacks handler ABI".to_string())?;
  let result = into_realm(access, &mirror.owner, result)?;
  // The original native settlement owns its rejection notification. Suppress
  // only this copied Promise's event, never other reentrant Promise events.
  access.realm_native_promise_settle(
    mirror.packet,
    mirror.promise,
    result,
    settlement == PromiseSettlement::Rejected,
  )?;
  Ok(handled)
}

pub(in crate::js2wasm) fn settle(
  host: *const Promise,
  result: *const Value,
  settlement: PromiseSettlement,
) -> Option<Result<bool, String>> {
  let mirror = unsafe { isolate_state(current_isolate()) }
    .native_promise_mirrors
    .iter()
    .find(|mirror| mirror.host == host)
    .cloned()?;
  Some(callback_access::with_owner(&mirror.owner, |access| {
    synchronize(access, &mirror, result, settlement)
  }))
}
