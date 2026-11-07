package cleaner

import (
	"context"
	"encoding/json"
	"errors"
	"os"
	"path/filepath"
	"testing"
	"time"
)

func TestLevelAndRules(t *testing.T) {
	if Level(10*GB, 10, 30) != "red" || Level(30*GB, 10, 30) != "yellow" || Level(31*GB, 10, 30) != "green" {
		t.Fatal("disk thresholds")
	}
	home := "/Users/test"
	for _, path := range []string{"/System/file", home + "/Library/cache", home + "/Pictures/Photos Library.photoslibrary/a", home + "/Downloads/.git/config", home + "/.codex/sessions/log", home + "/Downloads/keep/a"} {
		if !Protected(path, home, []string{home + "/Downloads/keep"}) {
			t.Fatalf("not protected: %s", path)
		}
	}
	if Protected(home+"/Downloads/old.dmg", home, nil) {
		t.Fatal("normal download protected")
	}
}

func TestScanAndStaleFile(t *testing.T) {
	home := t.TempDir()
	root := filepath.Join(home, "Downloads")
	if err := os.Mkdir(root, 0700); err != nil {
		t.Fatal(err)
	}
	old := filepath.Join(root, "old.dmg")
	newer := filepath.Join(root, "new.dmg")
	for _, p := range []string{old, newer} {
		if err := os.WriteFile(p, []byte("123456"), 0600); err != nil {
			t.Fatal(err)
		}
	}
	past := time.Now().AddDate(0, 0, -120)
	if err := os.Chtimes(old, past, past); err != nil {
		t.Fatal(err)
	}
	if err := os.Symlink(old, filepath.Join(root, "link.dmg")); err != nil {
		t.Fatal(err)
	}
	s := DefaultSettings(home)
	s.MinSizeBytes = 4
	result, err := Scan(context.Background(), home, s, func(context.Context, string) (time.Time, bool) { return past, true })
	if err != nil {
		t.Fatal(err)
	}
	if len(result.Candidates) != 1 || result.Candidates[0].Path != old {
		t.Fatalf("scan result: %+v", result.Candidates)
	}
	if err := ValidateForAction(result.Candidates[0], home, s); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(old, []byte("changed"), 0600); err != nil {
		t.Fatal(err)
	}
	if !errors.Is(ValidateForAction(result.Candidates[0], home, s), ErrStale) {
		t.Fatal("modified file was accepted")
	}
	// Home mode still scans its children, while keeping protected areas out.
	s.ScanHome = true
	if _, err := Scan(context.Background(), home, s, func(context.Context, string) (time.Time, bool) { return time.Time{}, false }); err != nil {
		t.Fatal(err)
	}
}

func TestClaudeCannotPromoteUnknownUseOrProtected(t *testing.T) {
	home := "/Users/test"
	s := DefaultSettings(home)
	past := time.Now().AddDate(0, 0, -120)
	items := []Candidate{
		{ID: "old", Path: home + "/Downloads/old.dmg", ModifiedAt: past, LastUsedAt: past, LastUseKnown: true, Safety: SafetyReview},
		{ID: "unknown", Path: home + "/Downloads/unknown.dmg", ModifiedAt: past, Safety: SafetyReview},
		{ID: "protected", Path: home + "/Library/private.db", ModifiedAt: past, LastUsedAt: past, LastUseKnown: true, Safety: SafetyProtected},
	}
	result := reviewOutput{}
	for _, id := range []string{"old", "unknown", "protected"} {
		result.Items = append(result.Items, struct {
			ID      string `json:"id"`
			Verdict string `json:"verdict"`
			Reason  string `json:"reason"`
		}{id, "safe", "disposable"})
	}
	structured, _ := json.Marshal(result)
	envelope, _ := json.Marshal(map[string]any{"structured_output": json.RawMessage(structured)})
	cli := ReviewCLI{Run: func(context.Context, string, []string, []byte) ([]byte, error) { return envelope, nil }}
	out, err := cli.Review(context.Background(), items, home, s)
	if err != nil {
		t.Fatal(err)
	}
	if out[0].Safety != SafetySafe || out[1].Safety != SafetyReview || out[2].Safety != SafetyProtected {
		t.Fatalf("unsafe AI promotion: %+v", out)
	}
	cli.Run = func(context.Context, string, []string, []byte) ([]byte, error) { return nil, errors.New("offline") }
	out, err = cli.Review(context.Background(), items, home, s)
	if err == nil || out[0].Safety != SafetyReview {
		t.Fatal("CLI failure must preserve manual review")
	}
}
