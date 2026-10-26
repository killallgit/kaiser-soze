use super::{Edge, EdgeKind};
use std::collections::{HashMap, VecDeque};

/// How many calls are remembered as open before the oldest is forgotten.
pub const CALL_BOUND: usize = 4096;

/// Remembers the tool calls a reply made until a request carries their result.
#[derive(Default)]
pub struct Calls {
    open: HashMap<(String, String), u64>,
    order: VecDeque<((String, String), u64)>,
    next: u64,
}

impl Calls {
    /// The `calls` edges one step adds: one for each result in its request that answers an open
    /// call of the thread, which then closes, so a result resent in a later request adds nothing.
    /// The step's own calls are remembered after its results, as the request comes first.
    pub fn edges(&mut self, thread: &str, results: &[String], calls: &[String]) -> Vec<Edge> {
        let mut edges = Vec::new();
        for id in results.iter().filter(|id| !id.is_empty()) {
            if self.open.remove(&(thread.to_string(), id.clone())).is_some() {
                edges.push(Edge::new(EdgeKind::Calls, format!("call:{id}"), format!("result:{id}")));
            }
        }
        for id in calls.iter().filter(|id| !id.is_empty()) {
            self.remember((thread.to_string(), id.clone()));
        }
        edges
    }

    fn remember(&mut self, key: (String, String)) {
        self.next += 1;
        self.open.insert(key.clone(), self.next);
        self.order.push_back((key, self.next));
        while self.open.len() > CALL_BOUND {
            let Some((oldest, seq)) = self.order.pop_front() else {
                break;
            };
            if self.open.get(&oldest) == Some(&seq) {
                self.open.remove(&oldest);
            }
        }
        if self.order.len() > 2 * CALL_BOUND {
            let open = &self.open;
            self.order.retain(|(key, seq)| open.get(key) == Some(seq));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(ids: &[&str]) -> Vec<String> {
        ids.iter().map(|id| id.to_string()).collect()
    }

    fn pairs(edges: &[Edge]) -> Vec<(&str, &str)> {
        edges.iter().map(|e| (e.from.as_str(), e.to.as_str())).collect()
    }

    #[test]
    fn a_result_in_a_later_request_gives_one_edge_from_its_call() {
        // The reply makes the call, the next request carries the result
        let mut calls = Calls::default();

        let reply = calls.edges("t1", &[], &ids(&["c1"]));
        let next = calls.edges("t1", &ids(&["c1"]), &[]);

        assert!(reply.is_empty());
        assert_eq!(pairs(&next), [("call:c1", "result:c1")]);
        assert_eq!(next[0].kind, EdgeKind::Calls);
    }

    #[test]
    fn a_result_resent_in_a_later_request_gives_no_second_edge() {
        // Every request resends the whole conversation, so old results come again
        let mut calls = Calls::default();
        calls.edges("t1", &[], &ids(&["c1"]));
        calls.edges("t1", &ids(&["c1"]), &[]);

        assert!(calls.edges("t1", &ids(&["c1"]), &[]).is_empty());
    }

    #[test]
    fn a_result_or_a_call_with_no_match_gives_no_edge() {
        // An unknown result is ignored, and a call that never gets a result adds nothing
        let mut calls = Calls::default();

        assert!(calls.edges("t1", &ids(&["gone"]), &ids(&["c1"])).is_empty());
    }

    #[test]
    fn an_empty_id_is_ignored() {
        // An empty id names nothing, so it cannot pair unrelated calls and results
        let mut calls = Calls::default();
        calls.edges("t1", &[], &ids(&[""]));

        assert!(calls.edges("t1", &ids(&[""]), &[]).is_empty());
    }

    #[test]
    fn a_result_matches_only_a_call_of_its_own_thread() {
        // Another thread may have used the same call id
        let mut calls = Calls::default();
        calls.edges("t1", &[], &ids(&["c1"]));

        assert!(calls.edges("t2", &ids(&["c1"]), &[]).is_empty());
        assert_eq!(calls.edges("t1", &ids(&["c1"]), &[]).len(), 1);
    }

    #[test]
    fn several_results_in_one_request_each_give_an_edge() {
        // A reply may call several tools at once
        let mut calls = Calls::default();
        calls.edges("t1", &[], &ids(&["c1", "c2"]));

        let edges = calls.edges("t1", &ids(&["c1", "c2"]), &[]);

        assert_eq!(pairs(&edges), [("call:c1", "result:c1"), ("call:c2", "result:c2")]);
    }

    #[test]
    fn the_oldest_open_call_is_forgotten_past_the_bound() {
        // It keeps at most CALL_BOUND open calls, so unanswered calls cannot grow without limit
        let mut calls = Calls::default();
        for n in 0..=CALL_BOUND {
            calls.edges("t1", &[], &[format!("c{n}")]);
        }

        assert!(calls.edges("t1", &ids(&["c0"]), &[]).is_empty());
        assert_eq!(calls.edges("t1", &[format!("c{CALL_BOUND}")], &[]).len(), 1);
    }

    #[test]
    fn answered_calls_do_not_make_the_order_list_grow_without_limit() {
        // A long run of answered calls behind one unanswered call keeps memory bounded
        let mut calls = Calls::default();
        calls.edges("t1", &[], &ids(&["first"]));
        for n in 0..CALL_BOUND * 4 {
            calls.edges("t1", &[], &[format!("c{n}")]);
            calls.edges("t1", &[format!("c{n}")], &[]);
        }

        assert!(calls.order.len() <= 2 * CALL_BOUND + 1);
        assert_eq!(calls.edges("t1", &ids(&["first"]), &[]).len(), 1);
    }
}
