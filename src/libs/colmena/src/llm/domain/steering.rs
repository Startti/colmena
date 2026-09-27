//! Messages the person writes while an agent works, read by the agent loop
//! at its next step.
//!
//! A run that accepts them has one [`SteeringInbox`] (ADP's worker: one per
//! job, in Redis). The contract the agent loop is built against: between
//! steps it takes what is waiting ([`SteeringInbox::take`]); at a final
//! answer it takes or closes in one operation
//! ([`SteeringInbox::take_or_close`]), so a message arriving at that instant
//! is never lost nor read twice; whatever is left when the loop returns is
//! dropped by [`SteeringInbox::close`], and the client, which still has it,
//! sends it as the next turn. The engine hands the inbox only to the root's
//! `llm_call` ([`in_steering`], [`take_inbox`]): nothing else a run runs
//! reads it.
//!
//! The trait never fails toward the engine: an implementation that cannot
//! reach its store logs and returns nothing, and the loop goes on unread.

use std::collections::VecDeque;
use std::future::Future;
use std::sync::{Arc, Mutex};

/// One message: the client's id (so the client knows which one was read)
/// and its text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SteeringMessage {
    pub id: String,
    pub text: String,
}

/// Whether `id` can travel in a frame: 1 to 256 characters of
/// `[A-Za-z0-9_.-]`, not made only of dots: the ids ADP's worker accepts
/// (`platform_shared::control::valid_id`). The loop skips a message with any
/// other id.
pub fn is_steering_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 256
        && !id.bytes().all(|b| b == b'.')
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
}

/// Where a run's messages wait. See the module docs.
#[async_trait::async_trait]
pub trait SteeringInbox: Send + Sync {
    /// Everything waiting, in the order it arrived. The inbox stays open.
    async fn take(&self) -> Vec<SteeringMessage>;
    /// In one operation: what is waiting (the inbox stays open) or, with
    /// nothing waiting, closes the inbox. A message that arrives after it
    /// closed is refused, so the client sends it as the next turn.
    async fn take_or_close(&self) -> Vec<SteeringMessage>;
    /// Closes the inbox and drops what was left. Closing twice does nothing.
    async fn close(&self);
}

#[derive(Default)]
struct MemState {
    closed: bool,
    waiting: VecDeque<SteeringMessage>,
}

/// An inbox in memory, for tests and anything that runs in one process.
/// [`push`](Self::push) is how a message arrives.
#[derive(Default)]
pub struct InMemorySteeringInbox {
    state: Mutex<MemState>,
}

impl InMemorySteeringInbox {
    pub fn new() -> Self {
        Self::default()
    }

    /// Leaves a message. `false` if the inbox is closed: it was refused.
    pub fn push(&self, id: &str, text: &str) -> bool {
        let mut state = self.state.lock().unwrap();
        if state.closed {
            return false;
        }
        state.waiting.push_back(SteeringMessage {
            id: id.to_string(),
            text: text.to_string(),
        });
        true
    }

    pub fn is_closed(&self) -> bool {
        self.state.lock().unwrap().closed
    }
}

#[async_trait::async_trait]
impl SteeringInbox for InMemorySteeringInbox {
    async fn take(&self) -> Vec<SteeringMessage> {
        let mut state = self.state.lock().unwrap();
        if state.closed {
            return Vec::new();
        }
        state.waiting.drain(..).collect()
    }

    async fn take_or_close(&self) -> Vec<SteeringMessage> {
        let mut state = self.state.lock().unwrap();
        if state.closed {
            return Vec::new();
        }
        if state.waiting.is_empty() {
            state.closed = true;
            return Vec::new();
        }
        state.waiting.drain(..).collect()
    }

    async fn close(&self) {
        let mut state = self.state.lock().unwrap();
        state.closed = true;
        state.waiting.clear();
    }
}

tokio::task_local! {
    /// The inbox of the node this code runs in, until its `llm_call` takes it
    /// ([`take_inbox`]). Empty in any other node, in a node of a nested run
    /// and in a call's work.
    static SLOT: Arc<Mutex<Option<Arc<dyn SteeringInbox>>>>;
}

/// Runs `fut` (a node's execution) with `inbox` in its slot. The engine wraps
/// every node of a run this way: a root run built with an inbox puts it in
/// the slot of its `llm_call` nodes, and every other node and run (a nested
/// one, one without an inbox) puts none, which hides an outer one.
pub async fn in_steering<F: Future>(inbox: Option<Arc<dyn SteeringInbox>>, fut: F) -> F::Output {
    SLOT.scope(Arc::new(Mutex::new(inbox)), fut).await
}

/// Runs a call's work with no inbox in scope: nothing a call runs (a tool
/// that is an `llm_call`, a child run) reads the person's messages.
pub(crate) async fn outside_steering<F: Future>(fut: F) -> F::Output {
    in_steering(None, fut).await
}

/// Takes the inbox of the node this code runs in, once: the `llm_call` that
/// runs the node takes it before anything else, so nothing it runs inside
/// finds it (a pending call it resumes on its own, outside `run_call`).
/// `None` outside a node, or once taken.
pub(crate) fn take_inbox() -> Option<Arc<dyn SteeringInbox>> {
    SLOT.try_with(|slot| slot.lock().unwrap().take())
        .ok()
        .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(messages: &[SteeringMessage]) -> Vec<&str> {
        messages.iter().map(|m| m.id.as_str()).collect()
    }

    #[tokio::test]
    async fn take_hands_over_what_waits_in_order_and_leaves_it_open() {
        let inbox = InMemorySteeringInbox::new();
        assert!(inbox.push("m1", "uno"));
        assert!(inbox.push("m2", "dos"));
        let taken = inbox.take().await;
        assert_eq!(ids(&taken), ["m1", "m2"]);
        assert_eq!(taken[1].text, "dos");
        assert!(inbox.take().await.is_empty(), "taken once");
        assert!(inbox.push("m3", "tres"), "still open");
        assert!(!inbox.is_closed());
    }

    #[tokio::test]
    async fn take_or_close_hands_over_what_waits_or_closes_when_nothing_does() {
        let inbox = InMemorySteeringInbox::new();
        assert!(inbox.push("m1", "uno"));
        assert_eq!(ids(&inbox.take_or_close().await), ["m1"]);
        assert!(!inbox.is_closed(), "it brought something: still open");
        assert!(inbox.take_or_close().await.is_empty());
        assert!(inbox.is_closed(), "nothing waited: closed in the same call");
        assert!(
            !inbox.push("m2", "tarde"),
            "a message after it closed is refused"
        );
        assert!(inbox.take().await.is_empty());
    }

    #[tokio::test]
    async fn close_drops_what_was_left_and_closing_twice_does_nothing() {
        let inbox = InMemorySteeringInbox::new();
        assert!(inbox.push("m1", "uno"));
        inbox.close().await;
        inbox.close().await;
        assert!(inbox.is_closed());
        assert!(inbox.take().await.is_empty());
        assert!(inbox.take_or_close().await.is_empty());
        assert!(!inbox.push("m2", "dos"));
    }

    #[test]
    fn a_steering_id_is_short_and_has_no_separators() {
        for ok in ["m1", "q7Hk2Lp0aZ", "call_3f2a-9c.1"] {
            assert!(is_steering_id(ok), "{ok}");
        }
        assert!(is_steering_id(&"x".repeat(256)));
        let long = "x".repeat(257);
        for bad in [
            "",
            ".",
            "..",
            "a:b",
            "a/b",
            "a b",
            "a\"b",
            "a}b",
            long.as_str(),
        ] {
            assert!(!is_steering_id(bad), "{bad}");
        }
    }

    fn boxed() -> Option<Arc<dyn SteeringInbox>> {
        Some(Arc::new(InMemorySteeringInbox::new()))
    }

    #[tokio::test]
    async fn the_inbox_is_taken_once_inside_its_scope_and_is_not_there_outside() {
        assert!(take_inbox().is_none());
        let (first, second) = in_steering(boxed(), async {
            (take_inbox().is_some(), take_inbox().is_some())
        })
        .await;
        assert!(first, "the first taker gets it");
        assert!(!second, "only once");
    }

    #[tokio::test]
    async fn outside_steering_hides_the_inbox_of_the_scope_around_it() {
        let (inner, after) = in_steering(boxed(), async {
            let inner = outside_steering(async { take_inbox().is_some() }).await;
            (inner, take_inbox().is_some())
        })
        .await;
        assert!(!inner, "a call's work finds none");
        assert!(after, "and the node still has its own");
    }
}
