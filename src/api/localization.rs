use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    routing::get,
    Json, Router,
};
use serde::Serialize;
use std::sync::Arc;

use crate::{services::auth, AppState};

use super::users::parse_emby_auth_header;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/Cultures", get(get_cultures))
        .route("/Countries", get(get_countries))
        .route("/ParentalRatings", get(get_parental_ratings))
        .route("/Options", get(get_localization_options))
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct CultureDto {
    pub name: String,
    pub display_name: String,
    pub two_letter_iso_language_name: String,
    pub three_letter_iso_language_name: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct CountryDto {
    pub name: String,
    pub display_name: String,
    pub two_letter_iso_region_name: String,
    pub three_letter_iso_region_name: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct ParentalRatingDto {
    pub name: String,
    pub value: i32,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct LocalizationOption {
    pub name: String,
    pub value: String,
}

async fn require_auth(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<crate::models::User, (StatusCode, String)> {
    let (_, _, _, token) = parse_emby_auth_header(headers)
        .ok_or_else(|| (StatusCode::UNAUTHORIZED, "Missing auth header".to_string()))?;

    let token = token.ok_or_else(|| (StatusCode::UNAUTHORIZED, "Missing token".to_string()))?;

    auth::validate_session(&state.db, &token)
        .await
        .map_err(|e| (StatusCode::UNAUTHORIZED, e.to_string()))
}

async fn get_cultures(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<Vec<CultureDto>>, (StatusCode, String)> {
    let _user = require_auth(&state, &headers).await?;

    let cultures = [
        ("en-US", "English (United States)", "en", "eng"),
        ("en-GB", "English (United Kingdom)", "en", "eng"),
        ("ja-JP", "Japanese (Japan)", "ja", "jpn"),
        ("zh-CN", "Chinese (Simplified)", "zh", "zho"),
        ("zh-TW", "Chinese (Traditional)", "zh", "zho"),
        ("ko-KR", "Korean (Korea)", "ko", "kor"),
        ("de-DE", "German (Germany)", "de", "deu"),
        ("fr-FR", "French (France)", "fr", "fra"),
        ("es-ES", "Spanish (Spain)", "es", "spa"),
        ("pt-BR", "Portuguese (Brazil)", "pt", "por"),
        ("it-IT", "Italian (Italy)", "it", "ita"),
        ("ru-RU", "Russian (Russia)", "ru", "rus"),
        ("nl-NL", "Dutch (Netherlands)", "nl", "nld"),
        ("pl-PL", "Polish (Poland)", "pl", "pol"),
        ("sv-SE", "Swedish (Sweden)", "sv", "swe"),
    ]
    .into_iter()
    .map(|(name, display, iso2, iso3)| CultureDto {
        name: name.to_string(),
        display_name: display.to_string(),
        two_letter_iso_language_name: iso2.to_string(),
        three_letter_iso_language_name: iso3.to_string(),
    })
    .collect();

    Ok(Json(cultures))
}

async fn get_countries(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<Vec<CountryDto>>, (StatusCode, String)> {
    let _user = require_auth(&state, &headers).await?;

    let countries = [
        ("US", "United States", "USA"),
        ("GB", "United Kingdom", "GBR"),
        ("JP", "Japan", "JPN"),
        ("CN", "China", "CHN"),
        ("KR", "South Korea", "KOR"),
        ("DE", "Germany", "DEU"),
        ("FR", "France", "FRA"),
        ("ES", "Spain", "ESP"),
        ("IT", "Italy", "ITA"),
        ("CA", "Canada", "CAN"),
        ("AU", "Australia", "AUS"),
        ("BR", "Brazil", "BRA"),
        ("MX", "Mexico", "MEX"),
        ("RU", "Russia", "RUS"),
        ("IN", "India", "IND"),
        ("NL", "Netherlands", "NLD"),
        ("SE", "Sweden", "SWE"),
        ("NO", "Norway", "NOR"),
        ("DK", "Denmark", "DNK"),
        ("FI", "Finland", "FIN"),
    ]
    .into_iter()
    .map(|(code, name, code3)| CountryDto {
        name: name.to_string(),
        display_name: name.to_string(),
        two_letter_iso_region_name: code.to_string(),
        three_letter_iso_region_name: code3.to_string(),
    })
    .collect();

    Ok(Json(countries))
}

async fn get_parental_ratings(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<Vec<ParentalRatingDto>>, (StatusCode, String)> {
    let _user = require_auth(&state, &headers).await?;

    let ratings = [
        ("G", 0),
        ("PG", 10),
        ("PG-13", 13),
        ("R", 17),
        ("NC-17", 18),
        ("TV-Y", 0),
        ("TV-Y7", 7),
        ("TV-G", 0),
        ("TV-PG", 10),
        ("TV-14", 14),
        ("TV-MA", 17),
    ]
    .into_iter()
    .map(|(name, value)| ParentalRatingDto {
        name: name.to_string(),
        value,
    })
    .collect();

    Ok(Json(ratings))
}

async fn get_localization_options(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<Vec<LocalizationOption>>, (StatusCode, String)> {
    let _user = require_auth(&state, &headers).await?;

    let options = [
        ("English", "en-US"),
        ("Japanese", "ja-JP"),
        ("German", "de-DE"),
        ("French", "fr-FR"),
        ("Spanish", "es-ES"),
    ]
    .into_iter()
    .map(|(name, value)| LocalizationOption {
        name: name.to_string(),
        value: value.to_string(),
    })
    .collect();

    Ok(Json(options))
}
