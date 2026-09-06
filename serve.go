package main

import (
	"encoding/json"
	"fmt"
	"log"
	"net/http"
	"os"
	"os/exec"
	"path/filepath"
	"regexp"
	"strconv"
	"strings"
)

type Hub struct {
	clients    map[chan []byte]struct{}
	broadcast  chan []byte
	register   chan chan []byte
	unregister chan chan []byte
}

func (h *Hub) Run() {
	for {
		select {
		case client := <-h.register:
			h.clients[client] = struct{}{}
		case client := <-h.unregister:
			delete(h.clients, client)
			close(client)
		case msg := <-h.broadcast:
			for client := range h.clients {
				select {
				case client <- msg:
				default:
					close(client)
					delete(h.clients, client)
				}
			}
		}
	}
}

func startWebServer(port int) {
	auth := NewAuth()
	hub := &Hub{clients: make(map[chan []byte]struct{})}
	go hub.Run()
	cl := NewCluster(filepath.Join(stackDir(), "data"))

	mux := http.NewServeMux()

	mux.HandleFunc("/api/v1/login", func(w http.ResponseWriter, r *http.Request) {
		if r.Method != "POST" {
			http.Error(w, "POST required", 405)
			return
		}
		var req struct {
			Username string `json:"username"`
			Password string `json:"password"`
		}
		json.NewDecoder(r.Body).Decode(&req)
		token := auth.Login(req.Username, req.Password)
		if token == "" {
			w.WriteHeader(401)
			json.NewEncoder(w).Encode(map[string]string{"error": "invalid credentials"})
			return
		}
		http.SetCookie(w, &http.Cookie{Name: "dt", Value: token, Path: "/", HttpOnly: true, MaxAge: 86400})
		json.NewEncoder(w).Encode(map[string]string{"status": "ok", "token": token})
	})

	authReq := func(next http.HandlerFunc) http.HandlerFunc {
		return func(w http.ResponseWriter, r *http.Request) {
			token := ""
			if c, err := r.Cookie("dt"); err == nil {
				token = c.Value
			}
			if token == "" {
				if h := r.Header.Get("Authorization"); len(h) > 7 && h[:7] == "Bearer " {
					token = h[7:]
				}
			}
			if !auth.Valid(token) {
				w.WriteHeader(401)
				json.NewEncoder(w).Encode(map[string]string{"error": "unauthorized"})
				return
			}
			next(w, r)
		}
	}

	mux.HandleFunc("/api/v1/sites", authReq(func(w http.ResponseWriter, r *http.Request) {
		if r.Method == "GET" {
			json.NewEncoder(w).Encode(listSites())
			return
		}
		if r.Method == "POST" {
			var req struct {
				Name string `json:"name"`
				Dir  string `json:"dir"`
				Port int    `json:"port"`
				Type string `json:"type"`
			}
			json.NewDecoder(r.Body).Decode(&req)
			if req.Dir == "" {
				req.Dir = "/var/www/" + req.Name
			}
			lang := detectLang(req.Dir)
			if req.Type != "" {
				lang = LangInfo{req.Type, req.Type, defaultPorts[req.Type], nil}
			}
			if req.Port == 0 {
				req.Port = freePort(lang.Port)
			}
			c := readCompose()
			c = removeService(c, req.Name)
			c = strings.TrimRight(c, "\n") + "\n" + buildServiceBlock(req.Name, req.Dir, req.Port, lang)
			writeCompose(c)
			svc := req.Name
			if lang.Name == "php" {
				svc = req.Name + "-php"
			}
			dockerCompose("up", "-d", "--build", svc)
			json.NewEncoder(w).Encode(map[string]string{"status": "deployed", "name": req.Name})
		}
	}))

	mux.HandleFunc("/api/v1/sites/", authReq(func(w http.ResponseWriter, r *http.Request) {
		name := strings.TrimPrefix(r.URL.Path, "/api/v1/sites/")
		if strings.HasSuffix(name, "/logs") {
			name = strings.TrimSuffix(name, "/logs")
			w.Header().Set("Content-Type", "text/plain")
			cmd := exec.Command("docker", "logs", "--tail", "100", name)
			out, _ := cmd.CombinedOutput()
			w.Write(out)
			return
		}
		if strings.HasSuffix(name, "/restart") {
			name = strings.TrimSuffix(name, "/restart")
			dockerCompose("restart", name)
			json.NewEncoder(w).Encode(map[string]string{"status": "restarted"})
			return
		}
		if strings.HasSuffix(name, "/remove") && r.Method == "DELETE" {
			name = strings.TrimSuffix(name, "/remove")
			for _, s := range []string{name, name + "-php"} {
				dockerCompose("down", s)
			}
			c := readCompose()
			c = removeService(c, name)
			writeCompose(c)
			json.NewEncoder(w).Encode(map[string]string{"status": "removed"})
			return
		}
		http.NotFound(w, r)
	}))

	mux.HandleFunc("/api/v1/runners", authReq(func(w http.ResponseWriter, r *http.Request) {
		names := runnerNames()
		var runners []map[string]interface{}
		for _, name := range names {
			status := "unknown"
			cmd := exec.Command("docker", "inspect", "-f", "{{.State.Status}}", name)
			if out, err := cmd.Output(); err == nil {
				status = strings.TrimSpace(string(out))
			}
			image := ""
			cmd2 := exec.Command("docker", "inspect", "-f", "{{.Config.Image}}", name)
			if out, err := cmd2.Output(); err == nil {
				image = strings.TrimSpace(string(out))
			}
			runners = append(runners, map[string]interface{}{
				"name": name, "status": status, "image": image,
			})
		}
		json.NewEncoder(w).Encode(runners)
	}))

	// Cluster management
	mux.HandleFunc("/api/v1/servers", authReq(cl.handleServers))
	mux.HandleFunc("/api/v1/servers/", authReq(cl.handleServer))

	mux.HandleFunc("/api/v1/status", authReq(func(w http.ResponseWriter, r *http.Request) {
		json.NewEncoder(w).Encode(map[string]interface{}{
			"sites":   listSites(),
			"runners": runnerNames(),
			"version": "1.2.0",
		})
	}))

	mux.HandleFunc("/api/v1/webhook/", func(w http.ResponseWriter, r *http.Request) {
		if r.Method != "POST" {
			http.Error(w, "POST required", 405)
			return
		}
		var payload struct {
			Ref        string `json:"ref"`
			Repository struct {
				Name string `json:"name"`
			} `json:"repository"`
		}
		json.NewDecoder(r.Body).Decode(&payload)
		if payload.Ref != "refs/heads/main" && payload.Ref != "refs/heads/master" {
			json.NewEncoder(w).Encode(map[string]string{"status": "skipped"})
			return
		}
		sites := listSites()
		for _, s := range sites {
			if n, ok := s["name"].(string); ok && n == payload.Repository.Name {
				dockerCompose("up", "-d", "--build", n)
				json.NewEncoder(w).Encode(map[string]string{"status": "deployed", "site": n})
				return
			}
		}
		json.NewEncoder(w).Encode(map[string]string{"status": "no matching site"})
	})

	mux.HandleFunc("/api/v1/docs", func(w http.ResponseWriter, r *http.Request) {
		serveStaticFile(w, r, "api_docs.html")
	})

	mux.HandleFunc("/", func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path != "/" {
			http.NotFound(w, r)
			return
		}
		serveStaticFile(w, r, "index.html")
	})

	addr := fmt.Sprintf("0.0.0.0:%d", port)
	log.Printf("deploy-stack v1.2.0 on :%d", port)
	log.Printf("Web:  http://localhost:%d", port)
	log.Printf("API:  http://localhost:%d/api/v1/", port)
	log.Printf("Docs: http://localhost:%d/api/v1/docs", port)
	log.Fatal(http.ListenAndServe(addr, mux))
}

func serveStaticFile(w http.ResponseWriter, r *http.Request, name string) {
	ex, _ := os.Executable()
	staticDir := filepath.Join(filepath.Dir(ex), "static")
	data, err := os.ReadFile(filepath.Join(staticDir, name))
	if err != nil {
		http.Error(w, name+" not found", 404)
		return
	}
	if strings.HasSuffix(name, ".html") {
		w.Header().Set("Content-Type", "text/html; charset=utf-8")
	} else {
		w.Header().Set("Content-Type", "text/plain")
	}
	w.Write(data)
}

func getContainerStatus(name string) string {
	cmd := exec.Command("docker", "inspect", "-f", "{{.State.Status}}", name)
	out, err := cmd.Output()
	if err != nil {
		return "stopped"
	}
	return strings.TrimSpace(string(out))
}

func jsonResp(w http.ResponseWriter, v interface{}) {
	w.Header().Set("Content-Type", "application/json")
	json.NewEncoder(w).Encode(v)
}

func jsonErr(w http.ResponseWriter, msg string, code int) {
	w.WriteHeader(code)
	w.Header().Set("Content-Type", "application/json")
	json.NewEncoder(w).Encode(map[string]string{"error": msg})
}

func jsonDec(r *http.Request, v interface{}) error {
	return json.NewDecoder(r.Body).Decode(v)
}

func listSites() []map[string]interface{} {
	seen := make(map[string]bool)
	var sites []map[string]interface{}

	// From docker-compose.yml
	content := readCompose()
	re := regexp.MustCompile(`(?m)^  (\S+):`)
	for _, m := range re.FindAllStringSubmatchIndex(content, -1) {
		name := content[m[2]:m[3]]
		if name == "runner" || strings.HasPrefix(name, "runner-") {
			continue
		}
		block := content[m[1]:]
		if next := re.FindStringIndex(block[1:]); next != nil {
			block = block[:next[0]+1]
		}
		port := 0
		if pm := regexp.MustCompile(`127\.0\.0\.1:(\d+):`).FindStringSubmatch(block); len(pm) > 1 {
			port, _ = strconv.Atoi(pm[1])
		}
		status := getContainerStatus(name)
		sites = append(sites, map[string]interface{}{
			"name": name, "port": port, "status": status,
			"type": "static", "dir": "/var/www/" + name,
		})
		seen[name] = true
	}

	// Also find running containers not in compose
	cmd := exec.Command("docker", "ps", "--format", "{{.Names}}|{{.Status}}|{{.Ports}}")
	if out, err := cmd.Output(); err == nil {
		for _, line := range strings.Split(strings.TrimSpace(string(out)), "\n") {
			parts := strings.SplitN(line, "|", 3)
			if len(parts) < 2 {
				continue
			}
			name := parts[0]
			if name == "runner" || strings.HasPrefix(name, "runner-") || strings.Contains(name, "docker_dind") || strings.Contains(name, "forgejo") {
				continue
			}
			if seen[name] {
				continue
			}
			port := 0
			if pm := regexp.MustCompile(`127\.0\.0\.1:(\d+):`).FindStringSubmatch(parts[2]); len(pm) > 1 {
				port, _ = strconv.Atoi(pm[1])
			}
			sites = append(sites, map[string]interface{}{
				"name": name, "port": port, "status": "running",
				"type": "docker", "dir": "/var/www/" + name,
			})
		}
	}

	return sites
}
