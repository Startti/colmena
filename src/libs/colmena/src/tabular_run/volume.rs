//! A call's staged volume held so that it is given back on EVERY path: normal
//! end, error, panic, or the call's future being dropped. Releasing unmounts the
//! output volume and removes up to a gibibyte of staged data, so it is handed to
//! the blocking pool and never runs on an async worker, whichever path reaches it.

use crate::dag_engine::infrastructure::python_exec::staging::StagedCall;

pub struct Volume(Option<StagedCall>);

impl Volume {
    pub fn new(call: StagedCall) -> Self {
        Self(Some(call))
    }

    pub fn get(&self) -> &StagedCall {
        self.0
            .as_ref()
            .expect("a volume is held until it is released")
    }

    /// Gives the volume back and returns once it is (the budget share with it).
    pub async fn release(mut self) {
        if let Some(call) = self.0.take() {
            let _ = tokio::task::spawn_blocking(move || drop(call)).await;
        }
    }
}

impl Drop for Volume {
    fn drop(&mut self) {
        let Some(call) = self.0.take() else { return };
        match tokio::runtime::Handle::try_current() {
            Ok(rt) => {
                rt.spawn_blocking(move || drop(call));
            }
            Err(_) => drop(call),
        }
    }
}
