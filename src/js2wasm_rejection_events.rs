//! Ordered retained-value notifications. The host import never calls user code.

#[derive(Clone, Copy, Debug)]
pub(crate) struct PendingPromiseRejection {
  pub(crate) realm_id: usize,
  pub(crate) isolate: usize,
  pub(crate) event: u8,
  pub(crate) promise: f64,
  pub(crate) reason: f64,
  pub(crate) continuation_data: usize,
  pub(crate) realm_owner_identity: usize,
}

impl PendingPromiseRejection {
  pub(crate) fn new(
    realm_id: usize,
    isolate: usize,
    event: f64,
    promise: f64,
    reason: f64,
  ) -> Result<Self, String> {
    if realm_id == 0 {
      return Err("rejection event has no owning realm".into());
    }
    let event = match event {
      0.0 => 0,
      1.0 => 1,
      2.0 => 2,
      3.0 => 3,
      _ => return Err("invalid Promise rejection event".into()),
    };
    for handle in [promise, reason] {
      if !handle.is_finite()
        || handle < 0.0
        || handle.fract() != 0.0
        || handle > 9_007_199_254_740_991.0
      {
        return Err("invalid retained rejection value handle".into());
      }
    }
    Ok(Self {
      realm_id,
      isolate,
      event,
      promise,
      reason,
      continuation_data: 0,
      realm_owner_identity: 0,
    })
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::collections::VecDeque;
  #[test]
  fn retains_order_and_owner_without_invoking_callbacks() {
    let mut events = VecDeque::new();
    for event in [0.0, 1.0, 2.0, 3.0] {
      events.push_back(
        PendingPromiseRejection::new(7, 11, event, 42.0, 43.0).unwrap(),
      );
    }
    for event in 0..4 {
      let actual = events.pop_front().unwrap();
      assert_eq!(
        (
          actual.realm_id,
          actual.isolate,
          actual.event,
          actual.promise,
          actual.reason
        ),
        (7, 11, event, 42.0, 43.0)
      );
    }
    assert!(events.is_empty());
  }
  #[test]
  fn refuses_unknown_events_and_unrootable_handles() {
    assert!(PendingPromiseRejection::new(0, 11, 0.0, 1.0, 2.0).is_err());
    for event in [-1.0, 4.0, 0.5, f64::NAN, f64::INFINITY] {
      assert!(PendingPromiseRejection::new(7, 11, event, 1.0, 2.0).is_err());
    }
    for handle in [-1.0, 0.5, 9_007_199_254_740_992.0, f64::NAN, f64::INFINITY]
    {
      assert!(PendingPromiseRejection::new(7, 11, 0.0, handle, 2.0).is_err());
      assert!(PendingPromiseRejection::new(7, 11, 0.0, 1.0, handle).is_err());
    }
  }
}
