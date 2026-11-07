package cleaner

import (
	"context"
	"errors"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
)

var ErrStale = errors.New("file changed since scan; scan again")

func ValidateForAction(c Candidate, home string, s Settings) error {
	if Protected(c.Path, home, s.Exclusions) {
		return errors.New("protected path")
	}
	roots := s.Roots
	if s.ScanHome {
		roots = []string{home}
	}
	allowed := false
	for _, root := range roots {
		if !within(c.Path, root) {
			continue
		}
		allowed = true
		// Reject a parent replaced by a symlink after the scan.
		parent := filepath.Dir(c.Path)
		for within(parent, root) {
			fi, err := os.Lstat(parent)
			if err != nil || !fi.IsDir() || fi.Mode()&os.ModeSymlink != 0 {
				return ErrStale
			}
			if parent == root {
				break
			}
			parent = filepath.Dir(parent)
		}
		break
	}
	if !allowed {
		return errors.New("outside selected scan folders")
	}
	fi, err := os.Lstat(c.Path)
	if err != nil {
		return ErrStale
	}
	if !fi.Mode().IsRegular() || fi.Mode()&os.ModeSymlink != 0 {
		return ErrStale
	}
	dev, ino, ok := identity(fi)
	if !ok || dev != c.Device || ino != c.Inode || fi.Size() != c.Size || !fi.ModTime().Equal(c.ModifiedAt) {
		return ErrStale
	}
	return nil
}

func Trash(ctx context.Context, c Candidate, helper, home string, s Settings) error {
	if err := ValidateForAction(c, home, s); err != nil {
		return err
	}
	if helper == "" {
		return errors.New("macOS Trash helper is missing")
	}
	cmd := exec.CommandContext(ctx, helper, c.Path)
	if out, err := cmd.CombinedOutput(); err != nil {
		return fmt.Errorf("move to Trash: %w: %s", err, out)
	}
	return nil
}

func DeletePermanently(c Candidate, home string, s Settings) error {
	if err := ValidateForAction(c, home, s); err != nil {
		return err
	}
	return os.Remove(c.Path)
}
