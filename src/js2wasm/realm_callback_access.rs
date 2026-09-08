use super::*;

type Action<'a> = dyn FnMut(&mut dyn RealmAccess) + 'a;
#[derive(Clone)]
struct Active {
  owner: Rc<RefCell<DenoRuntime>>,
  slot: *mut c_void,
  run: unsafe fn(*mut c_void, &mut Action<'_>) -> Result<(), String>,
}
struct Slot<'a>(RefCell<&'a mut dyn RealmAccess>);
thread_local! {
  static ACTIVE: RefCell<Vec<Active>> = const { RefCell::new(Vec::new()) };
}
struct Guard;
impl Drop for Guard {
  fn drop(&mut self) {
    ACTIVE.with(|stack| {
      stack.borrow_mut().pop();
    });
  }
}

unsafe fn run_slot(
  slot: *mut c_void,
  action: &mut Action<'_>,
) -> Result<(), String> {
  // Only installed by with_active for this stack frame, removed by Guard
  // before Slot is dropped. The RefCell prevents overlapping mutable access.
  let slot = unsafe { &*(slot.cast::<Slot<'_>>()) };
  let mut access = slot
    .0
    .try_borrow_mut()
    .map_err(|_| "callback realm access is already borrowed".to_string())?;
  action(&mut **access);
  Ok(())
}

pub(super) fn with_active<T>(
  owner: &Rc<RefCell<DenoRuntime>>,
  access: &mut dyn RealmAccess,
  action: impl FnOnce() -> T,
) -> T {
  let mut slot = Slot(RefCell::new(access));
  ACTIVE.with(|stack| {
    stack.borrow_mut().push(Active {
      owner: owner.clone(),
      slot: (&mut slot as *mut Slot<'_>).cast(),
      run: run_slot,
    })
  });
  let _guard = Guard;
  action()
}

pub(super) fn with_owner<T>(
  owner: &Rc<RefCell<DenoRuntime>>,
  action: impl FnOnce(&mut dyn RealmAccess) -> Result<T, String>,
) -> Result<T, String> {
  let active = ACTIVE.with(|stack| {
    stack
      .borrow()
      .iter()
      .rev()
      .find(|entry| Rc::ptr_eq(&entry.owner, owner))
      .cloned()
  });
  if let Some(active) = active {
    let mut action = Some(action);
    let mut result = None;
    unsafe {
      (active.run)(active.slot, &mut |access| {
        result = Some(action.take().expect("realm action runs once")(access));
      })?;
    }
    return result
      .ok_or_else(|| "callback realm action did not run".to_string())?;
  }
  let mut runtime = owner.try_borrow_mut().map_err(|_| {
    "realm is already executing without callback access".to_string()
  })?;
  action(&mut *runtime)
}
