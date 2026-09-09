package main

import (
	"crypto/rand"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"os"
	"path/filepath"
	"strings"
	"time"
)

type Server struct {
	ID       string `json:"id"`
	Name     string `json:"name"`
	Host     string `json:"host"`
	Port     int    `json:"port"`
	User     string `json:"user"`
	Pass     string `json:"pass"`
	Token    string `json:"token,omitempty"`
	Online   bool   `json:"online"`
	LastPing time.Time `json:"last_ping"`
}

type ClusterStore struct {
	servers []Server
	dataDir string
}

func NewClusterStore(dataDir string) *ClusterStore {
	os.MkdirAll(dataDir, 0755)
	cs := &ClusterStore{dataDir: dataDir}
	cs.load()
	return cs
}

func (cs *ClusterStore) load() {
	f := filepath.Join(cs.dataDir, "cluster.json")
	data, err := os.ReadFile(f)
	if err == nil {
		json.Unmarshal(data, &cs.servers)
	}
}

func (cs *ClusterStore) save() {
	data, _ := json.MarshalIndent(cs.servers, "", "  ")
	os.WriteFile(filepath.Join(cs.dataDir, "cluster.json"), data, 0600)
}

func (cs *ClusterStore) AddServer(name, host string, port int, user, pass string) Server {
	s := Server{ID: genShortID(), Name: name, Host: host, Port: port, User: user, Pass: pass}
	cs.servers = append(cs.servers, s)
	cs.save()
	return s
}

func (cs *ClusterStore) RemoveServer(id string) {
	for i, s := range cs.servers {
		if s.ID == id {
			cs.servers = append(cs.servers[:i], cs.servers[i+1:]...)
			break
		}
	}
	cs.save()
}

func (cs *ClusterStore) ListServers() []Server { return cs.servers }

func (cs *ClusterStore) GetServer(id string) *Server {
	for i := range cs.servers {
		if cs.servers[i].ID == id {
			return &cs.servers[i]
		}
	}
	return nil
}

func (cs *ClusterStore) UpdateToken(id, token string) {
	for i := range cs.servers {
		if cs.servers[i].ID == id {
			cs.servers[i].Token = token
			cs.servers[i].Online = true
			cs.servers[i].LastPing = time.Now()
			break
		}
	}
	cs.save()
}

func (cs *ClusterStore) LoginServer(id string) error {
	s := cs.GetServer(id)
	if s == nil {
		return fmt.Errorf("server not found")
	}
	url := fmt.Sprintf("http://%s:%d/api/v1/login", s.Host, s.Port)
	body := fmt.Sprintf(`{"username":"%s","password":"%s"}`, s.User, s.Pass)
	client := &http.Client{Timeout: 5 * time.Second}
	resp, err := client.Post(url, "application/json", strings.NewReader(body))
	if err != nil {
		return err
	}
	defer resp.Body.Close()
	var result struct {
		Token string `json:"token"`
		Error string `json:"error"`
	}
	json.NewDecoder(resp.Body).Decode(&result)
	if result.Error != "" {
		return fmt.Errorf(result.Error)
	}
	cs.UpdateToken(s.ID, result.Token)
	return nil
}

func (cs *ClusterStore) CallServer(id, method, path string, body interface{}) ([]byte, error) {
	s := cs.GetServer(id)
	if s == nil {
		return nil, fmt.Errorf("server not found")
	}
	if s.Token == "" {
		if err := cs.LoginServer(id); err != nil {
			return nil, err
		}
		s = cs.GetServer(id)
	}

	url := fmt.Sprintf("http://%s:%d%s", s.Host, s.Port, path)
	var bodyReader io.Reader
	if body != nil {
		data, _ := json.Marshal(body)
		bodyReader = strings.NewReader(string(data))
	}

	req, _ := http.NewRequest(method, url, bodyReader)
	req.Header.Set("Authorization", "Bearer "+s.Token)
	req.Header.Set("Content-Type", "application/json")

	client := &http.Client{Timeout: 30 * time.Second}
	resp, err := client.Do(req)
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()

	if resp.StatusCode == 401 {
		if err := cs.LoginServer(id); err != nil {
			return nil, err
		}
		req.Header.Set("Authorization", "Bearer "+cs.GetServer(id).Token)
		resp, err = client.Do(req)
		if err != nil {
			return nil, err
		}
		defer resp.Body.Close()
	}

	return io.ReadAll(resp.Body)
}

func (cs *ClusterStore) PingServer(id string) bool {
	s := cs.GetServer(id)
	if s == nil {
		return false
	}
	url := fmt.Sprintf("http://%s:%d/api/v1/status", s.Host, s.Port)
	client := &http.Client{Timeout: 3 * time.Second}
	resp, err := client.Get(url)
	if err != nil {
		s.Online = false
		cs.save()
		return false
	}
	defer resp.Body.Close()
	s.Online = resp.StatusCode == 200 || resp.StatusCode == 401
	s.LastPing = time.Now()
	cs.save()
	return s.Online
}

func (cs *ClusterStore) PingAll() {
	for i := range cs.servers {
		cs.PingServer(cs.servers[i].ID)
	}
}

func genShortID() string {
	b := make([]byte, 8)
	rand.Read(b)
	return fmt.Sprintf("%x", b)
}

type Cluster struct {
	store *ClusterStore
}

func NewCluster(dataDir string) *Cluster {
	return &Cluster{store: NewClusterStore(dataDir)}
}

func (cl *Cluster) handleServers(w http.ResponseWriter, r *http.Request) {
	switch r.Method {
	case "GET":
		cl.store.PingAll()
		jsonResp(w, cl.store.ListServers())
	case "POST":
		var req struct {
			Name string `json:"name"`
			Host string `json:"host"`
			Port int    `json:"port"`
			User string `json:"user"`
			Pass string `json:"pass"`
		}
		if err := jsonDec(r, &req); err != nil {
			jsonErr(w, "invalid json", 400)
			return
		}
		if req.Port == 0 { req.Port = 3000 }
		s := cl.store.AddServer(req.Name, req.Host, req.Port, req.User, req.Pass)
		cl.store.LoginServer(s.ID)
		jsonResp(w, map[string]string{"status": "added", "id": s.ID})
	}
}

func (cl *Cluster) handleServer(w http.ResponseWriter, r *http.Request) {
	parts := strings.Split(strings.TrimPrefix(r.URL.Path, "/api/v1/servers/"), "/")
	if len(parts) < 1 {
		jsonErr(w, "invalid", 400)
		return
	}
	id := parts[0]

	// Proxy container operations to remote servers
	if len(parts) >= 2 {
		action := parts[1]

	// ── агрегированная сводка сервера ──
	if action == "status" && r.Method == "GET" {
		data, err := cl.store.CallServer(id, "GET", "/api/v1/status", nil)
		if err != nil {
			jsonErr(w, err.Error(), 502)
			return
		}
		w.Header().Set("Content-Type", "application/json")
		w.Write(data)
		return
	}

	// GET /servers/:id/containers
	if action == "containers" && r.Method == "GET" {
			data, err := cl.store.CallServer(id, "GET", "/api/v1/containers?show_all=true", nil)
			if err != nil {
				jsonErr(w, err.Error(), 502)
				return
			}
			w.Header().Set("Content-Type", "application/json")
			w.Write(data)
			return
		}

		// GET /servers/:id/runners
		if action == "runners" && r.Method == "GET" {
			data, err := cl.store.CallServer(id, "GET", "/api/v1/runners", nil)
			if err != nil {
				jsonErr(w, err.Error(), 502)
				return
			}
			w.Header().Set("Content-Type", "application/json")
			w.Write(data)
			return
		}

		// Proxy /servers/:id/containers/:cname/*
		if action == "containers" && len(parts) >= 3 {
			cname := parts[2]
			rest := ""
			if len(parts) > 3 {
				rest = "/" + strings.Join(parts[3:], "/")
			}

			method := r.Method
			if method == "GET" && rest == "" {
				method = "GET"
			}

			path := "/api/v1/containers/" + cname + rest

			var body interface{}
			if r.Method == "POST" || r.Method == "PUT" || r.Method == "PATCH" {
				var b map[string]interface{}
				if jsonDec(r, &b) == nil {
					body = b
				}
			}

			data, err := cl.store.CallServer(id, method, path, body)
			if err != nil {
				jsonErr(w, err.Error(), 502)
				return
			}
			w.Header().Set("Content-Type", "application/json")
			w.Write(data)
			return
		}

		// Proxy /servers/:id/runners/:rname/*
		if action == "runners" && len(parts) >= 3 {
			rname := parts[2]
			rest := ""
			if len(parts) > 3 {
				rest = "/" + strings.Join(parts[3:], "/")
			}
			path := "/api/v1/runners/" + rname + rest
			data, err := cl.store.CallServer(id, r.Method, path, nil)
			if err != nil {
				jsonErr(w, err.Error(), 502)
				return
			}
			w.Header().Set("Content-Type", "application/json")
			w.Write(data)
			return
		}
	}

	if r.Method == "DELETE" {
		cl.store.RemoveServer(id)
		jsonResp(w, map[string]string{"status": "removed"})
		return
	}

	s := cl.store.GetServer(id)
	if s == nil {
		jsonErr(w, "not found", 404)
		return
	}
	jsonResp(w, s)
}
