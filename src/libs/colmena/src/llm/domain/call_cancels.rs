//! Cancelling one tool call on its own, while the rest of the turn runs.
//!
//! A run (one turn) can have one [`CallCancels`]: a token per tool call, each
//! a child of the turn's token, plus the ids asked to stop before their call
//! started. Whoever runs the turn puts it in scope ([`in_registry`]); the
//! agent loop's `run_call` registers each call in it and answers a call the
//! person cancelled with [`CANCELLED_BY_PERSON_TEXT`]. Without one in scope,
//! every call runs as before.
//!
//! A node that runs its call's work as a run of its own (a `subgraph` used as
//! a tool) takes the call's token with [`adopt_current_call`]: it stops by
//! itself (closing its row CANCELLED), and the loop waits for it instead of
//! dropping it.

use crate::llm::domain::ToolResult;
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tokio_util::sync::CancellationToken;

/// Stable code of a call the person cancelled: the `error` of its
/// [`ToolResult`], and the prefix of `DagError::Cancelled`, which closes what
/// the cancel cut inside a child run. A consumer tells a cancel from a failure
/// by it.
pub const CALL_CANCELLED_CODE: &str = "CANCELLED_BY_PERSON";

/// What the model reads for a call the person cancelled before it finished.
/// What the call already did (notes saved, requests sent) stays done: the text
/// does not promise to undo it.
pub const CANCELLED_BY_PERSON_TEXT: &str =
    include_str!("../../../text/prompts/agent_loop/cancelled_by_person.md");

tokio::task_local! {
    /// The per-call cancels of the run this code runs in.
    static REGISTRY: Arc<CallCancels>;
    /// The call whose work is running now (set by [`run_as_call`]).
    static CURRENT: CurrentCall;
}

struct CurrentCall {
    token: CancellationToken,
    adopted: Arc<AtomicBool>,
}

/// How many cancels of calls that have not started a registry remembers
/// (see [`CallCancels::cancel`]). A turn's real calls stay far below it.
const MAX_EARLY_CANCELS: usize = 1024;

#[derive(Default)]
struct State {
    running: HashMap<String, Running>,
    requested: HashSet<String>,
    ended: HashSet<String>,
}

/// A running id: the token of the first call that began with it, and how
/// many calls with it are running (more than one only with a repeated id).
struct Running {
    token: CancellationToken,
    calls: usize,
}

/// The per-call cancels of one run. See the module docs.
pub struct CallCancels {
    turn: CancellationToken,
    state: Mutex<State>,
}

impl CallCancels {
    /// The registry of a run whose whole-turn token is `turn`. Every call's
    /// token is a child of it, so stopping the turn stops every call.
    ///
    /// `turn` must be the token that tears the run down (the one the engine
    /// is given). On a turn Stop a running call is never answered: it waits
    /// for the run to be dropped. With any other token, stopping it leaves
    /// every running call, and the future run with [`in_registry`], pending
    /// forever.
    pub fn new(turn: CancellationToken) -> Self {
        Self {
            turn,
            state: Mutex::new(State::default()),
        }
    }

    /// Asks to stop the call `tool_call_id`. A running call has its token
    /// fired. A call that already ended (answered, or paused on a question)
    /// is left alone: `false`. An id not seen yet is remembered, because a
    /// cancel can arrive before its call starts (queued behind the group's
    /// limit, or still being streamed by the model): a call with that id that
    /// starts later does not run. At most 1024 such ids are remembered: past
    /// that a new one is refused (`false`), and the ones already remembered
    /// stay, so a client that cancels ids no call has cannot grow the
    /// registry without bound. An empty id names no call: `false`.
    ///
    /// `true` says the request was delivered or remembered, not that the call
    /// will be answered as cancelled. The call may finish first (a success
    /// keeps its result), have no work to stop (a refused or redirected
    /// call), or never begin (an id of an earlier turn, or a repeat the loop
    /// answers without running it). The call's Finish frame is what confirms.
    pub fn cancel(&self, tool_call_id: &str) -> bool {
        if tool_call_id.is_empty() {
            return false;
        }
        let mut state = self.state.lock().unwrap();
        if let Some(running) = state.running.get(tool_call_id) {
            running.token.cancel();
            return true;
        }
        if state.ended.contains(tool_call_id) {
            return false;
        }
        if state.requested.contains(tool_call_id) {
            return true;
        }
        if state.requested.len() >= MAX_EARLY_CANCELS {
            return false;
        }
        state.requested.insert(tool_call_id.to_string());
        true
    }

    /// A call starts: its token, a child of the turn's, or `None` when the
    /// person cancelled it before it started (the call must not run; it
    /// counts as ended).
    ///
    /// Ids are meant to be unique in a run, and real providers send unique
    /// ones. A call whose id is already running (a scripted model that
    /// repeats one, or a provider that sends none, which gives `""`) still
    /// runs, under a fresh child token that is not registered: a cancel by id
    /// never reaches it, only a turn Stop does. The first call keeps the id,
    /// which stays running until every call that began with it
    /// [`end`](Self::end)ed. A cancel never names `""` (see
    /// [`cancel`](Self::cancel)), so a call without an id is never cancelled
    /// by id either.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn begin(&self, tool_call_id: &str) -> Option<CancellationToken> {
        let mut state = self.state.lock().unwrap();
        if state.requested.remove(tool_call_id) {
            state.ended.insert(tool_call_id.to_string());
            return None;
        }
        let token = self.turn.child_token();
        match state.running.get_mut(tool_call_id) {
            Some(running) => running.calls += 1,
            None => {
                let running = Running {
                    token: token.clone(),
                    calls: 1,
                };
                state.running.insert(tool_call_id.to_string(), running);
            }
        }
        Some(token)
    }

    /// A call ended, whatever its outcome: once every call that began with
    /// its id ended, a later cancel is a no-op.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn end(&self, tool_call_id: &str) {
        let mut state = self.state.lock().unwrap();
        if let Some(running) = state.running.get_mut(tool_call_id) {
            running.calls -= 1;
            if running.calls > 0 {
                return;
            }
            state.running.remove(tool_call_id);
        }
        state.ended.insert(tool_call_id.to_string());
    }

    /// The whole turn was stopped (the composer's Stop).
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn turn_stopped(&self) -> bool {
        self.turn.is_cancelled()
    }
}

/// The answer of a call the person cancelled: not a success, its `error` is
/// [`CALL_CANCELLED_CODE`] and its output the text the model reads.
pub fn cancelled_result(tool_call_id: &str) -> ToolResult {
    ToolResult {
        tool_call_id: tool_call_id.to_string(),
        success: false,
        output: CANCELLED_BY_PERSON_TEXT.trim().to_string(),
        error: Some(CALL_CANCELLED_CODE.to_string()),
    }
}

/// Whether `result` is the answer of a call the person cancelled.
pub fn is_cancelled_result(result: &ToolResult) -> bool {
    !result.success && result.error.as_deref() == Some(CALL_CANCELLED_CODE)
}

/// Runs `fut` with `calls` as the run's registry; without one, as is. `calls`
/// must be built on the run's own turn token (see [`CallCancels::new`]).
pub async fn in_registry<F: Future>(calls: Option<Arc<CallCancels>>, fut: F) -> F::Output {
    match calls {
        Some(calls) => REGISTRY.scope(calls, fut).await,
        None => fut.await,
    }
}

/// The registry of the run this code runs in, if it has one.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn current_registry() -> Option<Arc<CallCancels>> {
    REGISTRY.try_with(Arc::clone).ok()
}

/// For a node that runs its call's work as a run of its own: the token of the
/// call running now, with the promise that the node stops by itself when it
/// fires (so the loop waits for it instead of dropping it). `None` outside a
/// call.
pub fn adopt_current_call() -> Option<CancellationToken> {
    CURRENT
        .try_with(|call| {
            call.adopted.store(true, Ordering::SeqCst);
            call.token.clone()
        })
        .ok()
}

/// Runs a call's work `fut` under its `token`. Once the whole turn was
/// stopped it never returns, whatever the work did (even if it finished): the
/// run being torn down drops it, as before per-call cancels existed, so
/// nothing answers the call. Otherwise, when the token fires:
/// - the work adopted the token ([`adopt_current_call`]): waits for it to stop
///   by itself, and returns what it returned;
/// - otherwise: drops the work and returns `None`.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) async fn run_as_call<F: Future>(
    calls: &CallCancels,
    token: &CancellationToken,
    fut: F,
) -> Option<F::Output> {
    let adopted = Arc::new(AtomicBool::new(false));
    let current = CurrentCall {
        token: token.clone(),
        adopted: adopted.clone(),
    };
    let fut = CURRENT.scope(current, fut);
    tokio::pin!(fut);
    let out = tokio::select! {
        biased;
        out = &mut fut => out,
        _ = token.cancelled() => {
            if calls.turn_stopped() {
                return std::future::pending().await;
            }
            if !adopted.load(Ordering::SeqCst) {
                return None;
            }
            fut.await
        }
    };
    // Whichever arm won: work that watches its call's token can finish in the
    // very poll that saw the Stop, or while it winds down after a cancel of
    // its own call.
    if calls.turn_stopped() {
        return std::future::pending().await;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn registry() -> (CancellationToken, CallCancels) {
        let turn = CancellationToken::new();
        (turn.clone(), CallCancels::new(turn))
    }

    #[test]
    fn a_call_cancelled_before_it_starts_does_not_start() {
        let (_, calls) = registry();
        assert!(calls.cancel("c1"), "an id not seen yet is remembered");
        assert!(calls.begin("c1").is_none(), "it must not run");
        assert!(!calls.cancel("c1"), "it counts as ended");
    }

    #[test]
    fn cancelling_a_running_call_fires_only_its_token() {
        let (_, calls) = registry();
        let a = calls.begin("a").unwrap();
        let b = calls.begin("b").unwrap();
        assert!(calls.cancel("a"));
        assert!(a.is_cancelled());
        assert!(!b.is_cancelled());
    }

    #[test]
    fn cancelling_a_call_that_ended_is_a_no_op() {
        let (_, calls) = registry();
        let a = calls.begin("a").unwrap();
        calls.end("a");
        assert!(!calls.cancel("a"));
        assert!(!a.is_cancelled());
    }

    #[test]
    fn stopping_the_turn_fires_every_calls_token() {
        let (turn, calls) = registry();
        let a = calls.begin("a").unwrap();
        assert!(!calls.turn_stopped());
        turn.cancel();
        assert!(a.is_cancelled());
        assert!(calls.turn_stopped());
    }

    /// Real providers send unique ids; a scripted model can repeat one.
    #[test]
    fn a_second_call_with_a_running_id_does_not_take_over_its_cancel() {
        let (turn, calls) = registry();
        let first = calls.begin("a").unwrap();
        let second = calls.begin("a").expect("it still runs");
        assert!(calls.cancel("a"));
        assert!(first.is_cancelled(), "the cancel reaches the first call");
        assert!(
            !second.is_cancelled(),
            "the second is not cancellable by id"
        );
        calls.end("a");
        assert!(
            calls.cancel("a"),
            "the id is live while a call with it runs"
        );
        calls.end("a");
        assert!(!calls.cancel("a"), "both ended");
        turn.cancel();
        assert!(second.is_cancelled(), "a turn Stop still stops it");
    }

    /// An OpenAI-compatible provider that sends no id gives `""`.
    #[test]
    fn a_call_without_an_id_runs_and_only_a_turn_stop_stops_it() {
        let (turn, calls) = registry();
        assert!(!calls.cancel(""), "an empty id names no call");
        let a = calls.begin("").expect("it runs");
        let b = calls.begin("").expect("so does another one");
        assert!(!calls.cancel(""));
        calls.end("");
        assert!(!a.is_cancelled() && !b.is_cancelled());
        turn.cancel();
        assert!(a.is_cancelled() && b.is_cancelled());
    }

    /// A client that cancels ids no call has cannot grow the registry.
    #[test]
    fn past_the_cap_a_cancel_of_a_call_not_started_is_refused() {
        let (_, calls) = registry();
        for i in 0..MAX_EARLY_CANCELS {
            assert!(calls.cancel(&format!("early {i}")));
        }
        assert!(calls.cancel("early 0"), "one already remembered still is");
        assert!(!calls.cancel("one too many"), "a new one is refused");
        assert!(calls.begin("one too many").is_some(), "and not remembered");
        assert!(calls.cancel("one too many"), "a running call is cancelled");
        assert!(calls.begin("early 0").is_none(), "the remembered ones hold");
    }

    #[test]
    fn the_cancelled_answer_is_recognized_and_a_failure_is_not() {
        let r = cancelled_result("c1");
        assert!(is_cancelled_result(&r));
        assert!(!r.success);
        assert_eq!(r.tool_call_id, "c1");
        assert_eq!(
            r.output,
            "La persona canceló este agente antes de que terminara."
        );
        assert!(!is_cancelled_result(&ToolResult::failure(
            "c1".into(),
            "boom".into()
        )));
        // The worker API ends a stream on these substrings (stream.rs:77).
        assert!(!r.output.contains(r#""type":"finish""#));
        assert!(!r.output.contains(r#""type":"error""#));
    }

    #[tokio::test]
    async fn the_registry_is_in_scope_only_inside_in_registry() {
        let (_, calls) = registry();
        assert!(current_registry().is_none());
        let inside = in_registry(Some(Arc::new(calls)), async {
            current_registry().is_some()
        })
        .await;
        assert!(inside);
        assert!(in_registry(None, async { current_registry().is_none() }).await);
    }

    #[test]
    fn outside_a_call_there_is_nothing_to_adopt() {
        assert!(adopt_current_call().is_none());
    }

    /// Sets its flag when dropped: tells whether a call's work was dropped.
    struct DropFlag(Arc<AtomicBool>);
    impl Drop for DropFlag {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }

    fn cancel_in(token: &CancellationToken, ms: u64) {
        let token = token.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(ms)).await;
            token.cancel();
        });
    }

    #[tokio::test(start_paused = true)]
    async fn work_nobody_adopted_is_dropped_when_its_call_is_cancelled() {
        let (_, calls) = registry();
        let token = calls.begin("a").unwrap();
        let dropped = Arc::new(AtomicBool::new(false));
        let guard = DropFlag(dropped.clone());
        let work = async move {
            let _guard = guard;
            tokio::time::sleep(Duration::from_secs(60)).await;
            "finished"
        };
        cancel_in(&token, 5);
        assert_eq!(run_as_call(&calls, &token, work).await, None);
        assert!(dropped.load(Ordering::SeqCst));
    }

    #[tokio::test(start_paused = true)]
    async fn adopted_work_is_waited_for_until_it_stops_by_itself() {
        let (_, calls) = registry();
        let token = calls.begin("a").unwrap();
        let work = async {
            let mine = adopt_current_call().expect("runs as a call");
            mine.cancelled().await;
            // A child run closes its row before it returns.
            tokio::time::sleep(Duration::from_millis(20)).await;
            "stopped by itself"
        };
        cancel_in(&token, 5);
        assert_eq!(
            run_as_call(&calls, &token, work).await,
            Some("stopped by itself")
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_turn_stop_never_answers_the_call() {
        let (turn, calls) = registry();
        let token = calls.begin("a").unwrap();
        cancel_in(&turn, 5);
        let work = tokio::time::sleep(Duration::from_secs(60));
        let out =
            tokio::time::timeout(Duration::from_secs(1), run_as_call(&calls, &token, work)).await;
        assert!(
            out.is_err(),
            "a turn Stop is not answered: the run is torn down"
        );
    }

    /// Work that watches its call's token can finish in the very poll that
    /// saw the Stop, before the token's own arm runs.
    #[tokio::test(start_paused = true)]
    async fn a_turn_stop_is_not_answered_even_when_adopted_work_stops_at_once() {
        let (turn, calls) = registry();
        let token = calls.begin("a").unwrap();
        let finished = Arc::new(AtomicBool::new(false));
        let f = finished.clone();
        let work = async move {
            adopt_current_call()
                .expect("runs as a call")
                .cancelled()
                .await;
            f.store(true, Ordering::SeqCst);
            "stopped at once"
        };
        cancel_in(&turn, 5);
        let out =
            tokio::time::timeout(Duration::from_secs(1), run_as_call(&calls, &token, work)).await;
        assert!(finished.load(Ordering::SeqCst), "the work saw the Stop");
        assert!(out.is_err(), "a turn Stop is not answered");
    }
}
