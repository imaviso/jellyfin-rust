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

    sqlx::query(
        "INSERT INTO sessions (token, user_id, device_id, device_name, client) VALUES (?, ?, ?, ?, ?)",
    )
    .bind(&token)
    .bind(&user.id)
    .bind(device_id)
    .bind(device_name)
    .bind(client)
    .execute(pool)
    .await?;

    let session = Session {
        token,
        user_id: user.id.clone(),
        device_id: device_id.to_string(),
        device_name: device_name.to_string(),
        client: client.to_string(),
        created_at: chrono::Utc::now().to_rfc3339(),
    };

    Ok((user, session))
}

/// Validate session token and get user
pub async fn validate_session(pool: &SqlitePool, token: &str) -> Result<User> {
    let session: Session = sqlx::query_as("SELECT * FROM sessions WHERE token = ?")
        .bind(token)
        .fetch_optional(pool)
        .await?
        .ok_or_else(|| anyhow!("Invalid session"))?;

    let user: User = sqlx::query_as("SELECT * FROM users WHERE id = ?")
        .bind(&session.user_id)
        .fetch_one(pool)
        .await?;

    Ok(user)
}
