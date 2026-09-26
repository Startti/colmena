//! How one model response's tool calls are batched for execution.
//!
//! Pure, no I/O. In the model's order, a call without a chain key runs alone
//! (a barrier); consecutive calls that carry a chain key form a group whose
//! chains (same key → same chain, in model order) run concurrently in a
//! later task. This module only decides the plan — it does not execute it.

/// How one model response's tool calls run: in the model's order, a call
/// without a chain key alone, consecutive keyed calls together as a group whose
/// chains (same key → same chain, in order) run concurrently.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Batch {
    Alone(usize),
    Group(Vec<Vec<usize>>),
}

pub fn plan_batches(keys: &[Option<String>]) -> Vec<Batch> {
    let mut out = Vec::new();
    let mut group: Vec<(String, Vec<usize>)> = Vec::new();
    let flush = |group: &mut Vec<(String, Vec<usize>)>, out: &mut Vec<Batch>| {
        if !group.is_empty() {
            out.push(Batch::Group(group.drain(..).map(|(_, c)| c).collect()));
        }
    };
    for (i, key) in keys.iter().enumerate() {
        match key {
            None => {
                flush(&mut group, &mut out);
                out.push(Batch::Alone(i));
            }
            Some(k) => match group.iter_mut().find(|(gk, _)| gk == k) {
                Some((_, chain)) => chain.push(i),
                None => group.push((k.clone(), vec![i])),
            },
        }
    }
    flush(&mut group, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(s: &str) -> Option<String> {
        Some(s.to_string())
    }

    #[test]
    fn barrier_before_and_after_a_keyed_group() {
        let keys = vec![None, key("a"), key("b"), None];
        assert_eq!(
            plan_batches(&keys),
            vec![
                Batch::Alone(0),
                Batch::Group(vec![vec![1], vec![2]]),
                Batch::Alone(3),
            ]
        );
    }

    #[test]
    fn repeated_key_extends_its_chain_in_model_order() {
        let keys = vec![key("a"), key("a"), key("b")];
        assert_eq!(
            plan_batches(&keys),
            vec![Batch::Group(vec![vec![0, 1], vec![2]])]
        );
    }

    #[test]
    fn empty_input_yields_empty_plan() {
        let keys: Vec<Option<String>> = vec![];
        assert_eq!(plan_batches(&keys), Vec::<Batch>::new());
    }

    #[test]
    fn all_unkeyed_calls_are_each_alone() {
        let keys = vec![None, None, None];
        assert_eq!(
            plan_batches(&keys),
            vec![Batch::Alone(0), Batch::Alone(1), Batch::Alone(2)]
        );
    }
}
