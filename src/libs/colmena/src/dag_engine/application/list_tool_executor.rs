//! Deterministic iteration engine for `for_each`: runs a dispatch closure over
//! N rows with a policy (error handling + concurrency) and stable ordering.

use serde_json::Value;
use std::future::Future;

pub const DEFAULT_MAX_ITEMS: usize = 1000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnError {
    Continue,
    Abort,
}

#[derive(Debug, Clone, Copy)]
pub struct ExecPolicy {
    pub on_error: OnError,
    pub concurrency: usize,
    pub max_items: usize,
}

impl Default for ExecPolicy {
    fn default() -> Self {
        Self {
            on_error: OnError::Continue,
            concurrency: 1,
            max_items: DEFAULT_MAX_ITEMS,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemStatus {
    Ok,
    Err,
}

#[derive(Debug, Clone)]
pub struct ItemResult {
    pub index: usize,
    pub input: Value,
    pub status: ItemStatus,
    pub output: Option<Value>,
    pub error: Option<String>,
}

/// Run `dispatch` over each row, sequentially when `policy.concurrency <= 1`
/// or bounded-concurrently otherwise. `Continue` collects every row's
/// result; `Abort` stops after the first error. Results are always
/// index-ordered regardless of completion order.
///
/// Note: `Abort` under concurrency is best-effort — in-flight items complete;
/// no NEW items start after an error is observed. Strict cancellation is
/// deferred to the backlog.
pub async fn run_list<F, Fut>(rows: Vec<Value>, policy: &ExecPolicy, dispatch: F) -> Vec<ItemResult>
where
    F: Fn(usize, Value) -> Fut,
    Fut: Future<Output = Result<Value, String>>,
{
    if policy.concurrency <= 1 {
        return run_sequential(rows, policy, dispatch).await;
    }
    use futures::stream::{self, StreamExt};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    // Best-effort Abort under concurrency: a shared flag is set when a row
    // errors under `OnError::Abort`. Items already pulled into the
    // `buffer_unordered` window still complete (no strict cancellation), but
    // any item not yet started is short-circuited — no NEW dispatch runs after
    // an error is observed.
    let aborted = Arc::new(AtomicBool::new(false));
    let abort_on_err = policy.on_error == OnError::Abort;
    let dispatch = &dispatch;
    let mut results: Vec<ItemResult> = stream::iter(rows.into_iter().enumerate())
        .map(|(index, row)| {
            let aborted = Arc::clone(&aborted);
            async move {
                if abort_on_err && aborted.load(Ordering::SeqCst) {
                    return ItemResult {
                        index,
                        input: row,
                        status: ItemStatus::Err,
                        output: None,
                        error: Some("skipped: batch aborted".to_string()),
                    };
                }
                match dispatch(index, row.clone()).await {
                    Ok(output) => ItemResult {
                        index,
                        input: row,
                        status: ItemStatus::Ok,
                        output: Some(output),
                        error: None,
                    },
                    Err(error) => {
                        if abort_on_err {
                            aborted.store(true, Ordering::SeqCst);
                        }
                        ItemResult {
                            index,
                            input: row,
                            status: ItemStatus::Err,
                            output: None,
                            error: Some(error),
                        }
                    }
                }
            }
        })
        .buffer_unordered(policy.concurrency)
        .collect()
        .await;
    results.sort_by_key(|r| r.index);
    results
}

async fn run_sequential<F, Fut>(
    rows: Vec<Value>,
    policy: &ExecPolicy,
    dispatch: F,
) -> Vec<ItemResult>
where
    F: Fn(usize, Value) -> Fut,
    Fut: Future<Output = Result<Value, String>>,
{
    let mut results = Vec::with_capacity(rows.len());
    for (index, row) in rows.into_iter().enumerate() {
        let item = match dispatch(index, row.clone()).await {
            Ok(output) => ItemResult {
                index,
                input: row,
                status: ItemStatus::Ok,
                output: Some(output),
                error: None,
            },
            Err(error) => ItemResult {
                index,
                input: row,
                status: ItemStatus::Err,
                output: None,
                error: Some(error),
            },
        };
        let is_err = item.status == ItemStatus::Err;
        results.push(item);
        if is_err && policy.on_error == OnError::Abort {
            break;
        }
    }
    results
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn continue_collects_ok_and_err_in_order() {
        let rows = vec![json!({"n":1}), json!({"n":2}), json!({"n":3})];
        let policy = ExecPolicy {
            on_error: OnError::Continue,
            concurrency: 1,
            max_items: DEFAULT_MAX_ITEMS,
        };
        let out = run_list(rows, &policy, |_i, row| async move {
            let n = row["n"].as_i64().unwrap();
            if n == 2 {
                Err("boom".into())
            } else {
                Ok(json!({"double": n * 2}))
            }
        })
        .await;
        assert_eq!(out.len(), 3);
        assert_eq!(out[0].index, 0);
        assert_eq!(out[0].status, ItemStatus::Ok);
        assert_eq!(out[1].status, ItemStatus::Err);
        assert_eq!(out[1].error.as_deref(), Some("boom"));
        assert_eq!(out[2].output.as_ref().unwrap(), &json!({"double": 6}));
    }

    #[tokio::test]
    async fn abort_stops_after_first_error() {
        let rows = vec![json!({"n":1}), json!({"n":2}), json!({"n":3})];
        let policy = ExecPolicy {
            on_error: OnError::Abort,
            concurrency: 1,
            max_items: DEFAULT_MAX_ITEMS,
        };
        let out = run_list(rows, &policy, |_i, row| async move {
            let n = row["n"].as_i64().unwrap();
            if n == 2 {
                Err("stop".into())
            } else {
                Ok(json!(n))
            }
        })
        .await;
        assert_eq!(out.len(), 2); // item 3 never ran
        assert_eq!(out[1].status, ItemStatus::Err);
    }

    #[tokio::test]
    async fn parallel_preserves_index_order() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;
        let rows: Vec<Value> = (0..10).map(|i| json!({"n": i})).collect();
        let policy = ExecPolicy {
            on_error: OnError::Continue,
            concurrency: 4,
            max_items: DEFAULT_MAX_ITEMS,
        };
        // Track peak in-flight to prove real concurrency (a sequential loop
        // would keep peak == 1, so this fails if buffer_unordered regresses).
        let in_flight = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let out = run_list(rows, &policy, |_i, row| {
            let in_flight = Arc::clone(&in_flight);
            let peak = Arc::clone(&peak);
            async move {
                let cur = in_flight.fetch_add(1, Ordering::SeqCst) + 1;
                peak.fetch_max(cur, Ordering::SeqCst);
                let n = row["n"].as_i64().unwrap();
                // Later items sleep less; without re-sort they'd finish out of order.
                tokio::time::sleep(std::time::Duration::from_millis((10 - n) as u64)).await;
                in_flight.fetch_sub(1, Ordering::SeqCst);
                Ok(json!(n))
            }
        })
        .await;
        for (i, item) in out.iter().enumerate() {
            assert_eq!(item.index, i);
            assert_eq!(item.output.as_ref().unwrap(), &json!(i as i64));
        }
        assert!(
            peak.load(Ordering::SeqCst) > 1,
            "expected concurrent execution, peak in-flight was {}",
            peak.load(Ordering::SeqCst)
        );
    }

    #[tokio::test]
    async fn abort_best_effort_under_concurrency() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;
        // 20 rows; item 0 fails immediately. Under Abort + concurrency, in-flight
        // items may finish but no NEW items should start after the error is seen.
        let rows: Vec<Value> = (0..20).map(|i| json!({"n": i})).collect();
        let policy = ExecPolicy {
            on_error: OnError::Abort,
            concurrency: 2,
            max_items: DEFAULT_MAX_ITEMS,
        };
        let executed = Arc::new(AtomicUsize::new(0));
        let out = run_list(rows, &policy, |_i, row| {
            let executed = Arc::clone(&executed);
            async move {
                executed.fetch_add(1, Ordering::SeqCst);
                let n = row["n"].as_i64().unwrap();
                if n == 0 {
                    Err("early failure".into())
                } else {
                    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                    Ok(json!(n))
                }
            }
        })
        .await;
        let ran = executed.load(Ordering::SeqCst);
        assert!(
            ran < 20,
            "Abort should skip items; all {ran} ran (no-op abort)"
        );
        assert_eq!(out.len(), 20, "one result entry per row expected");
        assert_eq!(out[0].status, ItemStatus::Err);
        assert!(
            out.iter()
                .any(|r| r.error.as_deref() == Some("skipped: batch aborted")),
            "expected at least one short-circuited item"
        );
    }
}
