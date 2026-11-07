package main

import (
	"context"
	"fmt"
	"path/filepath"
	"sort"
	"strconv"
	"strings"
	"time"

	"github.com/egoist/mygo"
	"github.com/egoist/mygo/ui"
	"github.com/quan-vu/safety-disk-cleaner/internal/cleaner"
)

func (a *app) view(c *ui.Context) {
	t := c.Theme()
	ui.Column(c).Fill().Padding(22).Gap(16).Children(func() {
		ui.Row(c).Gap(12).Children(func() {
			ui.Column(c).Grow(1).Gap(4).Children(func() {
				ui.Text(c, "Safety Disk Cleaner").FontSize(25).Bold()
				ui.Text(c, a.t("Gợi ý dọn đĩa an toàn, bạn luôn quyết định.", "Safe disk cleanup suggestions. You stay in control.")).FontSize(12).TextColor(t.TextMuted)
			})
			if a.settings.SetupDone && ui.Button(c, a.t("Quét ngay", "Scan now")).Disabled(a.scanning).Clicked() {
				a.startScan()
			}
		})
		if !a.settings.SetupDone {
			a.setupView(c)
			return
		}
		ui.Row(c).Gap(8).Children(func() {
			if ui.Button(c, a.t("Tổng quan", "Overview")).Clicked() {
				a.tab = "overview"
			}
			if ui.Button(c, a.t("Danh sách", "Files")).Clicked() {
				a.tab = "files"
			}
			if ui.Button(c, a.t("Cài đặt", "Settings")).Clicked() {
				a.tab = "settings"
			}
			ui.Spacer(c)
			ui.Text(c, a.t("Lần quét: ", "Last scan: ")+formatLastScan(a.settings.LastScan)).FontSize(11).TextColor(t.TextMuted)
		})
		ui.Divider(c)
		switch a.tab {
		case "files":
			a.filesView(c)
		case "settings":
			a.settingsView(c)
		default:
			a.overviewView(c)
		}
		if a.status != "" {
			ui.Text(c, a.status).FontSize(11).TextColor(t.TextMuted)
		}
	})
}

func (a *app) setupView(c *ui.Context) {
	t := c.Theme()
	ui.Column(c).Grow(1).Gap(14).Children(func() {
		ui.Text(c, a.t("Thiết lập lần đầu", "First setup")).FontSize(20).Bold()
		ui.Text(c, a.t("App chỉ gửi đường dẫn, kích thước và ngày của tệp cho Claude CLI; không đọc nội dung. Lịch tự động chỉ quét và thông báo.",
			"The app sends only file paths, sizes, and dates to Claude CLI, never file contents. Automatic mode only scans and notifies.")).TextColor(t.TextMuted)
		ui.Checkbox(c, &a.settings.ScanHome, a.t("Quét toàn bộ Home (trừ vùng bảo vệ)", "Scan all Home (except protected areas)"))
		ui.Text(c, a.t("Hoặc quét thư mục mặc định:", "Or scan the default folder:")+" "+strings.Join(a.settings.Roots, ", "))
		if ui.PrimaryButton(c, a.t("Hoàn tất và quét", "Finish and scan")).Clicked() {
			a.settings.SetupDone = true
			if err := a.store.SaveSettings(context.Background(), a.settings); err != nil {
				a.status = err.Error()
				a.settings.SetupDone = false
				return
			}
			if a.settings.OpenAtLogin {
				if err := mygo.App.SetOpenAtLogin(true); err != nil {
					a.status = err.Error()
				}
			}
			a.refreshVolumes()
			a.startScan()
		}
	})
}

func (a *app) overviewView(c *ui.Context) {
	t := c.Theme()
	ui.Column(c).Grow(1).Gap(16).Children(func() {
		ui.Text(c, a.t("Dung lượng trống", "Free disk space")).FontSize(18).Bold()
		if len(a.volumes) == 0 {
			ui.Text(c, a.t("Chưa đọc được thông tin ổ đĩa.", "Disk information is unavailable.")).TextColor(t.TextMuted)
		}
		for _, v := range a.volumes {
			color := t.Success
			if v.Level == "yellow" {
				color = t.Warning
			}
			if v.Level == "red" {
				color = t.Danger
			}
			ui.Row(c).Padding(14).Gap(12).Radius(t.Radius).Background(t.Surface).Border(1, t.Border).Children(func() {
				ui.Text(c, v.Path).Grow(1).SingleLine()
				ui.Text(c, formatBytes(v.FreeBytes)).Bold().TextColor(color)
				ui.Text(c, strings.ToUpper(v.Level)).TextColor(color)
			})
		}
		ui.Divider(c)
		var total int64
		var count int
		for _, item := range a.candidates {
			if !item.Ignored {
				total += item.Size
				count++
			}
		}
		ui.Text(c, a.t(fmt.Sprintf("%d tệp cần xem · tối đa %s", count, formatBytes(total)),
			fmt.Sprintf("%d files to review · up to %s", count, formatBytes(total)))).FontSize(18).Bold()
		ui.Text(c, a.t("Dung lượng chỉ được giải phóng sau khi Trash được làm trống. Mỗi tệp được kiểm tra lại trước khi dọn.",
			"Space is reclaimed after Trash is emptied. Every file is checked again before cleanup.")).TextColor(t.TextMuted)
		if ui.PrimaryButton(c, a.t("Xem danh sách", "Review files")).Clicked() {
			a.tab = "files"
		}
	})
}

func (a *app) filesView(c *ui.Context) {
	t := c.Theme()
	items := make([]cleaner.Candidate, 0, len(a.candidates))
	query := strings.ToLower(strings.TrimSpace(a.filter))
	for _, item := range a.candidates {
		if item.Ignored && !a.showIgnored {
			continue
		}
		if query != "" && !strings.Contains(strings.ToLower(item.Path), query) {
			continue
		}
		items = append(items, item)
	}
	sort.Slice(items, func(i, j int) bool {
		if a.sortSmall {
			return items[i].Size < items[j].Size
		}
		return items[i].Size > items[j].Size
	})
	ui.Column(c).Grow(1).Gap(10).Children(func() {
		ui.Row(c).Gap(8).Children(func() {
			ui.TextInput(c, &a.filter).Placeholder(a.t("Lọc theo đường dẫn", "Filter by path")).Grow(1)
			if ui.Button(c, a.t("Đổi thứ tự cỡ", "Reverse size sort")).Clicked() {
				a.sortSmall = !a.sortSmall
			}
			ui.Checkbox(c, &a.showIgnored, a.t("Hiện đã bỏ qua", "Show ignored"))
		})
		selected := len(a.selectedItems())
		ui.Row(c).Gap(8).Children(func() {
			ui.Text(c, a.t(fmt.Sprintf("Đã chọn %d", selected), fmt.Sprintf("%d selected", selected))).Grow(1)
			if ui.Button(c, a.t("Mở Finder", "Reveal in Finder")).Disabled(selected == 0).Clicked() {
				a.revealSelected()
			}
			if ui.Button(c, a.t("Bỏ qua", "Ignore")).Disabled(selected == 0).Clicked() {
				a.ignoreSelected()
			}
			if a.showIgnored && ui.Button(c, a.t("Bỏ đánh dấu bỏ qua", "Restore ignored")).Disabled(selected == 0).Clicked() {
				a.unignoreSelected()
			}
			if ui.Button(c, a.t("Chuyển vào Trash", "Move to Trash")).Disabled(selected == 0).Clicked() {
				a.cleanSelected(false)
			}
			if ui.Button(c, a.t("Xóa vĩnh viễn...", "Delete permanently...")).Disabled(selected == 0).Clicked() {
				a.deleteConfirm = true
			}
		})
		if a.deleteConfirm {
			a.deletePreview(c)
		}
		if len(items) == 0 {
			ui.Text(c, a.t("Không có tệp phù hợp.", "No matching files.")).TextColor(t.TextMuted)
		}
		ui.Scroll(c).Grow(1).Children(func() {
			ui.Column(c).Gap(7).Children(func() {
				for _, item := range items {
					a.fileRow(c, item)
				}
			})
		})
	})
}

func (a *app) fileRow(c *ui.Context, item cleaner.Candidate) {
	t := c.Theme()
	chosen := a.selected[item.ID]
	if chosen == nil {
		chosen = new(bool)
		a.selected[item.ID] = chosen
	}
	color := t.TextMuted
	label := a.t("Cần xem", "Review")
	if item.Safety == cleaner.SafetySafe {
		color, label = t.Success, a.t("Gợi ý an toàn", "Safe suggestion")
	}
	if item.Ignored {
		label = a.t("Đã bỏ qua", "Ignored")
	}
	ui.Row(c).Padding(10).Gap(12).AlignItems(ui.Center).Radius(t.Radius).Background(t.Surface).Border(1, t.Border).Children(func() {
		ui.Checkbox(c, chosen, filepath.Base(item.Path))
		ui.Column(c).Grow(1).Gap(3).Children(func() {
			ui.Text(c, item.Path).FontSize(11).TextColor(t.TextMuted).SingleLine()
			ui.Text(c, a.reason(item)).FontSize(11).SingleLine()
		})
		ui.Text(c, formatBytes(item.Size)).FontSize(12)
		ui.Text(c, label).FontSize(11).TextColor(color)
	})
}

func (a *app) reason(item cleaner.Candidate) string {
	if !item.LastUseKnown {
		return a.t("Chưa rõ lần dùng gần nhất · ", "Last use unknown · ") + item.Reason
	}
	return a.t("Dùng lần cuối ", "Last used ") + item.LastUsedAt.Format("2006-01-02") + " · " + item.Reason
}

func (a *app) deletePreview(c *ui.Context) {
	t := c.Theme()
	items := a.selectedItems()
	phrase := fmt.Sprintf("DELETE %d", len(items))
	ui.Column(c).Padding(12).Gap(7).Radius(t.Radius).Background(t.Surface).Border(1, t.Danger).Children(func() {
		ui.Text(c, a.t("Xem trước xóa vĩnh viễn", "Preview permanent deletion")).Bold().TextColor(t.Danger)
		ui.Text(c, a.t("Không thể hoàn tác. Chỉ các tệp vẫn khớp lần quét mới được xóa.", "Cannot be undone. Only files still matching the scan will be deleted."))
		for i, item := range items {
			if i >= 5 {
				break
			}
			ui.Text(c, item.Path).FontSize(11).SingleLine()
		}
		if len(items) > 5 {
			ui.Text(c, fmt.Sprintf("+ %d", len(items)-5))
		}
		ui.Text(c, a.t("Nhập ", "Type ")+phrase)
		ui.TextInput(c, &a.deletePhrase).Placeholder(phrase)
		ui.Row(c).Gap(8).Children(func() {
			if ui.Button(c, a.t("Hủy", "Cancel")).Clicked() {
				a.deleteConfirm = false
				a.deletePhrase = ""
			}
			if ui.PrimaryButton(c, a.t("Xóa vĩnh viễn", "Delete permanently")).Disabled(a.deletePhrase != phrase).Clicked() {
				a.cleanSelected(true)
			}
		})
	})
}

func (a *app) settingsView(c *ui.Context) {
	t := c.Theme()
	ui.Scroll(c).Grow(1).Children(func() {
		ui.Column(c).Gap(12).Children(func() {
			ui.Text(c, a.t("Quét và chạy nền", "Scanning and background")).FontSize(18).Bold()
			ui.Checkbox(c, &a.settings.AutoScan, a.t("Tự quét mỗi 2 giờ và thông báo", "Scan every 2 hours and notify"))
			ui.Checkbox(c, &a.settings.OpenAtLogin, a.t("Mở khi đăng nhập", "Open at login"))
			ui.Checkbox(c, &a.settings.ScanHome, a.t("Quét toàn bộ Home", "Scan all Home"))
			ui.Text(c, a.t("Thư mục tự chọn", "Selected folders")).Bold()
			for _, root := range a.settings.Roots {
				root := root
				ui.Row(c).Gap(8).Children(func() {
					ui.Text(c, root).Grow(1).SingleLine()
					if ui.Button(c, a.t("Bỏ", "Remove")).Clicked() {
						a.removeRoot(root)
					}
				})
			}
			if ui.Button(c, a.t("Thêm thư mục...", "Add folder...")).Clicked() {
				a.pickRoot()
			}
			ui.Divider(c)
			ui.Text(c, a.t("Quy tắc an toàn", "Safety rules")).FontSize(18).Bold()
			field := func(label string, value *string) {
				ui.Row(c).Gap(8).Children(func() { ui.Text(c, label).Width(240); ui.TextInput(c, value).Width(120) })
			}
			field(a.t("Dung lượng tối thiểu (MB)", "Minimum size (MB)"), &a.minSizeMB)
			field(a.t("Số ngày không sửa", "Minimum age (days)"), &a.minAgeDays)
			field(a.t("Đỏ khi trống ≤ GB", "Red when free ≤ GB"), &a.redGB)
			field(a.t("Vàng khi trống ≤ GB", "Yellow when free ≤ GB"), &a.yellowGB)
			ui.Text(c, a.t("Đường dẫn loại trừ, mỗi dòng một đường dẫn tuyệt đối", "Excluded absolute paths, one per line"))
			ui.TextArea(c, &a.exclusions).Height(110)
			if ui.PrimaryButton(c, a.t("Lưu cài đặt", "Save settings")).Clicked() {
				a.saveSettings()
			}
			ui.Text(c, a.t("Vùng hệ thống và dữ liệu ứng dụng luôn được bảo vệ.", "System and app data remain protected.")).FontSize(11).TextColor(t.TextMuted)
		})
	})
}

func (a *app) removeRoot(root string) {
	var roots []string
	for _, p := range a.settings.Roots {
		if p != root {
			roots = append(roots, p)
		}
	}
	a.settings.Roots = roots
}

func (a *app) pickRoot() {
	go func() {
		paths, err := mygo.Dialog.Open(mygo.OpenDialogOptions{Parent: a.win, Title: a.t("Chọn thư mục quét", "Choose scan folder"), Directory: true})
		if err != nil || len(paths) == 0 {
			return
		}
		path := filepath.Clean(paths[0])
		a.win.Update(func() {
			if cleaner.Protected(path, a.home, a.settings.Exclusions) {
				a.status = a.t("Thư mục này được bảo vệ.", "This folder is protected.")
				return
			}
			for _, p := range a.settings.Roots {
				if p == path {
					return
				}
			}
			a.settings.Roots = append(a.settings.Roots, path)
		})
	}()
}

func (a *app) saveSettings() {
	parse := func(value string) (int64, error) { return strconv.ParseInt(strings.TrimSpace(value), 10, 64) }
	size, e1 := parse(a.minSizeMB)
	age, e2 := parse(a.minAgeDays)
	red, e3 := parse(a.redGB)
	yellow, e4 := parse(a.yellowGB)
	if e1 != nil || e2 != nil || e3 != nil || e4 != nil {
		a.status = a.t("Ngưỡng phải là số nguyên dương.", "Thresholds must be positive integers.")
		return
	}
	s := a.settings
	s.MinSizeBytes, s.MinAgeDays, s.RedGB, s.YellowGB = size*1_000_000, int(age), red, yellow
	s.Exclusions = nil
	for _, line := range strings.Split(a.exclusions, "\n") {
		line = strings.TrimSpace(line)
		if line == "" {
			continue
		}
		if !filepath.IsAbs(line) {
			a.status = a.t("Đường dẫn loại trừ phải là đường dẫn tuyệt đối.", "Exclusions must be absolute paths.")
			return
		}
		s.Exclusions = append(s.Exclusions, filepath.Clean(line))
	}
	if err := s.Validate(); err != nil {
		a.status = err.Error()
		return
	}
	if err := a.store.SaveSettings(context.Background(), s); err != nil {
		a.status = err.Error()
		return
	}
	if err := mygo.App.SetOpenAtLogin(s.OpenAtLogin); err != nil {
		a.status = err.Error()
	} else {
		a.status = a.t("Đã lưu cài đặt.", "Settings saved.")
	}
	a.settings = s
	a.refreshVolumes()
	if s.AutoScan {
		a.maybeScan()
	}
}

func formatBytes(bytes int64) string {
	if bytes >= cleaner.GB {
		return fmt.Sprintf("%.1f GB", float64(bytes)/float64(cleaner.GB))
	}
	return fmt.Sprintf("%.0f MB", float64(bytes)/1_000_000)
}

func formatLastScan(t time.Time) string {
	if t.IsZero() {
		return "—"
	}
	return t.Local().Format("2006-01-02 15:04")
}
