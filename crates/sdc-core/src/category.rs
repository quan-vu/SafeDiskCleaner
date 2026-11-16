//! File type category, decided by extension only (no file content is read).

use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Category {
    App,
    Document,
    Video,
    Audio,
    Image,
    Archive,
    Other,
}

impl Category {
    pub const ALL: [Category; 7] = [
        Category::App,
        Category::Document,
        Category::Video,
        Category::Audio,
        Category::Image,
        Category::Archive,
        Category::Other,
    ];

    pub fn of(path: &Path) -> Category {
        let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
        match ext.as_str() {
            "dmg" | "pkg" | "mpkg" | "ipa" | "xip" | "app" | "iso" | "apk" | "exe" | "msi" => Category::App,
            "pdf" | "doc" | "docx" | "xls" | "xlsx" | "ppt" | "pptx" | "key" | "pages" | "numbers" | "txt" | "rtf"
            | "epub" | "csv" | "md" | "odt" | "ods" | "odp" => Category::Document,
            "mp4" | "mov" | "mkv" | "avi" | "m4v" | "webm" | "wmv" | "flv" => Category::Video,
            "mp3" | "wav" | "flac" | "aac" | "m4a" | "aiff" | "ogg" => Category::Audio,
            "jpg" | "jpeg" | "png" | "heic" | "gif" | "tif" | "tiff" | "raw" | "dng" | "psd" | "webp" | "cr2"
            | "nef" | "arw" => Category::Image,
            "zip" | "rar" | "7z" | "tar" | "gz" | "tgz" | "bz2" | "xz" | "zst" => Category::Archive,
            _ => Category::Other,
        }
    }

    /// (Vietnamese, English) display names.
    pub fn names(self) -> (&'static str, &'static str) {
        match self {
            Category::App => ("Ứng dụng", "App"),
            Category::Document => ("Tài liệu", "Document"),
            Category::Video => ("Video", "Video"),
            Category::Audio => ("Âm thanh", "Audio"),
            Category::Image => ("Hình ảnh", "Image"),
            Category::Archive => ("Nén", "Archive"),
            Category::Other => ("Khác", "Other"),
        }
    }

    pub fn symbol(self) -> &'static str {
        match self {
            Category::App => "app.badge",
            Category::Document => "doc.text",
            Category::Video => "film",
            Category::Audio => "waveform",
            Category::Image => "photo",
            Category::Archive => "archivebox",
            Category::Other => "doc",
        }
    }
}
