///! Session management

use horcrux_common::auth::Session;
use uuid::Uuid;

/// Session manager
pub struct SessionManager {
    session_duration: i64, // Session duration in seconds
}

impl SessionManager {
    pub fn new() -> Self {
        Self {
            session_duration: 7200, // 2 hours default
        }
    }

    /// Create a new session
    pub async fn create_session(&self, user_id: &str, username: &str, realm: &str) -> Session {
        let session_id = Uuid::new_v4().to_string();
        let now = chrono::Utc::now();
        let now_timestamp = now.timestamp();

        Session {
            id: session_id.clone(),
            user_id: user_id.to_string(),
            expires_at: now + chrono::Duration::seconds(self.session_duration),
            session_id,
            username: username.to_string(),
            realm: realm.to_string(),
            created: now_timestamp,
            expires: now_timestamp + self.session_duration,
        }
    }

    /// Generate CSRF token
    pub fn generate_csrf_token(&self) -> String {
        Uuid::new_v4().to_string()
    }

    /// Extend session expiration
    pub fn extend_session(&self, session: &mut Session) {
        let now = chrono::Utc::now().timestamp();
        session.expires = now + self.session_duration;
    }
}
