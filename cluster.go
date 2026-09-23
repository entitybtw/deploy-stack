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
	"sync"
	"time"
)

type Server struct {
	ID       string    `json:"id"`
	Name     string    `json:"name"`
	Host     string    `json:"host"`
	Port     int       `json:"port"`
	User     string    `json:"user"`
	Pass     string    `json:"pass"`
	Token    string    `json:"token,omitempty"`
	Online   bool      `json:"online"`
	LastPing time.Time `json:"last_ping"`
}

type ClusterStore struct {
	mu      sync.Mutex
	servers []Server
	dataDir string
}

func NewClusterStore(dataDir string) *ClusterStore {
	os.MkdirAll(dataDir, 0755)
	cs := &ClusterStore{dataDir: dataDir}
	cs.load()
	return cs
}

func (cs *ClusterStore) path() string {
	return filepath.Join(cs.dataDir, "cluster.json")
}

func (cs *ClusterStore) load() {
	data, err := os.ReadFile(cs.path())
	if err == nil {
		json.Unmarshal(data, &cs.servers)
	}
}

// save пишет атомарно: tmp + rename, чтобы параллельные запросы
// не портили cluster.json при обрыве записи.
func (cs *ClusterStore) save() {
	cs.mu.Lock()
	defer cs.mu.Unlock()
	cs.saveLocked()
}

func (cs *ClusterStore) saveLocked() {
	data, _ := json.MarshalIndent(cs.servers, "", "  ")
	p := cs.path()
	tmp := p + ".tmp"
	if err := os.WriteFile(tmp, data, 0600); err != nil {
		return
	}
	os.Rename(tmp, p)
}

func (cs *ClusterStore) snapshot() []Server {
	cs.mu.Lock()
	defer cs.mu.Unlock()
	out := make([]Server, len(cs.servers))
	copy(out, cs.servers)
	return out
}

func (cs *ClusterStore) AddServer(name, host string, port int, user, pass string) Server {
	cs.mu.Lock()
	s := Server{ID: genShortID(), Name: name, Host: host, Port: port, User: user, Pass: pass}
	cs.servers = append(cs.servers, s)
	cs.saveLocked()
	cs.mu.Unlock()
	return s
}

func (cs *ClusterStore) UpdateServer(id, name, host string, port int, user, pass string) bool {
	cs.mu.Lock()
	defer cs.mu.Unlock()
	for i := range cs.servers {
		if cs.servers[i].ID == id {
			s := &cs.servers[i]
			if name != "" {
				s.Name = name
			}
			if host != "" {
				s.Host = host
			}
			if port > 0 {
				s.Port = port
			}
			if user != "" {
				s.User = user
			}
			// пустой pass = «не менять»
			if pass != "" {
				s.Pass = pass
			}
			// учётка изменилась — старый токен невалиден
			s.Token = ""
			cs.saveLocked()
			return true
		}
	}
	return false
}

func (cs *ClusterStore) RemoveServer(id string) {
	cs.mu.Lock()
	defer cs.mu.Unlock()
	for i, s := range cs.servers {
		if s.ID == id {
			cs.servers = append(cs.servers[:i], cs.servers[i+1:]...)
			break
		}
	}
	cs.saveLocked()
}

func (cs *ClusterStore) ListServers() []Server {
	out := cs.snapshot()
	if out == nil {
		return []Server{}
	}
	return out
}

func (cs *ClusterStore) GetServer(id string) *Server {
	cs.mu.Lock()
	defer cs.mu.Unlock()
	for i := range cs.servers {
		if cs.servers[i].ID == id {
			s := cs.servers[i]
			return &s
		}
	}
	return nil
}

func (cs *ClusterStore) UpdateToken(id, token string) {
	cs.mu.Lock()
	defer cs.mu.Unlock()
	for i := range cs.servers {
		if cs.servers[i].ID == id {
			cs.servers[i].Token = token
			cs.servers[i].Online = true
			cs.servers[i].LastPing = time.Now()
			break
		}
	}
	cs.saveLocked()
}

func (cs *ClusterStore) setOnline(id string, online bool) {
	cs.mu.Lock()
	defer cs.mu.Unlock()
	for i := range cs.servers {
		if cs.servers[i].ID == id {
			cs.servers[i].Online = online
			cs.servers[i].LastPing = time.Now()
			break
		}
	}
	cs.saveLocked()
}

func (cs *ClusterStore) LoginServer(id string) error {
	s := cs.GetServer(id)
	if s == nil {
		return fmt.Errorf("server not found")
	}
	url := fmt.Sprintf("http://%s:%d/api/v1/login", s.Host, s.Port)
	body := fmt.Sprintf(`{"username":%q,"password":%q}`, s.User, s.Pass)
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
		return fmt.Errorf("%s", result.Error)
	}
	if result.Token == "" {
		return fmt.Errorf("login failed")
	}
	cs.UpdateToken(s.ID, result.Token)
	return nil
}

// CallServerStream — как CallServer, но отдаёт ответ в w со стримингом (flush).
// Нужно для exec/логов, чтобы вывод шёл в реальном времени.
func (cs *ClusterStore) CallServerStream(id, method, path string, body interface{}, w http.ResponseWriter) error {
	s := cs.GetServer(id)
	if s == nil {
		return fmt.Errorf("server not found")
	}
	if s.Token == "" {
		if err := cs.LoginServer(id); err != nil {
			return err
		}
		s = cs.GetServer(id)
		if s == nil {
			return fmt.Errorf("server not found")
		}
	}

	call := func(token string) (*http.Response, error) {
		url := fmt.Sprintf("http://%s:%d%s", s.Host, s.Port, path)
		var bodyReader io.Reader
		if body != nil {
			data, _ := json.Marshal(body)
			bodyReader = strings.NewReader(string(data))
		}
		req, err := http.NewRequest(method, url, bodyReader)
		if err != nil {
			return nil, err
		}
		req.Header.Set("Authorization", "Bearer "+token)
		req.Header.Set("Content-Type", "application/json")
		// без таймаута — exec может работать долго
		client := &http.Client{}
		return client.Do(req)
	}

	resp, err := call(s.Token)
	if err != nil {
		return err
	}
	if resp.StatusCode == 401 {
		resp.Body.Close()
		if err := cs.LoginServer(id); err != nil {
			return err
		}
		s = cs.GetServer(id)
		if s == nil {
			return fmt.Errorf("server not found")
		}
		resp, err = call(s.Token)
		if err != nil {
			return err
		}
	}
	defer resp.Body.Close()

	if resp.StatusCode >= 400 {
		data, _ := io.ReadAll(resp.Body)
		var e struct {
			Error string `json:"error"`
		}
		if json.Unmarshal(data, &e) == nil && e.Error != "" {
			return fmt.Errorf("%s", e.Error)
		}
		return fmt.Errorf("remote HTTP %d", resp.StatusCode)
	}

	if ct := resp.Header.Get("Content-Type"); ct != "" {
		w.Header().Set("Content-Type", ct)
	} else {
		w.Header().Set("Content-Type", "text/plain")
	}
	w.Header().Set("X-Accel-Buffering", "no")
	flusher, _ := w.(http.Flusher)

	buf := make([]byte, 4096)
	for {
		n, err := resp.Body.Read(buf)
		if n > 0 {
			w.Write(buf[:n])
			if flusher != nil {
				flusher.Flush()
			}
		}
		if err != nil {
			break
		}
	}
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
		if s == nil {
			return nil, fmt.Errorf("server not found")
		}
	}

	call := func(token string) (*http.Response, error) {
		url := fmt.Sprintf("http://%s:%d%s", s.Host, s.Port, path)
		var bodyReader io.Reader
		if body != nil {
			data, _ := json.Marshal(body)
			bodyReader = strings.NewReader(string(data))
		}
		req, err := http.NewRequest(method, url, bodyReader)
		if err != nil {
			return nil, err
		}
		req.Header.Set("Authorization", "Bearer "+token)
		req.Header.Set("Content-Type", "application/json")
		client := &http.Client{Timeout: 30 * time.Second}
		return client.Do(req)
	}

	resp, err := call(s.Token)
	if err != nil {
		return nil, err
	}

	if resp.StatusCode == 401 {
		resp.Body.Close()
		if err := cs.LoginServer(id); err != nil {
			return nil, err
		}
		s = cs.GetServer(id)
		if s == nil {
			return nil, fmt.Errorf("server not found")
		}
		resp, err = call(s.Token)
		if err != nil {
			return nil, err
		}
	}
	defer resp.Body.Close()

	data, err := io.ReadAll(resp.Body)
	if err != nil {
		return nil, err
	}
	if resp.StatusCode >= 400 {
		var e struct {
			Error string `json:"error"`
		}
		if json.Unmarshal(data, &e) == nil && e.Error != "" {
			return data, fmt.Errorf("%s", e.Error)
		}
		return data, fmt.Errorf("remote HTTP %d", resp.StatusCode)
	}
	return data, nil
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
		cs.setOnline(id, false)
		return false
	}
	resp.Body.Close()
	online := resp.StatusCode == 200 || resp.StatusCode == 401
	cs.setOnline(id, online)
	return online
}

// PingAll пингует все ноды параллельно — иначе Refresh в UI
// ждёт sum(timeout) при нескольких offline-серверах.
func (cs *ClusterStore) PingAll() {
	servers := cs.snapshot()
	var wg sync.WaitGroup
	for _, s := range servers {
		wg.Add(1)
		go func(id string) {
			defer wg.Done()
			cs.PingServer(id)
		}(s.ID)
	}
	wg.Wait()
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

// handleStatus — агрегатная сводка кластера: все ноды параллельно,
// у каждой online + счётчики sites/containers/runners.
func (cl *Cluster) handleStatus(w http.ResponseWriter, r *http.Request) {
	if r.Method != "GET" {
		jsonErr(w, "GET required", 405)
		return
	}
	servers := cl.store.ListServers()
	type nodeStatus struct {
		ID         string    `json:"id"`
		Name       string    `json:"name"`
		Host       string    `json:"host"`
		Port       int       `json:"port"`
		Online     bool      `json:"online"`
		LastPing   time.Time `json:"last_ping"`
		Error      string    `json:"error,omitempty"`
		Containers int       `json:"containers"`
		Sites      []string  `json:"sites"`
		Runners    []string  `json:"runners"`
		Version    string    `json:"version,omitempty"`
	}
	out := make([]nodeStatus, len(servers))
	var wg sync.WaitGroup
	for i, s := range servers {
		wg.Add(1)
		go func(i int, s Server) {
			defer wg.Done()
			ns := nodeStatus{
				ID: s.ID, Name: s.Name, Host: s.Host, Port: s.Port,
				Online: s.Online, LastPing: s.LastPing,
				Sites: []string{}, Runners: []string{},
			}
			data, err := cl.store.CallServer(s.ID, "GET", "/api/v1/status", nil)
			if err != nil {
				ns.Online = false
				ns.Error = err.Error()
				out[i] = ns
				return
			}
			var st struct {
				Containers int      `json:"containers"`
				Sites      []string `json:"sites"`
				Runners    []string `json:"runners"`
				Version    string   `json:"version"`
			}
			if json.Unmarshal(data, &st) == nil {
				ns.Online = true
				ns.Containers = st.Containers
				if st.Sites != nil {
					ns.Sites = st.Sites
				}
				if st.Runners != nil {
					ns.Runners = st.Runners
				}
				ns.Version = st.Version
			}
			out[i] = ns
		}(i, s)
	}
	wg.Wait()
	online := 0
	for _, n := range out {
		if n.Online {
			online++
		}
	}
	jsonResp(w, map[string]interface{}{
		"nodes":   out,
		"total":   len(out),
		"online":  online,
		"version": "1.5.3",
	})
}

func (cl *Cluster) handleServers(w http.ResponseWriter, r *http.Request) {
	switch r.Method {
	case "GET":
		// быстрый список: пинг в фоне, UI обновит статусы отдельным refresh
		servers := cl.store.ListServers()
		jsonResp(w, servers)
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
		if req.Name == "" || req.Host == "" {
			jsonErr(w, "name and host required", 400)
			return
		}
		if req.Port == 0 {
			req.Port = 3000
		}
		s := cl.store.AddServer(req.Name, req.Host, req.Port, req.User, req.Pass)
		// пробный login — но не блокируем добавление при ошибке
		loginErr := ""
		if err := cl.store.LoginServer(s.ID); err != nil {
			loginErr = err.Error()
		}
		resp := map[string]string{"status": "added", "id": s.ID}
		if loginErr != "" {
			resp["warning"] = loginErr
		}
		jsonResp(w, resp)
	}
}

func (cl *Cluster) handleServer(w http.ResponseWriter, r *http.Request) {
	parts := strings.Split(strings.TrimPrefix(r.URL.Path, "/api/v1/servers/"), "/")
	if len(parts) < 1 || parts[0] == "" {
		jsonErr(w, "invalid", 400)
		return
	}
	id := parts[0]

	// ── агрегированные действия над одной нодой ──
	if len(parts) >= 2 {
		action := parts[1]

		if action == "ping" && r.Method == "GET" {
			ok := cl.store.PingServer(id)
			s := cl.store.GetServer(id)
			jsonResp(w, map[string]interface{}{"online": ok, "server": s})
			return
		}

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

		// GET /servers/:id/sites — compose-сервисы удалённой ноды
		if action == "sites" && r.Method == "GET" {
			data, err := cl.store.CallServer(id, "GET", "/api/v1/sites", nil)
			if err != nil {
				jsonErr(w, err.Error(), 502)
				return
			}
			w.Header().Set("Content-Type", "application/json")
			w.Write(data)
			return
		}

		// POST /servers/:id/deploy — деплой сайта на удалённую ноду
		if action == "deploy" && r.Method == "POST" {
			var body map[string]interface{}
			if err := jsonDec(r, &body); err != nil {
				jsonErr(w, "invalid json", 400)
				return
			}
			data, err := cl.store.CallServer(id, "POST", "/api/v1/sites", body)
			if err != nil {
				jsonErr(w, err.Error(), 502)
				return
			}
			w.Header().Set("Content-Type", "application/json")
			w.Write(data)
			return
		}

		// GET /servers/:id/sites/:sname/logs — логи сайта по compose-сервису
		if action == "sites" && len(parts) >= 4 && parts[3] == "logs" && r.Method == "GET" {
			sname := parts[2]
			tail := r.URL.Query().Get("tail")
			if tail == "" {
				tail = "300"
			}
			if err := cl.store.CallServerStream(id, "GET", "/api/v1/containers/"+sname+"/logs?tail="+tail, nil, w); err != nil {
				w.Write([]byte("Error: " + err.Error()))
			}
			return
		}

		// Proxy /servers/:id/containers/:cname/*
		if action == "containers" && len(parts) >= 3 {
			cname := parts[2]
			rest := ""
			if len(parts) > 3 {
				rest = "/" + strings.Join(parts[3:], "/")
			}

			path := "/api/v1/containers/" + cname + rest

			var body interface{}
			if r.Method == "POST" || r.Method == "PUT" || r.Method == "PATCH" {
				var b map[string]interface{}
				if jsonDec(r, &b) == nil {
					body = b
				}
			}

			// exec/logs — стримим в реальном времени
			if strings.HasSuffix(rest, "/logs") || strings.HasSuffix(rest, "/exec") {
				if err := cl.store.CallServerStream(id, r.Method, path, body, w); err != nil {
					// заголовки могли уже уйти — дописываем текст ошибки
					w.Write([]byte("Error: " + err.Error()))
				}
				return
			}

			data, err := cl.store.CallServer(id, r.Method, path, body)
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

			var body interface{}
			if r.Method == "POST" || r.Method == "PUT" || r.Method == "PATCH" {
				var b map[string]interface{}
				if jsonDec(r, &b) == nil {
					body = b
				}
			}
			// логи раннера — стримим
			if strings.HasSuffix(rest, "/logs") {
				if err := cl.store.CallServerStream(id, r.Method, path, body, w); err != nil {
					w.Write([]byte("Error: " + err.Error()))
				}
				return
			}

			data, err := cl.store.CallServer(id, r.Method, path, body)
			if err != nil {
				jsonErr(w, err.Error(), 502)
				return
			}
			w.Header().Set("Content-Type", "application/json")
			w.Write(data)
			return
		}

		// POST /servers/:id/containers/:cname/exec — уже покрыто CallServerStream выше
	}

	if r.Method == "DELETE" {
		cl.store.RemoveServer(id)
		jsonResp(w, map[string]string{"status": "removed"})
		return
	}

	// PUT — редактирование учётки/адреса ноды
	if r.Method == "PUT" {
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
		if !cl.store.UpdateServer(id, req.Name, req.Host, req.Port, req.User, req.Pass) {
			jsonErr(w, "not found", 404)
			return
		}
		// пробный re-login
		warning := ""
		if err := cl.store.LoginServer(id); err != nil {
			warning = err.Error()
		}
		resp := map[string]string{"status": "updated"}
		if warning != "" {
			resp["warning"] = warning
		}
		jsonResp(w, resp)
		return
	}

	s := cl.store.GetServer(id)
	if s == nil {
		jsonErr(w, "not found", 404)
		return
	}
	jsonResp(w, s)
}
