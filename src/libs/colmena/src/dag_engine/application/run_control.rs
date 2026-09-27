//! What a live run can be told while it runs: stop the whole turn, or stop one
//! tool call and let the rest of the turn go on.

use crate::llm::domain::call_cancels::CallCancels;
use crate::llm::domain::steering::SteeringInbox;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

/// The controls of one run (a job, a turn), shared with whatever receives
/// commands for it (ADP's worker: a Redis subscriber per job). Hand it to
/// [`ColmenaEngine::execute_stream_controlled`](crate::dag_engine::engine::ColmenaEngine::execute_stream_controlled).
#[derive(Clone)]
pub struct RunControl {
    cancel: CancellationToken,
    calls: Arc<CallCancels>,
    steering: Option<Arc<dyn SteeringInbox>>,
}

impl RunControl {
    /// The controls around the turn's `cancel` token. Cancelling that token
    /// stops the whole run, as with `execute_stream_cancellable`.
    pub fn new(cancel: CancellationToken) -> Self {
        let calls = Arc::new(CallCancels::new(cancel.clone()));
        Self {
            cancel,
            calls,
            steering: None,
        }
    }

    /// The turn's token: `cancel_token().cancel()` is the composer's Stop.
    pub fn cancel_token(&self) -> &CancellationToken {
        &self.cancel
    }

    /// Stops one tool call of the run, at any depth, by its `tool_call_id`.
    /// `false` if that call already ended. `true` says the request was
    /// delivered or remembered, not that the call will be answered as
    /// cancelled: every case is in [`CallCancels::cancel`].
    pub fn cancel_call(&self, tool_call_id: &str) -> bool {
        self.calls.cancel(tool_call_id)
    }

    /// The run also reads, between steps of its root's agent, what the person
    /// writes while it works: `inbox` is where those messages wait
    /// (`llm::domain::steering`). Only the root's `llm_call` reads it.
    pub fn with_steering(mut self, inbox: Arc<dyn SteeringInbox>) -> Self {
        self.steering = Some(inbox);
        self
    }

    pub(crate) fn steering(&self) -> Option<Arc<dyn SteeringInbox>> {
        self.steering.clone()
    }

    pub(crate) fn calls(&self) -> Arc<CallCancels> {
        self.calls.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_control_cancels_one_call_or_the_whole_turn() {
        let control = RunControl::new(CancellationToken::new());
        let calls = control.calls();
        let running = calls.begin("c1").unwrap();
        assert!(control.cancel_call("c2"), "not started: remembered");
        assert!(calls.begin("c2").is_none());
        assert!(!running.is_cancelled());
        control.cancel_token().cancel();
        assert!(running.is_cancelled(), "the turn's Stop reaches every call");
    }

    #[test]
    fn a_control_carries_an_inbox_only_when_given_one() {
        use crate::llm::domain::steering::InMemorySteeringInbox;
        let plain = RunControl::new(CancellationToken::new());
        assert!(plain.steering().is_none());
        let steered = plain
            .clone()
            .with_steering(Arc::new(InMemorySteeringInbox::new()));
        assert!(steered.steering().is_some());
        assert!(
            plain.steering().is_none(),
            "a clone taken before is untouched"
        );
    }
}
