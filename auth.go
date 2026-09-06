package main

import (
	"crypto/rand"
	"encoding/hex"
	"os"
	"sync"
	"time"
)

type Auth struct {
	mu       sync.RWMutex
	sessions map[string]*Session
	Password string
	Username string
}

type Session struct {
	Token   string
	Created time.Time
}

func NewAuth() *Auth {
	return &Auth{
		sessions: make(map[string]*Session),
		Password: getEnv("DEPLOY_ADMIN_PASS", "admin"),
		Username: getEnv("DEPLOY_ADMIN_USER", "admin"),
	}
}

func (a *Auth) Login(user, pass string) string {
	if user != a.Username || pass != a.Password {
		return ""
	}
	token := genToken()
	a.mu.Lock()
	a.sessions[token] = &Session{Token: token, Created: time.Now()}
	a.mu.Unlock()
	return token
}

func (a *Auth) Valid(token string) bool {
	a.mu.RLock()
	s, ok := a.sessions[token]
	a.mu.RUnlock()
	if !ok {
		return false
	}
	if time.Since(s.Created) > 24*time.Hour {
		a.mu.Lock()
		delete(a.sessions, token)
		a.mu.Unlock()
		return false
	}
	return true
}

func (a *Auth) Logout(token string) {
	a.mu.Lock()
	delete(a.sessions, token)
	a.mu.Unlock()
}

func genToken() string {
	b := make([]byte, 32)
	rand.Read(b)
	return hex.EncodeToString(b)
}

func getEnv(key, def string) string {
	if v := os.Getenv(key); v != "" {
		return v
	}
	return def
}
