// Display Preferences API - Persists user display preferences per client

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

use crate::AppState;

/// Routes for /DisplayPreferences
pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/:display_prefs_id", get(get_display_preferences))
        .route("/:display_prefs_id", post(update_display_preferences))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DisplayPreferencesQuery {
    pub user_id: Option<String>,
    pub client: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "PascalCase")]
pub struct DisplayPreferences {
    pub id: String,
    pub view_type: Option<String>,
    pub sort_by: String,
    pub index_by: Option<String>,
    pub remember_indexing: bool,
    pub primary_image_height: i32,
    pub primary_image_width: i32,
    pub custom_prefs: HashMap<String, String>,
    pub scroll_direction: String,
    pub show_backdrop: bool,
    pub remember_sorting: bool,
    pub sort_order: String,
    pub show_sidebar: bool,
    pub client: String,
}

impl Default for DisplayPreferences {
    fn default() -> Self {
        Self {
            id: String::new(),
            view_type: None,
            sort_by: "SortName".to_string(),
            index_by: None,
            remember_indexing: false,
            primary_image_height: 250,
            primary_image_width: 250,
            custom_prefs: HashMap::new(),
            scroll_direction: "Horizontal".to_string(),
            show_backdrop: true,
            remember_sorting: false,
            sort_order: "Ascending".to_string(),
            show_sidebar: true,
            client: "default".to_string(),
        }
    }
}

/// Database row for display preferences
#[derive(Debug, sqlx::FromRow)]
struct DisplayPreferencesRow {
    id: String,
    #[allow(dead_code)]
    user_id: String,
    client: String,
    view_type: Option<String>,
    sort_by: Option<String>,
    sort_order: Option<String>,
    remember_sorting: i32,
    index_by: Option<String>,
    remember_indexing: i32,
    primary_image_height: Option<i32>,
    primary_image_width: Option<i32>,
    scroll_direction: Option<String>,
    show_backdrop: i32,
    show_sidebar: i32,
    custom_prefs: Option<String>,
}

impl DisplayPreferencesRow {
    fn into_dto(self) -> DisplayPreferences {
        let custom_prefs: HashMap<String, String> = self
            .custom_prefs
            .as_ref()
            .and_then(|s| serde_json::from_str(s).ok())
            .unwrap_or_default();

        DisplayPreferences {
            id: self.id,
            view_type: self.view_type,
            sort_by: self.sort_by.unwrap_or_else(|| "SortName".to_string()),
            index_by: self.index_by,
            remember_indexing: self.remember_indexing != 0,
            primary_image_height: self.primary_image_height.unwrap_or(250),
            primary_image_width: self.primary_image_width.unwrap_or(250),
            custom_prefs,
            scroll_direction: self
                .scroll_direction
                .unwrap_or_else(|| "Horizontal".to_string()),
            show_backdrop: self.show_backdrop != 0,
            remember_sorting: self.remember_sorting != 0,
            sort_order: self.sort_order.unwrap_or_else(|| "Ascending".to_string()),
            show_sidebar: self.show_sidebar != 0,
            client: self.client,
        }
    }
}

/// GET /DisplayPreferences/:id
async fn get_display_preferences(
    State(state): State<Arc<AppState>>,
    Path(display_prefs_id): Path<String>,
    Query(query): Query<DisplayPreferencesQuery>,
) -> Json<DisplayPreferences> {
    let user_id = query.user_id.as_deref().unwrap_or("default");
    let client = query.client.as_deref().unwrap_or("default");

    // Try to load from database
    let row: Option<DisplayPreferencesRow> = sqlx::query_as(
        "SELECT * FROM display_preferences WHERE id = ? AND user_id = ? AND client = ?",
    )
    .bind(&display_prefs_id)
    .bind(user_id)
    .bind(client)
    .fetch_optional(&state.db)
    .await
    .ok()
    .flatten();

    match row {
        Some(r) => Json(r.into_dto()),
        None => {
            // Return defaults
            let prefs = DisplayPreferences {
                id: display_prefs_id,
                client: client.to_string(),
                ..Default::default()
            };
            Json(prefs)
        }
    }
}

/// POST /DisplayPreferences/:id
async fn update_display_preferences(
    State(state): State<Arc<AppState>>,
    Path(display_prefs_id): Path<String>,
    Query(query): Query<DisplayPreferencesQuery>,
    Json(prefs): Json<DisplayPreferences>,
) -> Result<StatusCode, (StatusCode, String)> {
    let user_id = query.user_id.as_deref().unwrap_or("default");
    let client = query.client.as_deref().unwrap_or(&prefs.client);

    let custom_prefs_json = serde_json::to_string(&prefs.custom_prefs).unwrap_or_default();

    // Upsert the preferences
    sqlx::query(
        r#"INSERT INTO display_preferences 
           (id, user_id, client, view_type, sort_by, sort_order, remember_sorting, 
            index_by, remember_indexing, primary_image_height, primary_image_width,
            scroll_direction, show_backdrop, show_sidebar, custom_prefs)
           VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
           ON CONFLICT(user_id, client, id) DO UPDATE SET
            view_type = excluded.view_type,
            sort_by = excluded.sort_by,
            sort_order = excluded.sort_order,
            remember_sorting = excluded.remember_sorting,
            index_by = excluded.index_by,
            remember_indexing = excluded.remember_indexing,
            primary_image_height = excluded.primary_image_height,
            primary_image_width = excluded.primary_image_width,
            scroll_direction = excluded.scroll_direction,
            show_backdrop = excluded.show_backdrop,
            show_sidebar = excluded.show_sidebar,
            custom_prefs = excluded.custom_prefs"#,
    )
    .bind(&display_prefs_id)
    .bind(user_id)
    .bind(client)
    .bind(&prefs.view_type)
    .bind(&prefs.sort_by)
    .bind(&prefs.sort_order)
    .bind(prefs.remember_sorting as i32)
    .bind(&prefs.index_by)
    .bind(prefs.remember_indexing as i32)
    .bind(prefs.primary_image_height)
    .bind(prefs.primary_image_width)
    .bind(&prefs.scroll_direction)
    .bind(prefs.show_backdrop as i32)
    .bind(prefs.show_sidebar as i32)
    .bind(&custom_prefs_json)
    .execute(&state.db)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(StatusCode::NO_CONTENT)
}
