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
    #[serde(default)]
    pub skills: Vec<String>,
    pub acceptance: String,
    pub parent_id: Option<i64>,
    #[serde(default)]
    pub assigned_to: String,
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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub relations: Vec<Relation>,
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
    pub skills: Vec<String>,
    pub acceptance: String,
    #[serde(default)]
    pub assigned_to: String,
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
    pub skills: Vec<String>,
    #[serde(default)]
    pub acceptance: String,
    #[serde(default)]
    pub parent_id: Option<i64>,
    #[serde(default)]
    pub assigned_to: String,
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
    pub by_skills: std::collections::HashMap<String, i64>,
    pub by_assignee: std::collections::HashMap<String, i64>,
    pub oldest_open: Option<OldestOpen>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OldestOpen {
    pub id: i64,
    pub title: String,
    pub days_old: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
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
    pub skills: Vec<String>,
    pub acceptance: String,
    #[serde(default)]
    pub assigned_to: String,
    pub matched_fields: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_snippets: Option<std::collections::HashMap<String, String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BulkResult {
    pub action: String,
    pub count: usize,
    pub ids: Vec<i64>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub unblocked: Vec<UnblockedIssue>,
    pub dry_run: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Relation {
    pub id: i64,
    pub source_id: i64,
    pub target_id: i64,
    pub relation_type: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    pub id: i64,
    pub issue_id: i64,
    pub field: String,
    pub old_value: String,
    pub new_value: String,
    pub agent: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchCloseInput {
    pub id: i64,
    #[serde(default)]
    pub reason: String,
    #[serde(default)]
    pub wontfix: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchUpdateInput {
    pub id: i64,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub priority: Option<String>,
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub context: Option<String>,
    #[serde(default)]
    pub assigned_to: Option<String>,
    #[serde(default)]
    pub add_tags: Vec<String>,
    #[serde(default)]
    pub remove_tags: Vec<String>,
    #[serde(default)]
    pub add_skills: Vec<String>,
    #[serde(default)]
    pub remove_skills: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchItemResult {
    pub id: i64,
    pub outcome: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unblocked: Vec<UnblockedIssue>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchSummary {
    pub total: usize,
    pub ok: usize,
    pub error: usize,
    pub review: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchResult {
    pub action: String,
    pub results: Vec<BatchItemResult>,
    pub summary: BatchSummary,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportData {
    pub issue: Issue,
    pub notes: Vec<Note>,
    pub blocked_by: Vec<i64>,
    #[serde(default)]
    pub events: Vec<Event>,
    #[serde(default)]
    pub relations: Vec<Relation>,
}
