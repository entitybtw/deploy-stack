package main

import (
	"os"
	"path/filepath"
	"reflect"
	"strings"
	"testing"
)

// ── SYNC_EXCLUDE: разбор .deploy ──────────────────────────────

func TestSyncExcludeTokens(t *testing.T) {
	want := []string{"src/", "Cargo.toml", "Cargo.lock", "Dockerfile", "*.md", ".forgejo/"}
	cfg := map[string]string{
		"SYNC_EXCLUDE": "src/ Cargo.toml Cargo.lock Dockerfile *.md .forgejo/",
	}
	got := syncExcludeTokens(cfg)
	if !reflect.DeepEqual(got, want) {
		t.Errorf("tokens = %q, want %q", got, want)
	}
}

func TestSyncExcludeTokensNoTransformations(t *testing.T) {
	// Токены уходят как есть: без смены регистра, без срезания /
	// и без разворачивания глобов.
	cfg := map[string]string{"SYNC_EXCLUDE": "SRC/ README.MD **/x [ab].txt"}
	want := []string{"SRC/", "README.MD", "**/x", "[ab].txt"}
	if got := syncExcludeTokens(cfg); !reflect.DeepEqual(got, want) {
		t.Errorf("tokens = %q, want %q", got, want)
	}
}

func TestSyncExcludeTokensMissingOrEmpty(t *testing.T) {
	if got := syncExcludeTokens(map[string]string{"NAME": "x"}); got != nil {
		t.Errorf("нет ключа: got %q, want nil", got)
	}
	if got := syncExcludeTokens(map[string]string{"SYNC_EXCLUDE": "   "}); got != nil {
		t.Errorf("пустое значение: got %q, want nil", got)
	}
}

func TestLoadDeployConfigSyncExclude(t *testing.T) {
	dir := t.TempDir()
	deploy := "NAME=foo\n" +
		"PORT=8084\n" +
		"SYNC_EXCLUDE=src/ Cargo.toml *.md .forgejo/\n"
	if err := os.WriteFile(filepath.Join(dir, ".deploy"), []byte(deploy), 0644); err != nil {
		t.Fatal(err)
	}
	cfg := loadDeployConfig(dir)
	want := []string{"src/", "Cargo.toml", "*.md", ".forgejo/"}
	if got := syncExcludeTokens(cfg); !reflect.DeepEqual(got, want) {
		t.Errorf("tokens from .deploy = %q, want %q", got, want)
	}
	if cfg["NAME"] != "foo" || cfg["PORT"] != "8084" {
		t.Errorf("прочие ключи сломаны: %q", cfg)
	}
}

// ── rsync-аргументы ───────────────────────────────────────────

// Базовый список обязан совпадать с тем, что было до появления SYNC_EXCLUDE
// (ключ отсутствует → поведение как раньше).
func TestRsyncArgsBaseUnchanged(t *testing.T) {
	want := []string{"-a", "--delete",
		"--exclude=.git", "--exclude=*.db", "--exclude=*.sqlite", "--exclude=*.sqlite3",
		"--exclude=*.db-wal", "--exclude=*.db-shm",
		"--exclude=*.sqlite-wal", "--exclude=*.sqlite-shm",
		"--exclude=.deploy.Dockerfile",
		"--exclude=target/",
		"--exclude=.env", "--exclude=*.env", "--exclude=.env.*",
		"--exclude=data/", "--exclude=uploads/", "--exclude=upload/", "--exclude=.ssh/",
		"--exclude=cache/",
		"--exclude=/privacy.html", "--exclude=/terms.html",
		"--exclude=/site_data.js", "--exclude=/site_config.json",
		"--exclude=/access_requests.json",
		"--exclude=goserver/",
		"/repo/", "/var/www/site/"}
	got := rsyncArgs("/repo", "/var/www/site", nil)
	if !reflect.DeepEqual(got, want) {
		t.Errorf("rsyncArgs(nil) =\n%q\nwant\n%q", got, want)
	}
}

func TestRsyncArgsExtraExcludes(t *testing.T) {
	extra := []string{"src/", "Cargo.toml", "Cargo.lock", "Dockerfile", "*.md", ".forgejo/"}
	got := rsyncArgs("/repo", "/var/www/site", extra)

	for _, tok := range extra {
		found := false
		for _, a := range got {
			if a == "--exclude="+tok {
				found = true
				break
			}
		}
		if !found {
			t.Errorf("нет --exclude=%s в %q", tok, got)
		}
	}

	// Доп. исключения идут поверх базового списка, но перед путями.
	base := rsyncArgs("/repo", "/var/www/site", nil)
	if len(got) != len(base)+len(extra) {
		t.Errorf("длина %d, want %d", len(got), len(base)+len(extra))
	}
	firstExtra := -1
	lastBase := -1
	for i, a := range got {
		if a == "--exclude=goserver/" {
			lastBase = i
		}
		for _, tok := range extra {
			if a == "--exclude="+tok && firstExtra == -1 {
				firstExtra = i
			}
		}
	}
	if lastBase == -1 || firstExtra == -1 || firstExtra < lastBase {
		t.Errorf("extra не после базового списка: lastBase=%d firstExtra=%d", lastBase, firstExtra)
	}
	if got[len(got)-2] != "/repo/" || got[len(got)-1] != "/var/www/site/" {
		t.Errorf("пути не в конце: %q", got)
	}
}

func TestRsyncArgsNoDeleteExcluded(t *testing.T) {
	for _, args := range [][]string{
		rsyncArgs("/repo", "/dst", nil),
		rsyncArgs("/repo", "/dst", []string{"src/", "*.md"}),
	} {
		for _, a := range args {
			if strings.Contains(a, "delete-excluded") {
				t.Errorf("не должен добавляться --delete-excluded: %q", a)
			}
		}
	}
}

// ── matchSyncExclude (эмуляция rsync-правил для copyTree) ─────

func TestMatchSyncExclude(t *testing.T) {
	cases := []struct {
		rel   string
		isDir bool
		toks  []string
		want  bool
	}{
		// dirOnly (завершающий /) — только директории, на любой глубине
		{"src", true, []string{"src/"}, true},
		{"src", false, []string{"src/"}, false},
		{"a/b/src", true, []string{"src/"}, true},
		// без внутреннего / — basename на любой глубине
		{"Cargo.toml", false, []string{"Cargo.toml"}, true},
		{"sub/Cargo.toml", false, []string{"Cargo.toml"}, true},
		{"Cargo.lock.bak", false, []string{"Cargo.toml"}, false},
		// глоб
		{"docs/readme.md", false, []string{"*.md"}, true},
		{"README.md", false, []string{"*.md"}, true},
		{"docs/readme.txt", false, []string{"*.md"}, false},
		// c внутренним / — от корня репозитория
		{"sub/dir", true, []string{"sub/dir"}, true},
		{"other/dir", true, []string{"sub/dir"}, false},
		{"sub/dir", true, []string{"sub/dir/"}, true},
		{"sub/dir", false, []string{"sub/dir/"}, false},
		{"sub", true, []string{"sub/dir"}, false},
		// ведущий / = корень
		{"x.txt", false, []string{"/x.txt"}, true},
		{"a/x.txt", false, []string{"/x.txt"}, false},
		// пусто
		{"src", true, nil, false},
	}
	for _, c := range cases {
		if got := matchSyncExclude(c.rel, c.isDir, c.toks); got != c.want {
			t.Errorf("matchSyncExclude(%q, isDir=%v, %q) = %v, want %v",
				c.rel, c.isDir, c.toks, got, c.want)
		}
	}
}

// ── copyTree с SYNC_EXCLUDE ───────────────────────────────────

func writeTree(t *testing.T, root string, files map[string]string) {
	t.Helper()
	for rel, content := range files {
		p := filepath.Join(root, filepath.FromSlash(rel))
		if err := os.MkdirAll(filepath.Dir(p), 0755); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(p, []byte(content), 0644); err != nil {
			t.Fatal(err)
		}
	}
}

func assertExists(t *testing.T, root, rel string, want bool) {
	t.Helper()
	_, err := os.Stat(filepath.Join(root, filepath.FromSlash(rel)))
	if exist := err == nil; exist != want {
		t.Errorf("%s: exists=%v, want %v", rel, exist, want)
	}
}

func TestCopyTreeSyncExclude(t *testing.T) {
	src := t.TempDir()
	dst := t.TempDir()
	writeTree(t, src, map[string]string{
		"keep.txt":         "ok",
		"skip.md":          "no",
		"Cargo.toml":       "no",
		"Cargo.lock":       "no",
		"Dockerfile":       "no",
		"src/main.rs":      "no",
		"deep/nested/x.md": "no",
		"deep/keep.txt":    "ok",
		".forgejo/ci.yml":  "no",
		".env":             "base-excluded",
		".git/config":      "base-excluded",
	})

	extra := syncExcludeTokens(map[string]string{
		"SYNC_EXCLUDE": "src/ Cargo.toml Cargo.lock Dockerfile *.md .forgejo/",
	})
	if err := copyTree(src, dst, extra); err != nil {
		t.Fatal(err)
	}

	assertExists(t, dst, "keep.txt", true)
	assertExists(t, dst, "deep/keep.txt", true)
	assertExists(t, dst, "skip.md", false)
	assertExists(t, dst, "Cargo.toml", false)
	assertExists(t, dst, "Cargo.lock", false)
	assertExists(t, dst, "Dockerfile", false)
	assertExists(t, dst, "src", false)
	assertExists(t, dst, "deep/nested/x.md", false)
	assertExists(t, dst, ".forgejo", false)
	// базовые исключения по-прежнему работают
	assertExists(t, dst, ".env", false)
	assertExists(t, dst, ".git", false)
}

func TestCopyTreeNoExtraUnchanged(t *testing.T) {
	src := t.TempDir()
	dst := t.TempDir()
	writeTree(t, src, map[string]string{
		"keep.txt": "ok",
		"note.md":  "copied: без SYNC_EXCLUDE ничего не фильтруется сверх базы",
		".env":     "base-excluded",
	})
	if err := copyTree(src, dst, nil); err != nil {
		t.Fatal(err)
	}
	assertExists(t, dst, "keep.txt", true)
	assertExists(t, dst, "note.md", true)
	assertExists(t, dst, ".env", false)
}

// ── syncDir end-to-end: .deploy → rsync или copyTree ──────────

func TestSyncDirReadsDeployExclude(t *testing.T) {
	src := t.TempDir()
	dst := filepath.Join(t.TempDir(), "www")
	writeTree(t, src, map[string]string{
		".deploy":      "SYNC_EXCLUDE=src/ Cargo.toml *.md .forgejo/\n",
		"index.html":   "<h1>ok</h1>",
		"readme.md":    "no",
		"Cargo.toml":   "no",
		"src/lib.rs":   "no",
		"docs/a.md":    "no",
		"docs/api.txt": "ok",
		".env":         "base-excluded",
	})

	if err := syncDir(src, dst); err != nil {
		t.Fatal(err)
	}

	assertExists(t, dst, "index.html", true)
	assertExists(t, dst, "docs/api.txt", true)
	assertExists(t, dst, "readme.md", false)
	assertExists(t, dst, "Cargo.toml", false)
	assertExists(t, dst, "src", false)
	assertExists(t, dst, "docs/a.md", false)
	assertExists(t, dst, ".env", false)
}

func TestSyncDirWithoutDeployExclude(t *testing.T) {
	src := t.TempDir()
	dst := filepath.Join(t.TempDir(), "www")
	writeTree(t, src, map[string]string{
		".deploy":    "NAME=plain\n",
		"index.html": "<h1>ok</h1>",
		"readme.md":  "copied",
		"Cargo.toml": "copied",
		".env":       "base-excluded",
	})

	if err := syncDir(src, dst); err != nil {
		t.Fatal(err)
	}

	assertExists(t, dst, "index.html", true)
	assertExists(t, dst, "readme.md", true)
	assertExists(t, dst, "Cargo.toml", true)
	assertExists(t, dst, ".env", false)
}
