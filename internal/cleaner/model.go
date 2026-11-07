package cleaner

import (
	"errors"
	"os"
	"path/filepath"
	"strings"
	"time"
)

const GB int64 = 1_000_000_000

type Settings struct {
	SetupDone      bool      `json:"setupDone"`
	AutoScan       bool      `json:"autoScan"`
	OpenAtLogin    bool      `json:"openAtLogin"`
	ScanHome       bool      `json:"scanHome"`
	Roots          []string  `json:"roots"`
	Exclusions     []string  `json:"exclusions"`
	MinSizeBytes   int64     `json:"minSizeBytes"`
	MinAgeDays     int       `json:"minAgeDays"`
	RedGB          int64     `json:"redGB"`
	YellowGB       int64     `json:"yellowGB"`
	LastScan       time.Time `json:"lastScan"`
	LastAlertLevel string    `json:"lastAlertLevel"`
	LastAlertIDs   []string  `json:"lastAlertIds"`
}

func DefaultSettings(home string) Settings {
	return Settings{
		AutoScan: true, OpenAtLogin: true,
		Roots:        []string{filepath.Join(home, "Downloads")},
		MinSizeBytes: 500 * 1_000_000, MinAgeDays: 90,
		RedGB: 10, YellowGB: 30,
	}
}

func (s Settings) Validate() error {
	if s.MinSizeBytes < 1 || s.MinAgeDays < 1 || s.RedGB < 1 || s.YellowGB <= s.RedGB {
		return errors.New("invalid size, age, or disk thresholds")
	}
	if !s.ScanHome && len(s.Roots) == 0 {
		return errors.New("select at least one scan folder")
	}
	for _, p := range append(append([]string{}, s.Roots...), s.Exclusions...) {
		if !filepath.IsAbs(p) || filepath.Clean(p) != p {
			return errors.New("scan paths must be absolute and clean")
		}
	}
	return nil
}

type Safety string

const (
	SafetyProtected Safety = "protected"
	SafetyReview    Safety = "review"
	SafetySafe      Safety = "safe"
)

type Candidate struct {
	ID           string    `json:"id"`
	Path         string    `json:"path"`
	Size         int64     `json:"size"`
	ModifiedAt   time.Time `json:"modifiedAt"`
	LastUsedAt   time.Time `json:"lastUsedAt"`
	LastUseKnown bool      `json:"lastUseKnown"`
	Device       uint64    `json:"device"`
	Inode        uint64    `json:"inode"`
	Safety       Safety    `json:"safety"`
	Reason       string    `json:"reason"`
	Ignored      bool      `json:"ignored"`
}

type Volume struct {
	Path      string `json:"path"`
	Device    uint64 `json:"device"`
	FreeBytes int64  `json:"freeBytes"`
	Level     string `json:"level"`
}

func Level(free int64, redGB, yellowGB int64) string {
	if free <= redGB*GB {
		return "red"
	}
	if free <= yellowGB*GB {
		return "yellow"
	}
	return "green"
}

func Protected(path, home string, exclusions []string) bool {
	p := filepath.Clean(path)
	h := filepath.Clean(home)
	if !filepath.IsAbs(p) || p == "/" || p == h {
		return true
	}
	if !within(p, h) {
		for _, x := range []string{"/System", "/Library", "/Applications", "/private", "/usr", "/bin", "/sbin", "/dev", "/etc", "/opt", "/var"} {
			if within(p, x) {
				return true
			}
		}
	}
	for _, x := range exclusions {
		if within(p, filepath.Clean(x)) {
			return true
		}
	}
	if within(p, h) {
		rel, _ := filepath.Rel(h, p)
		parts := strings.Split(rel, string(os.PathSeparator))
		for _, part := range parts {
			switch strings.ToLower(part) {
			case "library", ".trash", ".git", ".ssh", ".aws", ".config", ".claude", ".local", ".npm", ".cache", "node_modules", "photos library.photoslibrary", "music library.musiclibrary", "mobile documents", "icloud drive", "onedrive", "dropbox", "google drive":
				return true
			}
		}
	}
	for _, part := range strings.Split(p, string(os.PathSeparator)) {
		lower := strings.ToLower(part)
		if strings.HasPrefix(part, ".") && part != "." && part != ".." {
			return true
		}
		if strings.HasPrefix(lower, "onedrive") || strings.HasPrefix(lower, "dropbox") || strings.HasPrefix(lower, "google drive") {
			return true
		}
		if strings.HasSuffix(lower, ".app") || strings.HasSuffix(lower, ".photoslibrary") || strings.HasSuffix(lower, ".musiclibrary") || strings.HasSuffix(lower, ".bundle") || strings.HasSuffix(lower, ".framework") {
			return true
		}
	}
	return false
}

func within(path, dir string) bool {
	rel, err := filepath.Rel(dir, path)
	return err == nil && rel != ".." && !strings.HasPrefix(rel, ".."+string(os.PathSeparator))
}
