//! Linear, as much of it as printing needs: verifying a webhook came from
//! Linear, working out whether a comment asked for a print, and fetching the
//! issue behind it.
//!
//! A comment webhook carries only `issueId`, so the issue itself has to be
//! fetched over GraphQL — which is why this needs an API key and not just the
//! signing secret.

use hmac::{Hmac, Mac};
use serde::Deserialize;
use sha2::Sha256;

/// The marker that asks for a print. Deliberately bracketed so it is hard to
/// write by accident and easy to spot in a comment.
pub const MARKER: &str = "[print]";

/// Linear gives up on a webhook it cannot deliver, so a receiver has to answer
/// inside this. We answer well before it and print afterwards.
pub const DEADLINE_SECS: u64 = 5;

/// How far apart the webhook's own clock and ours may be. Linear's guidance.
pub const MAX_CLOCK_SKEW_MS: i64 = 60_000;

#[derive(Debug)]
pub enum Error {
    Http(String),
    Api(String),
    NotFound,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Http(e) => write!(f, "talking to Linear: {e}"),
            Error::Api(e) => write!(f, "Linear said: {e}"),
            Error::NotFound => write!(f, "no such issue"),
        }
    }
}

// --- webhook ---------------------------------------------------------------

/// Is this really from Linear? The signature is over the raw bytes, so this
/// has to run before the body is parsed as JSON — re-serializing would change
/// the bytes and the digest with them.
pub fn signature_matches(secret: &str, raw_body: &[u8], header: &str) -> bool {
    let Ok(expected) = decode_hex(header) else {
        return false;
    };
    let Ok(mut mac) = Hmac::<Sha256>::new_from_slice(secret.as_bytes()) else {
        return false;
    };
    mac.update(raw_body);
    // verify_slice is constant time, which matters: a timing oracle here would
    // let someone hunt for a valid signature a byte at a time.
    mac.verify_slice(&expected).is_ok()
}

fn decode_hex(s: &str) -> Result<Vec<u8>, ()> {
    let s = s.trim();
    if s.len() % 2 != 0 {
        return Err(());
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).map_err(|_| ()))
        .collect()
}

#[derive(Deserialize)]
pub struct CommentEvent {
    pub action: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub data: Comment,
    #[serde(rename = "updatedFrom")]
    pub updated_from: Option<UpdatedFrom>,
    #[serde(rename = "webhookTimestamp")]
    pub webhook_timestamp: Option<i64>,
}

#[derive(Deserialize)]
pub struct Comment {
    pub id: String,
    pub body: String,
    #[serde(rename = "issueId")]
    pub issue_id: String,
    #[serde(rename = "userId")]
    pub user_id: Option<String>,
}

#[derive(Deserialize)]
pub struct UpdatedFrom {
    pub body: Option<String>,
}

/// Why a webhook did not lead to a print. Worth naming rather than returning a
/// bare bool: the log line is the only way to tell "ignored on purpose" from
/// "quietly broken".
pub enum Verdict {
    Print,
    NotAComment,
    NoMarker,
    MarkerWasAlreadyThere,
    NotYou,
    Removed,
}

impl Verdict {
    pub fn describe(&self) -> &'static str {
        match self {
            Verdict::Print => "printing",
            Verdict::NotAComment => "not a comment event",
            Verdict::NoMarker => "no marker in the comment",
            Verdict::MarkerWasAlreadyThere => "marker was already there before this edit",
            Verdict::NotYou => "someone else's comment",
            Verdict::Removed => "comment was removed",
        }
    }
}

impl CommentEvent {
    /// Decide whether this event asks for a print.
    ///
    /// The awkward case is an edit: Linear fires an update for any change to a
    /// comment, so a comment that already said `[print]` would print again
    /// every time a typo was fixed. Only the marker *arriving* counts.
    pub fn verdict(&self, viewer_id: &str) -> Verdict {
        if self.kind != "Comment" {
            return Verdict::NotAComment;
        }
        if self.action == "remove" {
            return Verdict::Removed;
        }
        if !has_marker(&self.data.body) {
            return Verdict::NoMarker;
        }
        if self.data.user_id.as_deref() != Some(viewer_id) {
            return Verdict::NotYou;
        }
        if self.action == "update" {
            let was_there = self
                .updated_from
                .as_ref()
                .and_then(|u| u.body.as_deref())
                .map(has_marker)
                // An update with no previous body to compare against is
                // ambiguous; treat it as already-there so a stray edit cannot
                // reprint. A fresh request is one new comment away.
                .unwrap_or(true);
            if was_there {
                return Verdict::MarkerWasAlreadyThere;
            }
        }
        Verdict::Print
    }

    /// Has this arrived within the window Linear suggests, or is it a replay?
    pub fn is_fresh(&self, now_ms: i64) -> bool {
        match self.webhook_timestamp {
            Some(sent) => (now_ms - sent).abs() < MAX_CLOCK_SKEW_MS,
            // Older payloads may not carry one; the signature still stands.
            None => true,
        }
    }
}

fn has_marker(body: &str) -> bool {
    body.to_lowercase().contains(MARKER)
}

/// The comment without the marker, so a note written alongside it can go on the
/// paper. Empty if the marker was the whole comment.
pub fn note(body: &str) -> String {
    let mut out = String::with_capacity(body.len());
    let mut rest = body;
    // The marker matched case-insensitively, so cut it the same way.
    while let Some(at) = rest.to_lowercase().find(MARKER) {
        out.push_str(&rest[..at]);
        rest = &rest[at + MARKER.len()..];
    }
    out.push_str(rest);
    out.trim().to_string()
}

// --- the API ---------------------------------------------------------------

pub struct Client {
    key: String,
    http: reqwest::Client,
}

/// Only the fields that end up on paper.
#[derive(Debug, Deserialize)]
pub struct Issue {
    pub identifier: String,
    pub title: String,
    pub description: Option<String>,
    #[serde(rename = "priorityLabel")]
    pub priority_label: Option<String>,
    pub estimate: Option<f64>,
    #[serde(rename = "dueDate")]
    pub due_date: Option<String>,
    pub state: Option<Named>,
    pub assignee: Option<Named>,
    pub project: Option<Named>,
    pub labels: Option<Nodes>,
}

#[derive(Debug, Deserialize)]
pub struct Named {
    pub name: String,
}

#[derive(Debug, Deserialize)]
pub struct Nodes {
    pub nodes: Vec<Named>,
}

impl Issue {
    /// The issue's labels, flattened to names for one line of paper.
    pub fn label_names(&self) -> Vec<&str> {
        self.labels
            .as_ref()
            .map(|l| l.nodes.iter().map(|n| n.name.as_str()).collect())
            .unwrap_or_default()
    }
}

const ISSUE_QUERY: &str = r#"
query Issue($id: String!) {
  issue(id: $id) {
    identifier
    title
    description
    priorityLabel
    estimate
    dueDate
    state { name }
    assignee { name }
    project { name }
    labels { nodes { name } }
  }
}"#;

const VIEWER_QUERY: &str = "query { viewer { id name } }";

impl Client {
    pub fn new(key: String) -> Self {
        Client {
            key,
            http: reqwest::Client::new(),
        }
    }

    async fn call(&self, query: &str, variables: serde_json::Value) -> Result<serde_json::Value, Error> {
        let response = self
            .http
            .post("https://api.linear.app/graphql")
            .header("authorization", &self.key)
            .json(&serde_json::json!({ "query": query, "variables": variables }))
            .send()
            .await
            .map_err(|e| Error::Http(e.to_string()))?;

        let status = response.status();
        let body: serde_json::Value = response
            .json()
            .await
            .map_err(|e| Error::Http(format!("{status}: {e}")))?;

        // GraphQL answers 200 with an errors array, so the status alone does
        // not say whether this worked.
        if let Some(errors) = body.get("errors") {
            return Err(Error::Api(errors.to_string()));
        }
        body.get("data").cloned().ok_or(Error::NotFound)
    }

    /// Who the API key belongs to. Used as "me" rather than asking for a user
    /// id to be pasted into config — the key already answers the question.
    pub async fn viewer(&self) -> Result<(String, String), Error> {
        let data = self.call(VIEWER_QUERY, serde_json::json!({})).await?;
        let viewer = data.get("viewer").ok_or(Error::NotFound)?;
        let id = viewer.get("id").and_then(|v| v.as_str()).ok_or(Error::NotFound)?;
        let name = viewer
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("(unnamed)");
        Ok((id.to_string(), name.to_string()))
    }

    pub async fn issue(&self, id: &str) -> Result<Issue, Error> {
        let data = self.call(ISSUE_QUERY, serde_json::json!({ "id": id })).await?;
        let issue = data.get("issue").ok_or(Error::NotFound)?;
        if issue.is_null() {
            return Err(Error::NotFound);
        }
        serde_json::from_value(issue.clone()).map_err(|e| Error::Api(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(action: &str, body: &str, user: &str, was: Option<&str>) -> CommentEvent {
        CommentEvent {
            action: action.into(),
            kind: "Comment".into(),
            data: Comment {
                id: "c1".into(),
                body: body.into(),
                issue_id: "i1".into(),
                user_id: Some(user.into()),
            },
            updated_from: was.map(|b| UpdatedFrom { body: Some(b.into()) }),
            webhook_timestamp: None,
        }
    }

    #[test]
    fn signature_is_hmac_of_the_raw_body() {
        // Checked against a known HMAC-SHA256 so this catches a change of
        // algorithm, not just a change of code.
        let secret = "shh";
        let body = b"{\"hello\":\"world\"}";
        let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(body);
        let good = mac.finalize().into_bytes();
        let good = good.iter().map(|b| format!("{b:02x}")).collect::<String>();

        assert!(signature_matches(secret, body, &good));
        assert!(!signature_matches("wrong", body, &good));
        assert!(!signature_matches(secret, b"tampered", &good));
        assert!(!signature_matches(secret, body, "not hex"));
        assert!(!signature_matches(secret, body, ""));
    }

    #[test]
    fn a_new_comment_from_me_prints() {
        let e = event("create", "[print] please", "me", None);
        assert!(matches!(e.verdict("me"), Verdict::Print));
    }

    #[test]
    fn someone_elses_comment_does_not() {
        let e = event("create", "[print]", "them", None);
        assert!(matches!(e.verdict("me"), Verdict::NotYou));
    }

    #[test]
    fn editing_a_comment_that_already_asked_does_not_reprint() {
        let e = event("update", "[print] fixed typo", "me", Some("[print] fixd typo"));
        assert!(matches!(e.verdict("me"), Verdict::MarkerWasAlreadyThere));
    }

    #[test]
    fn adding_the_marker_in_an_edit_does_print() {
        let e = event("update", "actually, [print]", "me", Some("no marker here"));
        assert!(matches!(e.verdict("me"), Verdict::Print));
    }

    #[test]
    fn an_edit_with_no_previous_body_is_treated_as_already_there() {
        let e = event("update", "[print]", "me", None);
        assert!(matches!(e.verdict("me"), Verdict::MarkerWasAlreadyThere));
    }

    #[test]
    fn removing_a_comment_does_not_print() {
        let e = event("remove", "[print]", "me", None);
        assert!(matches!(e.verdict("me"), Verdict::Removed));
    }

    #[test]
    fn the_marker_is_case_insensitive_and_comes_out_of_the_note() {
        let e = event("create", "[PRINT] look at this", "me", None);
        assert!(matches!(e.verdict("me"), Verdict::Print));
        assert_eq!(note("[PRINT] look at this"), "look at this");
        assert_eq!(note("[print]"), "");
        assert_eq!(note("before [print] after"), "before  after");
    }

    #[test]
    fn a_replay_from_last_week_is_not_fresh() {
        let mut e = event("create", "[print]", "me", None);
        e.webhook_timestamp = Some(1_000_000);
        assert!(!e.is_fresh(1_000_000 + 10 * 60_000));
        assert!(e.is_fresh(1_000_000 + 5_000));
    }
}
