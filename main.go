package main

import (
	"bytes"
	"context"
	"fmt"
	"image"
	"image/color"
	"image/png"
	"log"
	"os"
	"os/exec"
	"path/filepath"
	"sort"
	"strconv"
	"strings"
	"time"

	"github.com/egoist/mygo"
	"github.com/egoist/mygo/ui"
	"github.com/quan-vu/safety-disk-cleaner/internal/cleaner"
)

type app struct {
	store         *cleaner.Store
	home          string
	settings      cleaner.Settings
	candidates    []cleaner.Candidate
	volumes       []cleaner.Volume
	selected      map[string]*bool
	win           *mygo.Window
	tray          *mygo.Tray
	quitting      bool
	vi            bool
	tab           string
	filter        string
	showIgnored   bool
	sortSmall     bool
	status        string
	scanning      bool
	deleteConfirm bool
	deletePhrase  string
	minSizeMB     string
	minAgeDays    string
	redGB         string
	yellowGB      string
	exclusions    string
}

func main() {
	if !mygo.App.RequestSingleInstanceLock() {
		return
	}
	home, err := os.UserHomeDir()
	if err != nil {
		log.Fatal(err)
	}
	dataDir, err := mygo.App.Path(mygo.PathUserData)
	if err != nil {
		log.Fatal(err)
	}
	store, err := cleaner.OpenStore(context.Background(), dataDir)
	if err != nil {
		log.Fatal(err)
	}
	settings, err := store.LoadSettings(context.Background(), home)
	if err != nil {
		log.Fatal(err)
	}
	candidates, err := store.LoadCandidates(context.Background())
	if err != nil {
		log.Fatal(err)
	}
	a := &app{store: store, home: home, settings: settings, candidates: candidates,
		selected: map[string]*bool{}, vi: strings.HasPrefix(strings.ToLower(mygo.App.Locale()), "vi"), tab: "overview"}
	a.loadEditors()
	mygo.App.SetActivationPolicy(mygo.ActivationPolicyAccessory)
	mygo.App.OnBeforeQuit(func(*mygo.QuitEvent) { a.quitting = true })
	mygo.App.OnQuit(func() { _ = store.Close() })
	mygo.App.OnSecondInstance(func([]string, string) { a.show() })
	mygo.App.OnNotificationClick(func(string) { a.show() })
	mygo.App.WhenReady(a.ready)
	if err := mygo.App.Run(); err != nil {
		log.Fatal(err)
	}
}

func (a *app) t(vi, en string) string {
	if a.vi {
		return vi
	}
	return en
}

func (a *app) loadEditors() {
	a.minSizeMB = strconv.FormatInt(a.settings.MinSizeBytes/1_000_000, 10)
	a.minAgeDays = strconv.Itoa(a.settings.MinAgeDays)
	a.redGB = strconv.FormatInt(a.settings.RedGB, 10)
	a.yellowGB = strconv.FormatInt(a.settings.YellowGB, 10)
	a.exclusions = strings.Join(a.settings.Exclusions, "\n")
}

func (a *app) ready() {
	a.win = mygo.NewWindow(mygo.WindowOptions{
		Title: "Safety Disk Cleaner", Width: 1040, Height: 720,
		Hidden:  mygo.App.WasOpenedAtLogin() && a.settings.SetupDone,
		Content: ui.View(a.view),
	})
	a.win.OnClose(func(e *mygo.CloseEvent) {
		if !a.quitting {
			e.PreventDefault()
			a.win.Hide()
		}
	})
	tray, err := mygo.NewTray(mygo.TrayOptions{ID: "safety-disk-cleaner", Icon: trayIcon("green"), ToolTip: "Safety Disk Cleaner"})
	if err == nil {
		a.tray = tray
		tray.OnClick(a.show)
		menu := mygo.NewMenu([]*mygo.MenuItem{
			{Label: a.t("Mở Safety Disk Cleaner", "Open Safety Disk Cleaner"), Click: func(*mygo.MenuItem, *mygo.Window) { a.show() }},
			{Label: a.t("Quét ngay", "Scan now"), Click: func(*mygo.MenuItem, *mygo.Window) { a.startScan() }},
			mygo.Separator(), {Role: mygo.RoleQuit},
		})
		tray.OnRightClick(func() {
			tray.SetMenu(menu)
			defer tray.SetMenu(nil)
			tray.PopUpMenu()
		})
	}
	mygo.App.OnDidBecomeActive(func() { mygo.ClearNotifications() })
	if a.settings.SetupDone {
		a.refreshVolumes()
		a.maybeScan()
	}
	go a.scheduleLoop()
}

func (a *app) show() {
	if a.win == nil {
		return
	}
	a.win.Show()
	a.win.Restore()
	a.win.Focus()
}

func (a *app) scheduleLoop() {
	ticker := time.NewTicker(time.Minute)
	defer ticker.Stop()
	for range ticker.C {
		if a.win == nil || a.win.IsDestroyed() {
			return
		}
		a.win.Update(a.maybeScan)
	}
}

func (a *app) maybeScan() {
	if a.settings.SetupDone && a.settings.AutoScan && !a.scanning &&
		(time.Since(a.settings.LastScan) >= 2*time.Hour || a.settings.LastScan.IsZero()) {
		a.startScan()
	}
}

func (a *app) refreshVolumes() {
	roots := a.settings.Roots
	if a.settings.ScanHome {
		roots = []string{a.home}
	}
	seen := map[uint64]bool{}
	var volumes []cleaner.Volume
	for _, p := range roots {
		v, err := cleaner.VolumeStats(p, a.settings)
		if err != nil || seen[v.Device] {
			continue
		}
		seen[v.Device] = true
		volumes = append(volumes, v)
	}
	a.volumes = volumes
	a.updateTray()
}

func (a *app) updateTray() {
	if a.tray == nil {
		return
	}
	level := "green"
	for _, v := range a.volumes {
		if v.Level == "red" {
			level = "red"
			break
		}
		if v.Level == "yellow" {
			level = "yellow"
		}
	}
	a.tray.SetIcon(trayIcon(level), false)
}

func trayIcon(level string) []byte {
	ink := color.NRGBA{R: 56, G: 178, B: 111, A: 255}
	if level == "yellow" {
		ink = color.NRGBA{R: 236, G: 179, B: 51, A: 255}
	}
	if level == "red" {
		ink = color.NRGBA{R: 224, G: 83, B: 77, A: 255}
	}
	img := image.NewNRGBA(image.Rect(0, 0, 32, 32))
	for y := 2; y < 30; y++ {
		for x := 2; x < 30; x++ {
			dx, dy := x-16, y-16
			if dx*dx+dy*dy <= 13*13 {
				img.Set(x, y, ink)
			}
		}
	}
	var buf bytes.Buffer
	_ = png.Encode(&buf, img)
	return buf.Bytes()
}

func (a *app) startScan() {
	if a.scanning || !a.settings.SetupDone {
		return
	}
	a.scanning = true
	a.status = a.t("Đang quét...", "Scanning...")
	settings := a.settings
	before := append([]cleaner.Candidate(nil), a.candidates...)
	beforeLevel := a.settings.LastAlertLevel
	go func() {
		ctx := context.Background()
		result, err := cleaner.Scan(ctx, a.home, settings, nil)
		var aiErr error
		if err == nil {
			result.Candidates, aiErr = (cleaner.ReviewCLI{}).Review(ctx, result.Candidates, a.home, settings)
		}
		if err == nil {
			err = a.store.ReplaceCandidates(ctx, result.Candidates)
		}
		var saved []cleaner.Candidate
		if err == nil {
			saved, err = a.store.LoadCandidates(ctx)
		}
		if err == nil {
			settings.LastScan = time.Now()
			settings.LastAlertIDs = candidateIDs(saved)
			settings.LastAlertLevel = worstLevel(result.Volumes)
			err = a.store.SaveSettings(ctx, settings)
		}
		a.win.Update(func() {
			a.scanning = false
			if err != nil {
				a.status = err.Error()
				return
			}
			a.candidates, a.volumes, a.settings = saved, result.Volumes, settings
			a.selected = map[string]*bool{}
			a.updateTray()
			a.status = a.t(fmt.Sprintf("Đã quét: %d gợi ý, bỏ qua %d mục.", len(saved), result.Skipped),
				fmt.Sprintf("Scan complete: %d suggestions, %d skipped.", len(saved), result.Skipped))
			if aiErr != nil {
				a.status += " Claude: " + aiErr.Error()
			}
			if shouldNotify(before, saved, beforeLevel, worstLevel(result.Volumes)) {
				a.notify(len(saved))
			}
		})
	}()
}

func candidateIDs(items []cleaner.Candidate) []string {
	ids := make([]string, 0, len(items))
	for _, c := range items {
		if !c.Ignored {
			ids = append(ids, c.ID)
		}
	}
	sort.Strings(ids)
	return ids
}

func worstLevel(volumes []cleaner.Volume) string {
	level := "green"
	for _, v := range volumes {
		if v.Level == "red" {
			return "red"
		}
		if v.Level == "yellow" {
			level = "yellow"
		}
	}
	return level
}

func shouldNotify(before, after []cleaner.Candidate, beforeLevel, afterLevel string) bool {
	if beforeLevel != afterLevel {
		return true
	}
	previous := map[string]bool{}
	for _, c := range before {
		previous[c.ID] = true
	}
	for _, c := range after {
		if !c.Ignored && !previous[c.ID] {
			return true
		}
	}
	return false
}

func (a *app) notify(count int) {
	if !mygo.NotificationsSupported() {
		return
	}
	n := mygo.NewNotification(mygo.NotificationOptions{ID: "cleanup-suggestions", Title: "Safety Disk Cleaner",
		Body: a.t(fmt.Sprintf("Có %d tệp cần xem trước khi dọn.", count), fmt.Sprintf("%d files to review for cleaning.", count))})
	go func() {
		if err := n.Show(); err != nil {
			log.Println("notification:", err)
		}
	}()
}

func (a *app) selectedItems() []cleaner.Candidate {
	var out []cleaner.Candidate
	for _, c := range a.candidates {
		if chosen := a.selected[c.ID]; chosen != nil && *chosen {
			out = append(out, c)
		}
	}
	return out
}

func (a *app) ignoreSelected() {
	items := a.selectedItems()
	ids := make([]string, 0, len(items))
	for _, c := range items {
		ids = append(ids, c.ID)
	}
	if len(ids) == 0 {
		return
	}
	if err := a.store.Ignore(context.Background(), ids); err != nil {
		a.status = err.Error()
		return
	}
	for i := range a.candidates {
		for _, id := range ids {
			if a.candidates[i].ID == id {
				a.candidates[i].Ignored = true
			}
		}
	}
	a.selected = map[string]*bool{}
	a.status = a.t("Đã bỏ qua các mục đã chọn.", "Selected files ignored.")
}

func (a *app) unignoreSelected() {
	items := a.selectedItems()
	var ids []string
	for _, c := range items { if c.Ignored { ids = append(ids, c.ID) } }
	if len(ids) == 0 { return }
	if err := a.store.Unignore(context.Background(), ids); err != nil { a.status = err.Error(); return }
	idsSet := map[string]bool{}
	for _, id := range ids { idsSet[id] = true }
	for i := range a.candidates { if idsSet[a.candidates[i].ID] { a.candidates[i].Ignored = false } }
	a.selected = map[string]*bool{}
	a.status = a.t("Đã đưa các tệp trở lại danh sách.", "Files returned to the review list.")
}

func (a *app) revealSelected() {
	for _, c := range a.selectedItems() {
		_ = exec.Command("/usr/bin/open", "-R", c.Path).Start()
	}
}

func (a *app) cleanSelected(permanent bool) {
	items := a.selectedItems()
	if len(items) == 0 {
		return
	}
	if permanent && a.deletePhrase != fmt.Sprintf("DELETE %d", len(items)) {
		return
	}
	a.deleteConfirm, a.deletePhrase = false, ""
	settings := a.settings
	go func() {
		if !permanent {
			answer, err := mygo.Dialog.Message(mygo.MessageOptions{Parent: a.win, Type: mygo.MessageWarning,
				Message: a.t("Chuyển tệp đã chọn vào Trash?", "Move selected files to Trash?"),
				Detail:  fmt.Sprintf("%d files", len(items)), Buttons: []string{a.t("Hủy", "Cancel"), a.t("Chuyển vào Trash", "Move to Trash")},
				DestructiveButtons: []int{1}, DefaultButton: 0, CancelButton: 0})
			if err != nil || answer.Button != 1 {
				return
			}
		}
		resourceDir, err := mygo.App.Path(mygo.PathResources)
		if err != nil {
			a.win.Update(func() { a.status = err.Error() })
			return
		}
		helper := filepath.Join(resourceDir, "trash-helper")
		var succeeded []string
		var failed []string
		for _, c := range items {
			var actionErr error
			if permanent {
				actionErr = cleaner.DeletePermanently(c, a.home, settings)
			} else {
				actionErr = cleaner.Trash(context.Background(), c, helper, a.home, settings)
			}
			action := "trash"
			if permanent {
				action = "delete"
			}
			result := "ok"
			if actionErr != nil {
				result = actionErr.Error()
				failed = append(failed, filepath.Base(c.Path)+": "+result)
			} else {
				succeeded = append(succeeded, c.ID)
			}
			_ = a.store.RecordAction(context.Background(), time.Now().Format(time.RFC3339), action, c.Path, result)
		}
		if len(succeeded) > 0 {
			if err := a.store.RemoveCandidates(context.Background(), succeeded); err != nil {
				failed = append(failed, "history: "+err.Error())
			}
		}
		a.win.Update(func() {
			remaining := a.candidates[:0]
			removed := map[string]bool{}
			for _, id := range succeeded {
				removed[id] = true
			}
			for _, c := range a.candidates {
				if !removed[c.ID] {
					remaining = append(remaining, c)
				}
			}
			a.candidates = remaining
			a.selected = map[string]*bool{}
			a.refreshVolumes()
			a.status = a.t(fmt.Sprintf("Đã dọn %d tệp; lỗi %d.", len(succeeded), len(failed)), fmt.Sprintf("Cleaned %d files; %d errors.", len(succeeded), len(failed)))
			if len(failed) > 0 {
				a.status += " " + strings.Join(failed, "; ")
			}
		})
	}()
}
