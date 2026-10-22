use crate::project::Project;
use serde_json::{json, Value};

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
}
