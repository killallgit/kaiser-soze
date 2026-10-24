mod calls;

pub use calls::Calls;

use crate::project::Project;
use serde_json::{json, Value};
use std::collections::{HashMap, VecDeque};

/// Which session, thread and turn an exchange belongs to, as its request headers say.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Identity {
    pub session: Option<String>,
    pub thread: Option<String>,
    pub parent_thread: Option<String>,
    pub turn: Option<String>,
}

/// The edges one exchange adds to the graph. An edge whose far end is unknown is left out.
pub fn edges(exchange: u64, identity: &Identity, project: Option<&Project>) -> Vec<Value> {
    let node = |kind: &str, id: &Option<String>| id.as_ref().map(|id| format!("{kind}:{id}"));
    let session = node("session", &identity.session);
    let thread = node("thread", &identity.thread);
    let parent = node("thread", &identity.parent_thread);
    let mut edges = Vec::new();
    if let (Some(session), Some(project)) = (&session, project) {
        let mut edge = edge("in_project", session, &format!("project:{}", project.id));
        if let Some(branch) = &project.branch {
            edge["branch"] = json!(branch);
        }
        edges.push(edge);
    }
    if let (Some(thread), Some(session)) = (&thread, &session) {
        edges.push(edge("in_session", thread, session));
    }
    if let Some(thread) = &thread {
        edges.push(edge("in_thread", &format!("exchange:{exchange}"), thread));
    }
    if let (Some(thread), Some(parent)) = (&thread, &parent) {
        edges.push(edge("spawned", thread, parent));
    }
    edges
}

/// How many threads the tracker remembers before it forgets the least recently started one.
pub const THREAD_BOUND: usize = 1024;

/// How many reply ids one thread remembers; past it the thread forgets them all but the newest.
pub const REPLY_BOUND: usize = 1024;

/// One request and its reply: the unit `turn` and `continues` link.
#[derive(Clone, Debug, PartialEq)]
pub struct Step {
    pub thread: Option<String>,
    pub exchange: u64,
    pub n: u64,
    pub turn: Option<String>,
    pub previous_message_id: Option<String>,
    pub message_id: Option<String>,
    /// Call ids of the `tool_result` blocks in the request.
    pub results: Vec<String>,
    /// Ids of the `tool_call` blocks in the reply.
    pub calls: Vec<String>,
}

#[derive(Default)]
struct Seen {
    last: Option<String>,
    replies: HashMap<String, String>,
}

/// Remembers the last step and the open tool calls of each thread, bounded, so a step can say which
/// step it continues and which call its results answer.
#[derive(Default)]
pub struct Threads {
    seen: HashMap<String, Seen>,
    order: VecDeque<String>,
    calls: Calls,
}

impl Threads {
    /// The `turn`, `continues` and `calls` edges one finished step adds, then remembers the step.
    /// A previous message id continues the step that replied with it, or nothing when that
    /// reply is unknown; without one the step continues the thread's last step.
    pub fn edges(&mut self, step: &Step) -> Vec<Value> {
        let node = format!("step:{}.{}", step.exchange, step.n);
        let mut edges = Vec::new();
        if let Some(turn) = &step.turn {
            edges.push(edge("turn", &node, &format!("turn:{turn}")));
        }
        let Some(thread) = &step.thread else {
            return edges;
        };
        if !self.seen.contains_key(thread) {
            self.order.push_back(thread.clone());
            if self.order.len() > THREAD_BOUND {
                if let Some(oldest) = self.order.pop_front() {
                    self.seen.remove(&oldest);
                }
            }
        }
        let seen = self.seen.entry(thread.clone()).or_default();
        let before = match &step.previous_message_id {
            Some(id) => seen.replies.get(id),
            None => seen.last.as_ref(),
        };
        if let Some(before) = before {
            edges.push(edge("continues", &node, before));
        }
        seen.last = Some(node.clone());
        if let Some(id) = &step.message_id {
            if seen.replies.len() >= REPLY_BOUND {
                seen.replies.clear();
            }
            seen.replies.insert(id.clone(), node);
        }
        edges.extend(self.calls.edges(thread, &step.results, &step.calls));
        edges
    }
}

fn edge(kind: &str, from: &str, to: &str) -> Value {
    json!({"kind": kind, "from": from, "to": to})
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity(session: &str, thread: &str, parent: Option<&str>) -> Identity {
        Identity {
            session: Some(session.into()),
            thread: Some(thread.into()),
            parent_thread: parent.map(Into::into),
            turn: None,
        }
    }

    fn project() -> Project {
        Project {
            id: "host/owner/repo".into(),
            branch: Some("main".into()),
        }
    }

    fn relations(edges: &[Value]) -> Vec<(&str, &str, &str)> {
        edges
            .iter()
            .map(|e| {
                (
                    e["kind"].as_str().unwrap(),
                    e["from"].as_str().unwrap(),
                    e["to"].as_str().unwrap(),
                )
            })
            .collect()
    }

    #[test]
    fn a_thread_in_a_session_gives_session_and_thread_edges() {
        // It links the thread to its session and the exchange to its thread
        let edges = edges(7, &identity("s1", "t1", None), None);

        assert_eq!(
            relations(&edges),
            [
                ("in_session", "thread:t1", "session:s1"),
                ("in_thread", "exchange:7", "thread:t1")
            ]
        );
    }

    #[test]
    fn a_parent_thread_gives_a_spawned_edge() {
        // It links the child thread to the parent that launched it
        let edges = edges(7, &identity("s1", "t2", Some("t1")), None);

        assert!(relations(&edges).contains(&("spawned", "thread:t2", "thread:t1")));
    }

    #[test]
    fn a_project_gives_an_in_project_edge_carrying_the_branch() {
        // It links the session to the project and keeps the branch as metadata
        let edges = edges(7, &identity("s1", "t1", None), Some(&project()));

        let edge = edges.iter().find(|e| e["kind"] == "in_project").unwrap();
        assert_eq!(edge["from"], "session:s1");
        assert_eq!(edge["to"], "project:host/owner/repo");
        assert_eq!(edge["branch"], "main");
    }

    #[test]
    fn a_project_without_a_branch_carries_no_branch() {
        // It omits the branch key when HEAD names no branch
        let detached = Project {
            id: "p".into(),
            branch: None,
        };

        let edges = edges(7, &identity("s1", "t1", None), Some(&detached));

        let edge = edges.iter().find(|e| e["kind"] == "in_project").unwrap();
        assert!(edge.get("branch").is_none());
    }

    #[test]
    fn an_empty_identity_gives_no_edges() {
        // It writes nothing for ids it was not given, even with a project
        assert!(edges(7, &Identity::default(), Some(&project())).is_empty());
    }

    #[test]
    fn a_thread_without_a_session_still_gives_in_thread_only() {
        // It skips the edges whose far end is unknown
        let identity = Identity {
            thread: Some("t1".into()),
            ..Identity::default()
        };

        assert_eq!(
            relations(&edges(7, &identity, None)),
            [("in_thread", "exchange:7", "thread:t1")]
        );
    }

    fn step(thread: &str, exchange: u64, n: u64) -> Step {
        Step {
            thread: Some(thread.into()),
            exchange,
            n,
            turn: None,
            previous_message_id: None,
            message_id: None,
            results: Vec::new(),
            calls: Vec::new(),
        }
    }

    #[test]
    fn a_step_in_a_turn_gives_a_turn_edge() {
        // It links the step to the turn its request named
        let mut threads = Threads::default();
        let step = Step {
            turn: Some("u1".into()),
            ..step("t1", 7, 2)
        };

        assert_eq!(relations(&threads.edges(&step)), [("turn", "step:7.2", "turn:u1")]);
    }

    #[test]
    fn a_later_step_on_a_thread_continues_the_one_before_it() {
        // Without a previous message id it follows arrival order, across exchanges and within a socket
        let mut threads = Threads::default();

        let first = threads.edges(&step("t1", 7, 1));
        let second = threads.edges(&step("t1", 7, 2));
        let third = threads.edges(&step("t1", 8, 1));
        let other = threads.edges(&step("t2", 9, 1));

        assert!(first.is_empty() && other.is_empty());
        assert_eq!(relations(&second), [("continues", "step:7.2", "step:7.1")]);
        assert_eq!(relations(&third), [("continues", "step:8.1", "step:7.2")]);
    }

    #[test]
    fn a_previous_message_id_continues_the_step_that_replied_with_it() {
        // It follows the id, not arrival order, and skips a step whose reply it never saw
        let mut threads = Threads::default();
        let reply = |exchange, message_id: &str| Step {
            message_id: Some(message_id.into()),
            ..step("t1", exchange, 1)
        };
        threads.edges(&reply(1, "m1"));
        threads.edges(&reply(2, "m2"));

        let follows_first = Step {
            previous_message_id: Some("m1".into()),
            ..step("t1", 3, 1)
        };
        let follows_unknown = Step {
            previous_message_id: Some("gone".into()),
            ..step("t1", 4, 1)
        };

        assert_eq!(
            relations(&threads.edges(&follows_first)),
            [("continues", "step:3.1", "step:1.1")]
        );
        assert!(threads.edges(&follows_unknown).is_empty());
    }

    #[test]
    fn a_step_without_a_thread_gives_no_continues_edge() {
        // It cannot be ordered against anything
        let mut threads = Threads::default();
        let nameless = Step {
            thread: None,
            ..step("t1", 7, 1)
        };

        threads.edges(&nameless);

        assert!(threads.edges(&nameless).is_empty());
    }

    #[test]
    fn the_tracker_forgets_the_oldest_thread_past_its_bound() {
        // It keeps at most THREAD_BOUND threads, so a long run does not grow without limit
        let mut threads = Threads::default();
        threads.edges(&step("first", 1, 1));
        for exchange in 2..=THREAD_BOUND as u64 + 1 {
            threads.edges(&step(&format!("t{exchange}"), exchange, 1));
        }

        assert!(threads.edges(&step("first", 9999, 1)).is_empty());
    }

    #[test]
    fn a_thread_forgets_old_reply_ids_past_its_bound_and_keeps_the_newest() {
        // A request naming a forgotten reply gets no edge; one naming the newest still does
        let mut threads = Threads::default();
        for exchange in 1..=REPLY_BOUND as u64 + 1 {
            threads.edges(&Step {
                message_id: Some(format!("m{exchange}")),
                ..step("t1", exchange, 1)
            });
        }
        let after = |id: String| Step {
            previous_message_id: Some(id),
            ..step("t1", 9999, 1)
        };

        let forgotten = threads.edges(&after("m1".into()));
        let newest = threads.edges(&after(format!("m{}", REPLY_BOUND + 1)));

        assert!(forgotten.is_empty());
        assert_eq!(relations(&newest).len(), 1);
    }
}
