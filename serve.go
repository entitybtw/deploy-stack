package main

import (
	"context"
	"encoding/json"
	"fmt"
	"log"
	"net/http"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"time"
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

	mux.HandleFunc("/api/v1/logout", func(w http.ResponseWriter, r *http.Request) {
		token := ""
		if c, err := r.Cookie("dt"); err == nil {
			token = c.Value
		}
		if token != "" {
			auth.Logout(token)
		}
		http.SetCookie(w, &http.Cookie{Name: "dt", Value: "", Path: "/", HttpOnly: true, MaxAge: -1})
		json.NewEncoder(w).Encode(map[string]string{"status": "ok"})
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

	// ── Containers ──
	mux.HandleFunc("/api/v1/containers", authReq(func(w http.ResponseWriter, r *http.Request) {
		if r.Method == "GET" {
			showAll := r.URL.Query().Get("show_all") == "true"
			json.NewEncoder(w).Encode(listContainers(showAll))
			return
		}
	}))

	mux.HandleFunc("/api/v1/containers/", authReq(func(w http.ResponseWriter, r *http.Request) {
		name := strings.TrimPrefix(r.URL.Path, "/api/v1/containers/")
		if name == "" {
			jsonErr(w, "name required", 400)
			return
		}

		if strings.HasSuffix(name, "/logs") {
			name = strings.TrimSuffix(name, "/logs")
			tail := "200"
			if t := r.URL.Query().Get("tail"); t != "" { tail = t }
			w.Header().Set("Content-Type", "text/plain")
			cmd := exec.Command("docker", "logs", "--tail", tail, name)
			out, _ := cmd.CombinedOutput()
			w.Write(out)
			return
		}

		if strings.HasSuffix(name, "/exec") && r.Method == "POST" {
			name = strings.TrimSuffix(name, "/exec")
			var req struct {
				Cmd string `json:"cmd"`
			}
			jsonDec(r, &req)
			if req.Cmd == "" {
				jsonErr(w, "cmd required", 400)
				return
			}
			w.Header().Set("Content-Type", "text/plain")
			w.Header().Set("X-Accel-Buffering", "no")
			flusher, ok := w.(http.Flusher)
			cmd := exec.Command("docker", "exec", name, "sh", "-c", req.Cmd)
			stdout, _ := cmd.StdoutPipe()
			stderr, _ := cmd.StderrPipe()
			if err := cmd.Start(); err != nil {
				w.Write([]byte("Error: " + err.Error()))
				return
			}
			buf := make([]byte, 4096)
			go func() {
				for {
					n, err := stdout.Read(buf)
					if n > 0 {
						w.Write(buf[:n])
						if ok { flusher.Flush() }
					}
					if err != nil { break }
				}
			}()
			go func() {
				for {
					n, err := stderr.Read(buf)
					if n > 0 {
						w.Write(buf[:n])
						if ok { flusher.Flush() }
					}
					if err != nil { break }
				}
			}()
			cmd.Wait()
			return
		}

		if strings.HasSuffix(name, "/inspect") {
			name = strings.TrimSuffix(name, "/inspect")
			cmd := exec.Command("docker", "inspect", name)
			out, _ := cmd.Output()
			w.Header().Set("Content-Type", "application/json")
			w.Write(out)
			return
		}

		if strings.HasSuffix(name, "/start") {
			name = strings.TrimSuffix(name, "/start")
			cmd := exec.Command("docker", "start", name)
			cmd.Run()
			jsonResp(w, map[string]string{"status": "started"})
			return
		}

		if strings.HasSuffix(name, "/stop") {
			name = strings.TrimSuffix(name, "/stop")
			cmd := exec.Command("docker", "stop", name)
			cmd.Run()
			jsonResp(w, map[string]string{"status": "stopped"})
			return
		}

		if strings.HasSuffix(name, "/restart") {
			name = strings.TrimSuffix(name, "/restart")
			cmd := exec.Command("docker", "restart", name)
			cmd.Run()
			jsonResp(w, map[string]string{"status": "restarted"})
			return
		}

		if strings.HasSuffix(name, "/remove") && r.Method == "DELETE" {
			name = strings.TrimSuffix(name, "/remove")
			cmd := exec.Command("docker", "rm", "-f", name)
			cmd.Run()
			jsonResp(w, map[string]string{"status": "removed"})
			return
		}

		if strings.HasSuffix(name, "/terminal") {
			name = strings.TrimSuffix(name, "/terminal")
			http.Error(w, "Use WebSocket for terminal", 400)
			return
		}

		jsonErr(w, "unknown action", 404)
	}))

	// ── Sites (deploy-stack compose-managed) ──
	mux.HandleFunc("/api/v1/sites", authReq(func(w http.ResponseWriter, r *http.Request) {
		if r.Method == "GET" {
			json.NewEncoder(w).Encode(listServices())
			return
		}
		if r.Method == "POST" {
			var req struct {
				Name string `json:"name"`
				Dir  string `json:"dir"`
				Port int    `json:"port"`
				Type string `json:"type"`
			}
			jsonDec(r, &req)
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
		if strings.HasSuffix(name, "/remove") && r.Method == "DELETE" {
			name = strings.TrimSuffix(name, "/remove")
			for _, s := range []string{name, name + "-php"} {
				dockerCompose("down", s)
			}
			c := readCompose()
			c = removeService(c, name)
			writeCompose(c)
			jsonResp(w, map[string]string{"status": "removed"})
			return
		}
		jsonErr(w, "unknown", 404)
	}))

	// ── Runners ──
	mux.HandleFunc("/api/v1/runners", authReq(func(w http.ResponseWriter, r *http.Request) {
		names := runnerNames()
		var runners []map[string]interface{}
		for _, name := range names {
			status := getContainerStatus(name)
			image := ""
			cmd := exec.Command("docker", "inspect", "-f", "{{.Config.Image}}", name)
			if out, err := cmd.Output(); err == nil {
				image = strings.TrimSpace(string(out))
			}
			runners = append(runners, map[string]interface{}{
				"name": name, "status": status, "image": image,
			})
		}
		json.NewEncoder(w).Encode(runners)
	}))

	mux.HandleFunc("/api/v1/runners/add", authReq(func(w http.ResponseWriter, r *http.Request) {
		var req struct {
			Name     string `json:"name"`
			URL      string `json:"url"`
			Token    string `json:"token"`
			Labels   string `json:"labels"`
			Capacity int    `json:"capacity"`
			Platform string `json:"platform"`
		}
		if err := jsonDec(r, &req); err != nil {
			jsonErr(w, "invalid json", 400)
			return
		}
		if req.Capacity == 0 { req.Capacity = 1 }
		if req.Platform == "" { req.Platform = "forgejo" }
		addRunner(req.Name, req.Token, req.URL)
		dockerCompose("up", "-d", req.Name)
		jsonResp(w, map[string]string{"status": "created", "name": req.Name})
	}))

	mux.HandleFunc("/api/v1/runners/", authReq(func(w http.ResponseWriter, r *http.Request) {
		name := strings.TrimPrefix(r.URL.Path, "/api/v1/runners/")
		if strings.HasSuffix(name, "/logs") {
			name = strings.TrimSuffix(name, "/logs")
			w.Header().Set("Content-Type", "text/plain")
			cmd := exec.Command("docker", "logs", "--tail", "200", name)
			out, _ := cmd.CombinedOutput()
			w.Write(out)
			return
		}
		if strings.HasSuffix(name, "/restart") {
			name = strings.TrimSuffix(name, "/restart")
			dockerCompose("restart", name)
			jsonResp(w, map[string]string{"status": "restarted"})
			return
		}
		if strings.HasSuffix(name, "/stop") {
			name = strings.TrimSuffix(name, "/stop")
			dockerCompose("stop", name)
			jsonResp(w, map[string]string{"status": "stopped"})
			return
		}
		if r.Method == "DELETE" {
			dockerCompose("stop", name)
			dockerCompose("rm", name)
			jsonResp(w, map[string]string{"status": "removed"})
			return
		}
		jsonResp(w, map[string]string{"name": name})
	}))

	// ── Cluster ──
	mux.HandleFunc("/api/v1/servers", authReq(cl.handleServers))
	mux.HandleFunc("/api/v1/servers/", authReq(cl.handleServer))

	// ── Status ──
	mux.HandleFunc("/api/v1/status", authReq(func(w http.ResponseWriter, r *http.Request) {
		json.NewEncoder(w).Encode(map[string]interface{}{
			"containers": len(listContainers(false)),
			"sites":      listServices(),
			"runners":    runnerNames(),
			"version":    "1.3.0",
		})
	}))

	// ── Docs ──
	mux.HandleFunc("/api/v1/docs", authReq(func(w http.ResponseWriter, r *http.Request) {
		serveStaticFile(w, r, "api_docs.html")
	}))

	// ── Static ──
	mux.HandleFunc("/", func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path != "/" {
			http.NotFound(w, r)
			return
		}
		serveStaticFile(w, r, "index.html")
	})

	addr := fmt.Sprintf("0.0.0.0:%d", port)
	log.Printf("deploy-stack v1.3.0 on :%d", port)
	log.Printf("Web:  http://localhost:%d", port)
	log.Printf("API:  http://localhost:%d/api/v1/", port)
	log.Fatal(http.ListenAndServe(addr, mux))
}

func serveStaticFile(w http.ResponseWriter, r *http.Request, name string) {
	paths := []string{
		filepath.Join(filepath.Dir(os.Args[0]), "static", name),
		filepath.Join("/usr/local/share/deploy-stack/static", name),
		filepath.Join("/root/deploy-stack/static", name),
	}
	for _, p := range paths {
		if data, err := os.ReadFile(p); err == nil {
			ct := "text/plain"
			if strings.HasSuffix(name, ".html") {
				ct = "text/html; charset=utf-8"
			}
			w.Header().Set("Content-Type", ct)
			w.Write(data)
			return
		}
	}
	http.Error(w, name+" not found", 404)
}

func getContainerStatus(name string) string {
	ctx, cancel := context.WithTimeout(context.Background(), 3*time.Second)
	defer cancel()
	cmd := exec.CommandContext(ctx, "docker", "inspect", "-f", "{{.State.Status}}", name)
	out, err := cmd.Output()
	if err != nil {
		return "stopped"
	}
	return strings.TrimSpace(string(out))
}

func getContainerImage(name string) string {
	ctx, cancel := context.WithTimeout(context.Background(), 3*time.Second)
	defer cancel()
	cmd := exec.CommandContext(ctx, "docker", "inspect", "-f", "{{.Config.Image}}", name)
	out, err := cmd.Output()
	if err != nil { return "" }
	return strings.TrimSpace(string(out))
}

func getContainerPorts(name string) string {
	ctx, cancel := context.WithTimeout(context.Background(), 3*time.Second)
	defer cancel()
	cmd := exec.CommandContext(ctx, "docker", "inspect", "-f", "{{range $p, $conf := .NetworkSettings.Ports}}{{$p}}->{{range $conf}}{{.HostPort}}{{end}} {{end}}", name)
	out, err := cmd.Output()
	if err != nil { return "" }
	return strings.TrimSpace(string(out))
}

func listContainers(showAll bool) []map[string]interface{} {
	var containers []map[string]interface{}
	infra := map[string]bool{
		"runner": true, "docker_dind": true, "deploy-web": true,
	}

	ctx, cancel := context.WithTimeout(context.Background(), 8*time.Second)
	defer cancel()
	cmd := exec.CommandContext(ctx, "docker", "ps", "-a", "--format", "{{.Names}}|{{.Status}}")
	out, err := cmd.CombinedOutput()
	if err != nil {
		log.Printf("docker ps error: %v, output: %s", err, string(out))
		return containers
	}

	seen := make(map[string]bool)
	for _, line := range strings.Split(strings.TrimSpace(string(out)), "\n") {
		parts := strings.SplitN(line, "|", 2)
		if len(parts) < 1 { continue }
		name := parts[0]
		status := ""
		if len(parts) > 1 { status = parts[1] }
		if seen[name] { continue }
		seen[name] = true

		if !showAll && infra[name] { continue }
		if !showAll && strings.HasPrefix(name, "runner-") && strings.Contains(name, "forgejo") { continue }

		img := getContainerImage(name)
		ports := getContainerPorts(name)

		info := map[string]interface{}{
			"name":   name,
			"status": status,
			"image":  img,
			"ports":  ports,
		}

		// Check if it's a deploy-stack managed container (has /var/www mount)
		ctx2, cancel2 := context.WithTimeout(context.Background(), 3*time.Second)
		defer cancel2()
		mntCmd := exec.CommandContext(ctx2, "docker", "inspect", "-f", "{{range .Mounts}}{{.Source}}:{{.Destination}} {{end}}", name)
		if mntOut, err := mntCmd.Output(); err == nil {
			mounts := string(mntOut)
			if strings.Contains(mounts, "/var/www") {
				info["managed"] = true
			}
		}

		containers = append(containers, info)
	}

	return containers
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
