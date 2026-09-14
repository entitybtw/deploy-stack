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
	cl := NewCluster(filepath.Join(deployHome(), "data"))

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
			w.Header().Set("Content-Type", "application/json")
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

		if strings.HasSuffix(name, "/edit") {
			name = strings.TrimSuffix(name, "/edit")
			var req struct {
				Port     int    `json:"port"`
				HostPath string `json:"hostPath"`
				HostIP   string `json:"hostIP"`
				Type     string `json:"type"`
			}
			jsonDec(r, &req)
			if err := recreateContainer(name, req.Port, req.HostPath, req.HostIP, req.Type); err != nil {
				jsonErr(w, err.Error(), 400)
				return
			}
			jsonResp(w, map[string]string{"status": "updated", "name": name})
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
			if req.Name == "" {
				jsonErr(w, "name required", 400)
				return
			}
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
			actual, err := deploySite(req.Name, req.Dir, req.Port, lang)
			if err != nil {
				jsonErr(w, err.Error(), 400)
				return
			}
			json.NewEncoder(w).Encode(map[string]interface{}{"status": "deployed", "name": req.Name, "port": actual})
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
		if r.Method == "GET" {
			names := runnerNames()
			var runners []map[string]interface{}
			for _, name := range names {
				status := getContainerStatus(name)
				image := ""
				cmd := exec.Command("docker", "inspect", "-f", "{{.Config.Image}}", name)
				if out, err := cmd.Output(); err == nil {
					image = strings.TrimSpace(string(out))
				}
				// Read labels from config.yaml
				labels := ""
				configPath := composePath()
				if data, err := os.ReadFile(configPath); err == nil {
					// Try to find runner labels in config
					content := string(data)
					_ = content
				}
				// Read from .runner file for labels
				runnerFile := filepath.Join(runnerDataRoot(), "data", ".runner")
				if name != "runner" {
					runnerFile = filepath.Join(runnerDataRoot(), "data", name, ".runner")
				}
				if data, err := os.ReadFile(runnerFile); err == nil {
					var rf struct {
						Labels []string `json:"labels"`
						Name   string   `json:"name"`
					}
					if json.Unmarshal(data, &rf) == nil && len(rf.Labels) > 0 {
						labels = strings.Join(rf.Labels, ", ")
					}
				}
				runners = append(runners, map[string]interface{}{
					"name": name, "status": status, "image": image, "labels": labels,
				})
			}
			w.Header().Set("Content-Type", "application/json")
			json.NewEncoder(w).Encode(runners)
			return
		}
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
		if req.URL == "" { req.URL = "http://10.0.0.1:3000" }
		addRunner(req.Name, req.Token, req.URL)
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
			cmd := exec.Command("docker", "restart", name)
			cmd.Run()
			jsonResp(w, map[string]string{"status": "restarted"})
			return
		}
		if strings.HasSuffix(name, "/stop") {
			name = strings.TrimSuffix(name, "/stop")
			cmd := exec.Command("docker", "stop", name)
			cmd.Run()
			jsonResp(w, map[string]string{"status": "stopped"})
			return
		}
		if strings.HasSuffix(name, "/start") {
			name = strings.TrimSuffix(name, "/start")
			cmd := exec.Command("docker", "start", name)
			cmd.Run()
			jsonResp(w, map[string]string{"status": "started"})
			return
		}
		if strings.HasSuffix(name, "/edit") && r.Method == "POST" {
			name = strings.TrimSuffix(name, "/edit")
			var req struct {
				Labels string `json:"labels"`
			}
			if err := jsonDec(r, &req); err != nil {
				jsonErr(w, "invalid json", 400)
				return
			}
			// Update .runner file labels - preserve all other fields
			runnerFile := filepath.Join(runnerDataRoot(), "data", ".runner")
			if name != "runner" {
				runnerFile = filepath.Join(runnerDataRoot(), "data", name, ".runner")
			}
			data, err := os.ReadFile(runnerFile)
			if err != nil {
				jsonErr(w, "runner config not found", 404)
				return
			}
			var rf map[string]interface{}
			json.Unmarshal(data, &rf)
			rf["labels"] = strings.Split(req.Labels, ",")
			labels := rf["labels"].([]string)
			for i := range labels {
				labels[i] = strings.TrimSpace(labels[i])
			}
			rf["labels"] = labels
			newData, _ := json.MarshalIndent(rf, "", "  ")
			os.WriteFile(runnerFile, newData, 0644)
			// Restart runner to apply
			cmd := exec.Command("docker", "restart", name)
			cmd.Run()
			jsonResp(w, map[string]string{"status": "updated"})
			return
		}
		if r.Method == "DELETE" {
			cmd := exec.Command("docker", "rm", "-f", name)
			cmd.Run()
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
			"version":    "1.4.0",
		})
	}))

	// ── One-shot auto deploy (имя/каталог/порт/тип можно не указывать) ──
	mux.HandleFunc("/api/v1/deploy", authReq(func(w http.ResponseWriter, r *http.Request) {
		if r.Method != "POST" {
			jsonErr(w, "POST required", 405)
			return
		}
		var req struct {
			Name string `json:"name"`
			Dir  string `json:"dir"`
			Port int    `json:"port"`
			Type string `json:"type"`
		}
		jsonDec(r, &req)
		dir := req.Dir
		if dir == "" {
			if req.Name != "" {
				dir = "/var/www/" + req.Name
			} else {
				jsonErr(w, "name or dir required", 400)
				return
			}
		}
		lang := detectLang(dir)
		if req.Type != "" {
			lang = LangInfo{req.Type, req.Type, defaultPorts[req.Type], nil}
		}
		name := req.Name
		if name == "" {
			name = filepath.Base(dir)
		}
		if req.Port == 0 {
			req.Port = freePort(lang.Port)
		}
		actual, err := deploySite(name, dir, req.Port, lang)
		if err != nil {
			jsonErr(w, err.Error(), 400)
			return
		}
		jsonResp(w, map[string]interface{}{"status": "deployed", "name": name, "port": actual, "type": lang.Name})
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
	log.Printf("deploy-stack v1.4.0 on :%d", port)
	log.Printf("Web:  http://localhost:%d", port)
	log.Printf("API:  http://localhost:%d/api/v1/", port)
	log.Fatal(http.ListenAndServe(addr, mux))
}

func serveStaticFile(w http.ResponseWriter, r *http.Request, name string) {
	paths := []string{
		filepath.Join(deployHome(), "static", name),
		filepath.Join("/usr/local/share/deploy-stack/static", name),
		filepath.Join(filepath.Dir(os.Args[0]), "static", name),
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
	containers := make([]map[string]interface{}, 0)
	infra := map[string]bool{
		"runner": true, "docker_dind": true, "deploy-web": true,
		"deploy-stack": true, "deploy-stack-runner": true,
	}

	// Batch: get all container names and statuses
	ctx, cancel := context.WithTimeout(context.Background(), 15*time.Second)
	defer cancel()
	cmd := exec.CommandContext(ctx, "docker", "ps", "-a", "--format", "{{.Names}}|{{.Status}}|{{.Image}}")
	out, err := cmd.CombinedOutput()
	if err != nil {
		log.Printf("docker ps error: %v, output: %s", err, string(out))
		return containers
	}

	// Batch: get all mounts at once
	var mountOut []byte
	mountMap := make(map[string]bool)

	type rawContainer struct {
		Name   string
		Status string
		Image  string
	}

	var all []rawContainer
	seen := make(map[string]bool)
	for _, line := range strings.Split(strings.TrimSpace(string(out)), "\n") {
		parts := strings.SplitN(line, "|", 3)
		if len(parts) < 1 { continue }
		name := parts[0]
		status := ""
		image := ""
		if len(parts) > 1 { status = parts[1] }
		if len(parts) > 2 { image = parts[2] }
		if seen[name] { continue }
		seen[name] = true
		all = append(all, rawContainer{Name: name, Status: status, Image: image})
	}

	// Batch: determine managed containers (label deploy-stack.site или mount /var/www)
	if len(all) > 0 {
		names := make([]string, len(all))
		for i, c := range all { names[i] = c.Name }
		ctx2, cancel2 := context.WithTimeout(context.Background(), 10*time.Second)
		defer cancel2()
		inspectCmd := exec.CommandContext(ctx2, "docker", "inspect", "--format",
			`{{.Name}}|{{index .Config.Labels "deploy-stack.site"}}|{{range .Mounts}}{{.Source}}:{{.Destination}}|{{end}}`)
		inspectCmd.Args = append(inspectCmd.Args, names...)
		mountOut, _ = inspectCmd.CombinedOutput()
		for _, line := range strings.Split(strings.TrimSpace(string(mountOut)), "\n") {
			parts := strings.SplitN(line, "|", 3)
			if len(parts) < 3 { continue }
			name := strings.TrimPrefix(parts[0], "/")
			label := strings.TrimSpace(parts[1])
			mounts := parts[2]
			if label != "" || strings.Contains(mounts, "/var/www") {
				mountMap[name] = true
			}
		}
	}

	for _, c := range all {
		if !showAll && infra[c.Name] { continue }
		if !showAll && strings.HasPrefix(c.Name, "runner-") { continue }
		if !showAll {
			if _, ok := mountMap[c.Name]; !ok { continue }
		}

		ports := ""
		ctx3, cancel3 := context.WithTimeout(context.Background(), 3*time.Second)
		portCmd := exec.CommandContext(ctx3, "docker", "inspect", "--format",
			`{{range $p, $conf := .NetworkSettings.Ports}}{{$p}}->{{if $conf}}{{with index $conf 0}}{{.HostPort}}{{end}}{{end}} {{end}}`, c.Name)
		portOut, _ := portCmd.CombinedOutput()
		cancel3()
		ports = strings.TrimSpace(string(portOut))

		containers = append(containers, map[string]interface{}{
			"name":    c.Name,
			"status":  c.Status,
			"image":   c.Image,
			"ports":   ports,
			"managed": mountMap[c.Name],
		})
	}

	return containers
}

// recreateContainer пересоздаёт managed-контейнер сайта с новым внешним портом,
// хост-IP и/или папкой источников. Вызывается из панели при редактировании.
func recreateContainer(name string, newPort int, hostPath, hostIP, typ string) error {
	ctx, cancel := context.WithTimeout(context.Background(), 20*time.Second)
	defer cancel()

	type mountInfo struct{ Src, Dst string; RW bool }

	// 1. Получаем образ, сборку bind-маунтов и внутренний порт.
	get := func(sep string, args ...string) string {
		out, _ := exec.CommandContext(ctx, "docker", append([]string{"inspect", "-f", sep}, args...)...).Output()
		return strings.TrimSpace(string(out))
	}
	image := get("{{.Config.Image}}", name)
	if image == "" {
		return fmt.Errorf("контейнер не найден")
	}

	// внутренний порт по .Config.ExposedPorts
	exposedStr := get("{{range $p, $_ := .Config.ExposedPorts}}{{$p}} {{end}}", name)
	innerPort := "80"
	for _, e := range strings.Fields(exposedStr) {
		if strings.HasPrefix(e, "80") { innerPort = "80"; break }
		if p := strings.Split(e, "/")[0]; p != "" { innerPort = p; break }
	}

	// bind-маунты host-папки (Type=bind)
	var binds []mountInfo
	mountsJSON := get("{{json .Mounts}}", name)
	if mountsJSON != "" {
		var ms []struct {
			Type   string `json:"Type"`
			Source string `json:"Source"`
			Dest   string `json:"Destination"`
			RW     bool   `json:"RW"`
		}
		if json.Unmarshal([]byte(mountsJSON), &ms) == nil {
			for _, m := range ms {
				if m.Type != "bind" { continue }
				src := m.Source
				if hostPath != "" && (m.Dest == "/usr/share/nginx/html" || m.Dest == "/var/www/html") {
					src = hostPath
				}
				binds = append(binds, mountInfo{Src: src, Dst: m.Dest, RW: m.RW})
			}
		}
	}

	// внешний порт по умолчанию из старых биндингов если не передали
	hostPort := newPort
	if hostPort == 0 {
		pb := get("{{json .NetworkSettings.Ports}}", name)
		if pb != "" {
			var ports map[string][]struct{ HostPort string `json:"HostPort"` }
			if json.Unmarshal([]byte(pb), &ports) == nil {
				for _, arr := range ports {
					if len(arr) > 0 { fmt.Sscanf(arr[0].HostPort, "%d", &hostPort); break }
				}
			}
		}
		if hostPort == 0 { hostPort = 8080 }
	}
	if hostIP == "" { hostIP = "0.0.0.0" }

	_ = typ
	if len(binds) == 0 {
		// не смогли найти bind — откат к стандартному nginx корню
		binds = append(binds, mountInfo{Src: "", Dst: "/usr/share/nginx/html", RW: false})
		return fmt.Errorf("не удалось определить каталог-источник контейнера")
	}
	// на всякий: если не передали папку, сохраняем существующую
	for i := range binds {
		if binds[i].Src == "" {
			return fmt.Errorf("не удалось определить каталог-источник контейнера")
		}
	}

	// 2. удаляем старый
	exec.CommandContext(ctx, "docker", "rm", "-f", name).Run()

	// 3. пересоздаём
	args := []string{"run", "-d", "--name", name, "--restart", "unless-stopped"}
	args = append(args, "-p", fmt.Sprintf("%s:%d:%s", hostIP, hostPort, innerPort))
	for _, b := range binds {
		roSuffix := ""
		if !b.RW { roSuffix = ":ro" }
		args = append(args, "-v", b.Src+":"+b.Dst+roSuffix)
	}
	args = append(args, image)

	out, err := exec.CommandContext(ctx, "docker", args...).CombinedOutput()
	if err != nil {
		return fmt.Errorf("не удалось пересоздать: %s: %s", err, strings.TrimSpace(string(out)))
	}
	return nil
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
