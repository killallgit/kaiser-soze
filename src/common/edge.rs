use serde::Serialize;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EdgeKind {
    InProject,
    InSession,
    InThread,
    Spawned,
    Turn,
    Continues,
    Calls,
}

/// One relation of the graph. The fields are declared in key order, so a serialised edge has its keys sorted.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Edge {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    pub from: String,
    pub kind: EdgeKind,
    pub to: String,
}

impl Edge {
    pub fn new(kind: EdgeKind, from: impl Into<String>, to: impl Into<String>) -> Self {
        Self {
            branch: None,
            from: from.into(),
            kind,
            to: to.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_edge_serialises_its_keys_in_sorted_order() {
        // It writes the bytes the tap file has always held for an edge
        let edge = Edge::new(EdgeKind::InThread, "exchange:7", "thread:t1");

        assert_eq!(
            serde_json::to_string(&edge).unwrap(),
            r#"{"from":"exchange:7","kind":"in_thread","to":"thread:t1"}"#
        );
    }

    #[test]
    fn a_branch_is_written_only_when_there_is_one() {
        // It sorts the branch first, and leaves the key out when HEAD names no branch
        let on_main = Edge {
            branch: Some("main".into()),
            ..Edge::new(EdgeKind::InProject, "session:s1", "project:p")
        };
        let detached = Edge::new(EdgeKind::InProject, "session:s1", "project:p");

        assert_eq!(
            serde_json::to_string(&on_main).unwrap(),
            r#"{"branch":"main","from":"session:s1","kind":"in_project","to":"project:p"}"#
        );
        assert_eq!(
            serde_json::to_string(&detached).unwrap(),
            r#"{"from":"session:s1","kind":"in_project","to":"project:p"}"#
        );
    }

    #[test]
    fn every_kind_is_written_in_snake_case() {
        // It names the kinds docs/common-format.md lists
        let kinds = [
            EdgeKind::InProject,
            EdgeKind::InSession,
            EdgeKind::InThread,
            EdgeKind::Spawned,
            EdgeKind::Turn,
            EdgeKind::Continues,
            EdgeKind::Calls,
        ];

        let written: Vec<String> = kinds.iter().map(|kind| serde_json::to_string(kind).unwrap()).collect();

        assert_eq!(
            written,
            [
                r#""in_project""#,
                r#""in_session""#,
                r#""in_thread""#,
                r#""spawned""#,
                r#""turn""#,
                r#""continues""#,
                r#""calls""#
            ]
        );
    }
}
