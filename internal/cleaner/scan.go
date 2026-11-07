package cleaner

import (
	"context"
	"fmt"
	"io/fs"
	"os"
	"os/exec"
	"path/filepath"
	"sort"
	"strings"
	"syscall"
	"time"
)

type ScanResult struct {
	Candidates []Candidate
	Volumes    []Volume
	Skipped    int
}

type LastUsedFunc func(context.Context, string) (time.Time, bool)

func Scan(ctx context.Context, home string, s Settings, lastUsed LastUsedFunc) (ScanResult, error) {
	if err := s.Validate(); err != nil {
		return ScanResult{}, err
	}
	if lastUsed == nil {
		lastUsed = SpotlightLastUsed
	}
	roots := s.Roots
	if s.ScanHome {
		roots = []string{home}
	}
	result := ScanResult{}
	seen := make(map[string]bool)
	volumes := make(map[uint64]Volume)
	cutoff := time.Now().AddDate(0, 0, -s.MinAgeDays)
	for _, root := range roots {
		if err := ctx.Err(); err != nil {
			return result, err
		}
		root = filepath.Clean(root)
		if (root != filepath.Clean(home) || !s.ScanHome) && Protected(root, home, s.Exclusions) {
			result.Skipped++
			continue
		}
		info, err := os.Lstat(root)
		if err != nil || !info.IsDir() || info.Mode()&os.ModeSymlink != 0 {
			result.Skipped++
			continue
		}
		rootDev, _, ok := identity(info)
		if !ok {
			result.Skipped++
			continue
		}
		if _, exists := volumes[rootDev]; !exists {
			if v, err := VolumeStats(root, s); err == nil {
				volumes[rootDev] = v
			}
		}
		walkErr := filepath.WalkDir(root, func(path string, entry fs.DirEntry, err error) error {
			if ctx.Err() != nil {
				return ctx.Err()
			}
			if err != nil {
				result.Skipped++
				return nil
			}
			if path != root && Protected(path, home, s.Exclusions) {
				if entry.IsDir() {
					return fs.SkipDir
				}
				return nil
			}
			if entry.Type()&os.ModeSymlink != 0 {
				return nil
			}
			if entry.IsDir() {
				if _, err := os.Lstat(filepath.Join(path, ".git")); err == nil {
					return fs.SkipDir
				}
			}
			fi, err := entry.Info()
			if err != nil {
				result.Skipped++
				return nil
			}
			dev, ino, ok := identity(fi)
			if !ok {
				result.Skipped++
				return nil
			}
			if dev != rootDev {
				if entry.IsDir() {
					return fs.SkipDir
				}
				return nil
			}
			if !fi.Mode().IsRegular() || fi.Size() < s.MinSizeBytes || fi.ModTime().After(cutoff) {
				return nil
			}
			id := fmt.Sprintf("%d:%d", dev, ino)
			if seen[id] {
				return nil
			}
			seen[id] = true
			result.Candidates = append(result.Candidates, Candidate{
				ID: id, Path: path, Size: fi.Size(), ModifiedAt: fi.ModTime(),
				Device: dev, Inode: ino, Safety: SafetyReview,
				Reason: "The file is large and old; last use is not yet known.",
			})
			return nil
		})
		if walkErr != nil {
			return result, walkErr
		}
	}
	for i := range result.Candidates {
		if err := ctx.Err(); err != nil {
			return result, err
		}
		c := &result.Candidates[i]
		if used, ok := lastUsed(ctx, c.Path); ok {
			c.LastUsedAt, c.LastUseKnown = used, true
			if used.After(cutoff) {
				c.Reason = "The file was opened recently; review before cleaning."
			} else {
				c.Reason = "Old large file; awaiting AI review."
			}
		}
	}
	sort.Slice(result.Candidates, func(i, j int) bool { return result.Candidates[i].Size > result.Candidates[j].Size })
	for _, v := range volumes {
		result.Volumes = append(result.Volumes, v)
	}
	sort.Slice(result.Volumes, func(i, j int) bool { return result.Volumes[i].Path < result.Volumes[j].Path })
	return result, nil
}

func identity(fi os.FileInfo) (uint64, uint64, bool) {
	st, ok := fi.Sys().(*syscall.Stat_t)
	if !ok {
		return 0, 0, false
	}
	return uint64(st.Dev), uint64(st.Ino), true
}

func VolumeStats(path string, s Settings) (Volume, error) {
	fi, err := os.Lstat(path)
	if err != nil {
		return Volume{}, err
	}
	dev, _, ok := identity(fi)
	if !ok {
		return Volume{}, fmt.Errorf("cannot identify volume: %s", path)
	}
	var st syscall.Statfs_t
	if err := syscall.Statfs(path, &st); err != nil {
		return Volume{}, err
	}
	free := int64(st.Bavail) * int64(st.Bsize)
	return Volume{Path: path, Device: dev, FreeBytes: free, Level: Level(free, s.RedGB, s.YellowGB)}, nil
}

func SpotlightLastUsed(ctx context.Context, path string) (time.Time, bool) {
	request, cancel := context.WithTimeout(ctx, 3*time.Second)
	defer cancel()
	cmd := exec.CommandContext(request, "/usr/bin/mdls", "-raw", "-name", "kMDItemLastUsedDate", path)
	out, err := cmd.Output()
	if err != nil {
		return time.Time{}, false
	}
	value := strings.TrimSpace(string(out))
	if value == "" || value == "(null)" {
		return time.Time{}, false
	}
	for _, layout := range []string{"2006-01-02 15:04:05 -0700", "2006-01-02 15:04:05 +0000", time.RFC3339} {
		if t, err := time.Parse(layout, value); err == nil {
			return t, true
		}
	}
	return time.Time{}, false
}
