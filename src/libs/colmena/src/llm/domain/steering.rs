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
//! sends it as the next turn. This module only defines the inbox: the loop,
//! and the engine that gives the root's `llm_call` its inbox, come in the
//! next changes of this series.
//!
//! The trait never fails toward the engine: an implementation that cannot
//! reach its store logs and returns nothing, and the loop goes on unread.

use std::collections::VecDeque;
use std::sync::Mutex;

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
}
