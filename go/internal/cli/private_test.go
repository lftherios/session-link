//go:build !windows

package cli

import (
	"os"
	"path/filepath"
	"testing"
)

func TestSecureHomeRepairsExistingFilesWithoutFollowingSymlinks(t *testing.T) {
	home := filepath.Join(t.TempDir(), "home")
	t.Setenv("SLINK_HOME", home)
	dir := filepath.Join(home, "runs")
	if err := os.MkdirAll(dir, 0755); err != nil {
		t.Fatal(err)
	}
	file := filepath.Join(dir, "old.json")
	if err := os.WriteFile(file, []byte("private transcript"), 0644); err != nil {
		t.Fatal(err)
	}
	external := filepath.Join(t.TempDir(), "source")
	if err := os.WriteFile(external, []byte("external source"), 0644); err != nil {
		t.Fatal(err)
	}
	if err := os.Symlink(external, filepath.Join(dir, "source-link")); err != nil {
		t.Fatal(err)
	}
	before, _ := os.Stat(external)
	if err := SecureHome(); err != nil {
		t.Fatal(err)
	}
	for path, want := range map[string]os.FileMode{home: 0700, dir: 0700, file: 0600} {
		info, err := os.Stat(path)
		if err != nil || info.Mode().Perm() != want {
			t.Fatalf("%s: mode %v, error %v", path, info, err)
		}
	}
	after, _ := os.Stat(external)
	if before.Mode() != after.Mode() {
		t.Fatal("changed a source outside the data directory")
	}
}
