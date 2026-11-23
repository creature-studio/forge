//! Non-blocking collection of completed jobs.
//!
//! This layer owns no scheduler, cancellation policy, or freshness checks.
//! It only returns the request context alongside the result, *removing* the
//! pending entry atomically once the job has completed. Callers remain
//! responsible for stale-result validation and for applying the result.

/// Take a completed request without cloning its metadata or changing the
/// pending slot when the job is still running.
pub(super) fn take_ready<P, R>(
    slot: &mut Option<P>,
    try_result: impl FnOnce(&mut P) -> Option<R>,
) -> Option<(P, R)> {
    let result = try_result(slot.as_mut()?)?;
    Some((
        slot.take().expect("completed request remains in slot"),
        result,
    ))
}

/// Collect completed entries from a collection, keeping pending entries and
/// their relative order. Does not apply results while mutating the collection.
pub(super) fn drain_ready<P, R>(
    pending: &mut Vec<P>,
    mut try_result: impl FnMut(&mut P) -> Option<R>,
) -> Vec<(P, R)> {
    let mut finished = Vec::new();
    let mut index = 0;
    while index < pending.len() {
        if let Some(result) = try_result(&mut pending[index]) {
            finished.push((pending.remove(index), result));
        } else {
            index += 1;
        }
    }
    finished
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, PartialEq, Eq)]
    struct Request {
        key: &'static str,
        done: bool,
    }

    #[test]
    fn single_slot_keeps_running_request_and_takes_ready_once() {
        let mut request = Some(Request {
            key: "latest",
            done: false,
        });
        assert!(take_ready(&mut request, |p| p.done.then_some(42)).is_none());
        assert_eq!(request.as_ref().unwrap().key, "latest");
        request.as_mut().unwrap().done = true;
        let (context, result) = take_ready(&mut request, |p| p.done.then_some(42)).unwrap();
        assert_eq!(context.key, "latest");
        assert_eq!(result, 42);
        assert!(request.is_none());
        assert!(take_ready(&mut request, |_| Some(1)).is_none());
    }

    #[test]
    fn vec_collects_only_completed_requests_in_order() {
        let mut pending = vec![
            Request {
                key: "old",
                done: true,
            },
            Request {
                key: "running",
                done: false,
            },
            Request {
                key: "new",
                done: true,
            },
        ];
        let done = drain_ready(&mut pending, |p| p.done.then_some(p.key));
        assert_eq!(
            done.iter().map(|(p, _)| p.key).collect::<Vec<_>>(),
            vec!["old", "new"]
        );
        assert_eq!(
            done.iter().map(|(_, value)| *value).collect::<Vec<_>>(),
            vec!["old", "new"]
        );
        assert_eq!(
            pending,
            vec![Request {
                key: "running",
                done: false
            }]
        );
    }
}
