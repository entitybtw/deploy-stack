// auth — сессии панели: логин/пароль из env, токены на 24ч, cookie dt.
use crate::util::{gen_token, get_env};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub struct Auth {
    sessions: Mutex<HashMap<String, Instant>>,
    pub password: String,
    pub username: String,
}

impl Auth {
    pub fn new() -> Self {
        Auth {
            sessions: Mutex::new(HashMap::new()),
            password: get_env("DEPLOY_ADMIN_PASS", "admin"),
            username: get_env("DEPLOY_ADMIN_USER", "admin"),
        }
    }

    /// Login — проверка учётки, выдача токена (пустой = ошибка).
    pub fn login(&self, user: &str, pass: &str) -> String {
        if user != self.username || pass != self.password {
            return String::new();
        }
        let token = gen_token();
        if let Ok(mut m) = self.sessions.lock() {
            m.insert(token.clone(), Instant::now());
        }
        token
    }

    /// Valid — активна ли сессия (24 часа).
    pub fn valid(&self, token: &str) -> bool {
        if token.is_empty() {
            return false;
        }
        let mut expired = false;
        let ok = {
            let m = match self.sessions.lock() {
                Ok(m) => m,
                Err(_) => return false,
            };
            match m.get(token) {
                Some(t) => {
                    if Instant::now().duration_since(*t) > Duration::from_secs(24 * 3600) {
                        expired = true;
                    }
                    !expired
                }
                None => false,
            }
        };
        if expired {
            if let Ok(mut m) = self.sessions.lock() {
                m.remove(token);
            }
        }
        ok
    }

    pub fn logout(&self, token: &str) {
        if let Ok(mut m) = self.sessions.lock() {
            m.remove(token);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn login_and_valid() {
        let a = Auth::new();
        let t = a.login(&a.username.clone(), &a.password.clone());
        assert!(!t.is_empty());
        assert!(a.valid(&t));
        assert!(!a.login("admin", "wrong").is_empty() == false);
        a.logout(&t);
        assert!(!a.valid(&t));
    }
}
