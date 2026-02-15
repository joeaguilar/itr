use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Issue {
    pub id: i64,
    pub title: String,
    pub status: String,
    pub priority: String,
    pub kind: String,
    pub context: String,
    pub files: Vec<String>,
    pub tags: Vec<String>,
    pub acceptance: String,
    pub parent_id: Option<i64>,
    pub close_reason: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Note {
    pub id: i64,
    pub issue_id: i64,
    pub content: String,
    pub agent: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IssueDetail {
    #[serde(flatten)]
    pub issue: Issue,
    pub urgency: f64,
    pub blocked_by: Vec<i64>,
    pub blocks: Vec<i64>,
    pub is_blocked: bool,
    pub notes: Vec<Note>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub urgency_breakdown: Option<UrgencyBreakdown>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub children: Option<Vec<IssueSummary>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IssueSummary {
    pub id: i64,
    pub title: String,
    pub status: String,
    pub priority: String,
    pub kind: String,
    pub urgency: f64,
    pub is_blocked: bool,
    pub blocked_by: Vec<i64>,
    pub tags: Vec<String>,
    pub files: Vec<String>,
    pub acceptance: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UrgencyBreakdown {
    pub components: Vec<(String, f64)>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnblockedIssue {
    pub id: i64,
    pub title: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchAddInput {
    pub title: String,
    #[serde(default = "default_priority")]
    pub priority: String,
    #[serde(default = "default_kind")]
    pub kind: String,
    #[serde(default)]
    pub context: String,
    #[serde(default)]
    pub files: Vec<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub acceptance: String,
    #[serde(default)]
    pub parent_id: Option<i64>,
    #[serde(default)]
    pub blocked_by: Vec<serde_json::Value>,
}

fn default_priority() -> String {
    "medium".to_string()
}

fn default_kind() -> String {
    "task".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphOutput {
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphNode {
    pub id: i64,
    pub title: String,
    pub status: String,
    pub urgency: f64,
    pub is_blocked: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphEdge {
    pub from: i64,
    pub to: i64,
    #[serde(rename = "type")]
    pub edge_type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Stats {
    pub total: i64,
    pub by_status: std::collections::HashMap<String, i64>,
    pub by_priority: std::collections::HashMap<String, i64>,
    pub by_kind: std::collections::HashMap<String, i64>,
    pub blocked: i64,
    pub ready: i64,
    pub avg_urgency: f64,
    pub oldest_open: Option<OldestOpen>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OldestOpen {
    pub id: i64,
    pub title: String,
    pub days_old: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportData {
    pub issue: Issue,
    pub notes: Vec<Note>,
    pub blocked_by: Vec<i64>,
}
