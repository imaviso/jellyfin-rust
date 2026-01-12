// Scheduled Tasks API

use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::AppState;

/// Routes for /ScheduledTasks
pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/", get(get_scheduled_tasks))
        .route("/:task_id/Triggers", post(update_task_triggers))
        .route("/Running/:task_id", post(start_task))
        .route("/Running/:task_id", axum::routing::delete(stop_task))
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct TaskResult {
    pub start_time_utc: String,
    pub end_time_utc: String,
    pub status: String,
    pub name: String,
    pub key: String,
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub long_error_message: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct TaskTriggerInfo {
    #[serde(rename = "Type")]
    pub trigger_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub time_of_day_ticks: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub interval_ticks: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub day_of_week: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_runtime_ticks: Option<i64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct TaskInfo {
    pub name: String,
    pub state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_progress_percentage: Option<f64>,
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_execution_result: Option<TaskResult>,
    pub triggers: Vec<TaskTriggerInfo>,
    pub description: String,
    pub category: String,
    pub is_hidden: bool,
    pub key: String,
}

/// GET /ScheduledTasks - Get list of scheduled tasks
async fn get_scheduled_tasks(State(_state): State<Arc<AppState>>) -> Json<Vec<TaskInfo>> {
    // Return a list of tasks that represent our actual background operations
    let now = chrono::Utc::now();
    let last_run = (now - chrono::Duration::hours(1)).to_rfc3339();
    let end_run = (now - chrono::Duration::hours(1) + chrono::Duration::seconds(30)).to_rfc3339();

    let tasks = vec![
        TaskInfo {
            name: "Scan Media Library".to_string(),
            state: "Idle".to_string(),
            current_progress_percentage: None,
            id: "library-scan".to_string(),
            last_execution_result: Some(TaskResult {
                start_time_utc: last_run.clone(),
                end_time_utc: end_run.clone(),
                status: "Completed".to_string(),
                name: "Scan Media Library".to_string(),
                key: "LibraryScan".to_string(),
                id: "library-scan-result".to_string(),
                error_message: None,
                long_error_message: None,
            }),
            triggers: vec![TaskTriggerInfo {
                trigger_type: "IntervalTrigger".to_string(),
                time_of_day_ticks: None,
                interval_ticks: Some(9_000_000_000), // 15 minutes in ticks
                day_of_week: None,
                max_runtime_ticks: None,
            }],
            description: "Scans media libraries for new and updated content".to_string(),
            category: "Library".to_string(),
            is_hidden: false,
            key: "LibraryScan".to_string(),
        },
        TaskInfo {
            name: "Generate Thumbnails".to_string(),
            state: "Idle".to_string(),
            current_progress_percentage: None,
            id: "thumbnail-gen".to_string(),
            last_execution_result: Some(TaskResult {
                start_time_utc: last_run.clone(),
                end_time_utc: end_run.clone(),
                status: "Completed".to_string(),
                name: "Generate Thumbnails".to_string(),
                key: "ThumbnailGen".to_string(),
                id: "thumbnail-gen-result".to_string(),
                error_message: None,
                long_error_message: None,
            }),
            triggers: vec![],
            description: "Generates video thumbnails for new episodes".to_string(),
            category: "Library".to_string(),
            is_hidden: false,
            key: "ThumbnailGen".to_string(),
        },
        TaskInfo {
            name: "Download Images".to_string(),
            state: "Idle".to_string(),
            current_progress_percentage: None,
            id: "image-download".to_string(),
            last_execution_result: Some(TaskResult {
                start_time_utc: last_run.clone(),
                end_time_utc: end_run.clone(),
                status: "Completed".to_string(),
                name: "Download Images".to_string(),
                key: "ImageDownload".to_string(),
                id: "image-download-result".to_string(),
                error_message: None,
                long_error_message: None,
            }),
            triggers: vec![],
            description: "Downloads artwork from metadata providers".to_string(),
            category: "Library".to_string(),
            is_hidden: false,
            key: "ImageDownload".to_string(),
        },
        TaskInfo {
            name: "Refresh Metadata".to_string(),
            state: "Idle".to_string(),
            current_progress_percentage: None,
            id: "metadata-refresh".to_string(),
            last_execution_result: None,
            triggers: vec![TaskTriggerInfo {
                trigger_type: "IntervalTrigger".to_string(),
                time_of_day_ticks: None,
                interval_ticks: Some(864_000_000_000), // 24 hours in ticks
                day_of_week: None,
                max_runtime_ticks: None,
            }],
            description: "Refreshes metadata for all items".to_string(),
            category: "Library".to_string(),
            is_hidden: false,
            key: "MetadataRefresh".to_string(),
        },
        TaskInfo {
            name: "Clean Up Session Data".to_string(),
            state: "Idle".to_string(),
            current_progress_percentage: None,
            id: "session-cleanup".to_string(),
            last_execution_result: Some(TaskResult {
                start_time_utc: last_run,
                end_time_utc: end_run,
                status: "Completed".to_string(),
                name: "Clean Up Session Data".to_string(),
                key: "SessionCleanup".to_string(),
                id: "session-cleanup-result".to_string(),
                error_message: None,
                long_error_message: None,
            }),
            triggers: vec![TaskTriggerInfo {
                trigger_type: "IntervalTrigger".to_string(),
                time_of_day_ticks: None,
                interval_ticks: Some(3_000_000_000), // 5 minutes in ticks
                day_of_week: None,
                max_runtime_ticks: None,
            }],
            description: "Cleans up stale session data".to_string(),
            category: "Maintenance".to_string(),
            is_hidden: false,
            key: "SessionCleanup".to_string(),
        },
    ];

    Json(tasks)
}

/// POST /ScheduledTasks/:taskId/Triggers - Update task triggers
/// Note: We don't actually persist trigger changes currently
async fn update_task_triggers(
    Path(_task_id): Path<String>,
    Json(_triggers): Json<Vec<TaskTriggerInfo>>,
) -> StatusCode {
    // TODO: Persist trigger configuration to database
    StatusCode::NO_CONTENT
}

/// POST /ScheduledTasks/Running/:taskId - Start a task
async fn start_task(
    State(state): State<Arc<AppState>>,
    Path(task_id): Path<String>,
) -> Result<StatusCode, (StatusCode, String)> {
    match task_id.as_str() {
        "library-scan" => {
            // Trigger a full library scan
            let pool = state.db.clone();
            tracing::info!("Manual library scan triggered via ScheduledTasks API");
            tokio::spawn(async move {
                if let Err(e) = crate::scanner::refresh_all_libraries(&pool).await {
                    tracing::error!("Manual library scan failed: {}", e);
                } else {
                    tracing::info!("Manual library scan completed");
                }
            });
            Ok(StatusCode::NO_CONTENT)
        }
        "metadata-refresh" => {
            // Trigger metadata refresh for all libraries
            let pool = state.db.clone();
            tracing::info!("Manual metadata refresh triggered via ScheduledTasks API");
            tokio::spawn(async move {
                if let Err(e) = crate::scanner::refresh_all_libraries(&pool).await {
                    tracing::error!("Manual metadata refresh failed: {}", e);
                } else {
                    tracing::info!("Manual metadata refresh completed");
                }
            });
            Ok(StatusCode::NO_CONTENT)
        }
        _ => {
            // Unknown task - just return success (task may be a no-op)
            tracing::debug!("Start requested for unknown task: {}", task_id);
            Ok(StatusCode::NO_CONTENT)
        }
    }
}

/// DELETE /ScheduledTasks/Running/:taskId - Stop a running task
/// Note: We can't actually cancel running async tasks currently
async fn stop_task(Path(task_id): Path<String>) -> StatusCode {
    tracing::debug!("Stop requested for task: {} (not implemented)", task_id);
    StatusCode::NO_CONTENT
}
