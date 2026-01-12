use anyhow::{anyhow, Result};
use argon2::{
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};
use rand_core::OsRng;
use sqlx::SqlitePool;
use uuid::Uuid;

use crate::models::{Session, User};

/// Hash a password using Argon2
pub fn hash_password(password: &str) -> Result<String> {
    let salt = SaltString::generate(&mut OsRng);
    let argon2 = Argon2::default();
    let hash = argon2
        .hash_password(password.as_bytes(), &salt)
        .map_err(|e| anyhow!("Failed to hash password: {}", e))?;
    Ok(hash.to_string())
}

/// Verify a password against a hash
pub fn verify_password(password: &str, hash: &str) -> Result<bool> {
    let parsed_hash =
        PasswordHash::new(hash).map_err(|e| anyhow!("Failed to parse password hash: {}", e))?;
    Ok(Argon2::default()
        .verify_password(password.as_bytes(), &parsed_hash)
        .is_ok())
}

/// Create a new user
pub async fn create_user(
    pool: &SqlitePool,
    name: &str,
    password: &str,
    is_admin: bool,
) -> Result<User> {
    let id = Uuid::new_v4().to_string();
    let password_hash = hash_password(password)?;

    sqlx::query("INSERT INTO users (id, name, password_hash, is_admin) VALUES (?, ?, ?, ?)")
        .bind(&id)
        .bind(name)
        .bind(&password_hash)
        .bind(is_admin)
        .execute(pool)
        .await?;

    Ok(User {
        id,
        name: name.to_string(),
        password_hash,
        is_admin,
        created_at: chrono::Utc::now().to_rfc3339(),
    })
}

/// Session lifetime in seconds (24 hours by default)
const SESSION_LIFETIME_SECS: i64 = 24 * 60 * 60;

/// Authenticate user and create session
pub async fn authenticate(
    pool: &SqlitePool,
    username: &str,
    password: &str,
    device_id: &str,
    device_name: &str,
    client: &str,
) -> Result<(User, Session)> {
    let user: User = sqlx::query_as("SELECT * FROM users WHERE name = ?")
        .bind(username)
        .fetch_optional(pool)
        .await?
        .ok_or_else(|| anyhow!("User not found"))?;

    if !verify_password(password, &user.password_hash)? {
        return Err(anyhow!("Invalid password"));
    }

    let token = Uuid::new_v4().to_string();
    let now = chrono::Utc::now();
    let expires_at = now + chrono::Duration::seconds(SESSION_LIFETIME_SECS);

    sqlx::query(
        "INSERT INTO sessions (token, user_id, device_id, device_name, client, last_activity, expires_at) VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&token)
    .bind(&user.id)
    .bind(device_id)
    .bind(device_name)
    .bind(client)
    .bind(now.to_rfc3339())
    .bind(expires_at.to_rfc3339())
    .execute(pool)
    .await?;

    let session = Session {
        token,
        user_id: user.id.clone(),
        device_id: device_id.to_string(),
        device_name: device_name.to_string(),
        client: client.to_string(),
        created_at: now.to_rfc3339(),
        last_activity: now.to_rfc3339(),
        expires_at: Some(expires_at.to_rfc3339()),
    };

    Ok((user, session))
}

/// Validate session token and get user
///
/// This function:
/// 1. Checks if the session exists
/// 2. Verifies the session hasn't expired
/// 3. Updates the last_activity timestamp
/// 4. Extends expiration on activity (sliding window)
pub async fn validate_session(pool: &SqlitePool, token: &str) -> Result<User> {
    let session: Session = sqlx::query_as("SELECT * FROM sessions WHERE token = ?")
        .bind(token)
        .fetch_optional(pool)
        .await?
        .ok_or_else(|| anyhow!("Invalid session"))?;

    // Check if session has expired
    if let Some(ref expires_at) = session.expires_at {
        let expiry = chrono::DateTime::parse_from_rfc3339(expires_at)
            .map_err(|_| anyhow!("Invalid expiry timestamp"))?;
        if chrono::Utc::now() > expiry {
            // Clean up expired session
            sqlx::query("DELETE FROM sessions WHERE token = ?")
                .bind(token)
                .execute(pool)
                .await?;
            return Err(anyhow!("Session expired"));
        }
    }

    // Update last_activity and extend expiration (sliding window)
    let now = chrono::Utc::now();
    let new_expires_at = now + chrono::Duration::seconds(SESSION_LIFETIME_SECS);

    sqlx::query("UPDATE sessions SET last_activity = ?, expires_at = ? WHERE token = ?")
        .bind(now.to_rfc3339())
        .bind(new_expires_at.to_rfc3339())
        .bind(token)
        .execute(pool)
        .await?;

    let user: User = sqlx::query_as("SELECT * FROM users WHERE id = ?")
        .bind(&session.user_id)
        .fetch_one(pool)
        .await?;

    Ok(user)
}

/// Clean up expired sessions from the database
/// Returns the number of sessions removed
pub async fn cleanup_expired_sessions(pool: &SqlitePool) -> Result<i32> {
    let now = chrono::Utc::now().to_rfc3339();

    let result =
        sqlx::query("DELETE FROM sessions WHERE expires_at IS NOT NULL AND expires_at < ?")
            .bind(&now)
            .execute(pool)
            .await?;

    Ok(result.rows_affected() as i32)
}

/// Revoke a specific session
pub async fn revoke_session(pool: &SqlitePool, token: &str) -> Result<()> {
    sqlx::query("DELETE FROM sessions WHERE token = ?")
        .bind(token)
        .execute(pool)
        .await?;

    Ok(())
}

/// Revoke all sessions for a user
pub async fn revoke_all_user_sessions(pool: &SqlitePool, user_id: &str) -> Result<i32> {
    let result = sqlx::query("DELETE FROM sessions WHERE user_id = ?")
        .bind(user_id)
        .execute(pool)
        .await?;

    Ok(result.rows_affected() as i32)
}
