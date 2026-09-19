//! Runtime Fluent catalogs and language selection. Human-readable strings live in locales/.
//! The executable loads those files once per thread (no translation is compiled into the binary).

use fluent_bundle::{FluentArgs, FluentBundle, FluentResource};
use std::{cell::RefCell, env, fs, path::{Path, PathBuf}};
use unic_langid::LanguageIdentifier;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Language {
    #[default]
    English,
    SimplifiedChinese,
}

struct Catalogs {
    english: Option<FluentBundle<FluentResource>>,
    chinese: Option<FluentBundle<FluentResource>>,
}

impl Catalogs {
    fn load(dir: &Path) -> Self {
        let english = Self::load_one(dir, "en-US");
        let chinese = Self::load_one(dir, "zh-CN");
        if english.is_none() {
            eprintln!("[i18n] English catalog unavailable at {}. Unresolved IDs will be displayed instead.", dir.display());
        }
        Self { english, chinese }
    }

    fn load_one(dir: &Path, tag: &str) -> Option<FluentBundle<FluentResource>> {
        let path = dir.join(tag).join("main.ftl");
        let source = match fs::read_to_string(&path) {
            Ok(source) => source,
            Err(error) => {
                eprintln!("[i18n] Cannot read {}: {error}", path.display());
                return None;
            }
        };
        let resource = match FluentResource::try_new(source) {
            Ok(resource) => resource,
            Err((_, errors)) => {
                eprintln!("[i18n] Invalid Fluent syntax in {}: {errors:?}", path.display());
                return None;
            }
        };
        let lang: LanguageIdentifier = match tag.parse() {
            Ok(lang) => lang,
            Err(error) => {
                eprintln!("[i18n] Invalid locale {tag}: {error}");
                return None;
            }
        };
        let mut bundle = FluentBundle::new(vec![lang]);
        // Keep existing UI text stable: Fluent normally adds invisible bidi isolates around variables.
        bundle.set_use_isolating(false);
        if let Err(errors) = bundle.add_resource(resource) {
            eprintln!("[i18n] Cannot add {}: {errors:?}", path.display());
            return None;
        }
        Some(bundle)
    }

    fn lookup(bundle: &FluentBundle<FluentResource>, key: &str, args: Option<&FluentArgs<'_>>) -> Option<String> {
        let pattern = bundle.get_message(key)?.value()?;
        let mut errors = Vec::new();
        let translated = bundle.format_pattern(pattern, args, &mut errors).into_owned();
        if !errors.is_empty() {
            eprintln!("[i18n] Invalid interpolation for {key}: {errors:?}");
            return None;
        }
        Some(translated)
    }

    fn tr(&self, language: Language, key: &str, args: Option<&FluentArgs<'_>>) -> String {
        if language == Language::SimplifiedChinese {
            if let Some(text) = self.chinese.as_ref().and_then(|bundle| Self::lookup(bundle, key, args)) {
                return text;
            }
        }
        self.english.as_ref().and_then(|bundle| Self::lookup(bundle, key, args)).unwrap_or_else(|| {
            eprintln!("[i18n] Missing English translation for {key}");
            key.to_owned()
        })
    }
}

// Thread-local avoids requiring a Sync Fluent memoizer; GUI lookups do not hit the filesystem.
thread_local! {
    static CATALOGS: RefCell<Catalogs> = RefCell::new(Catalogs::load(&locale_dir()));
}

fn locale_dir() -> PathBuf {
    if let Some(dir) = env::var_os("CODE_MAP_LOCALES_DIR") {
        return PathBuf::from(dir);
    }
    if let Ok(exe) = env::current_exe() {
        if let Some(parent) = exe.parent() {
            let beside_exe = parent.join("locales");
            if beside_exe.is_dir() { return beside_exe; }
        }
    }
    // cargo run puts the executable under target/{debug,release}, not beside locales.
    // First try the invocation directory, then this project's source directory.
    if let Ok(cwd) = env::current_dir() {
        let beside_cwd = cwd.join("locales");
        if beside_cwd.is_dir() { return beside_cwd; }
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("locales")
}

impl Language {
    pub fn detect() -> Self {
        let command_line = env::args().find_map(|arg| arg.strip_prefix("--lang=").and_then(Self::from_language_tag));
        if let Some(language) = command_line { return language; }
        if let Some(language) = env::var("CODE_MAP_LANG").ok().and_then(|tag| Self::from_language_tag(&tag)) {
            return language;
        }
        for name in ["LC_ALL", "LC_MESSAGES", "LANGUAGE", "LANG"] {
            if let Some(language) = env::var(name).ok().and_then(|tag| Self::from_language_tag(&tag)) {
                return language;
            }
        }
        #[cfg(target_os = "windows")]
        if let Some(language) = windows_locale().and_then(|tag| Self::from_language_tag(&tag)) {
            return language;
        }
        Self::English
    }

    pub fn from_index(index: usize) -> Self {
        match index { 1 => Self::SimplifiedChinese, _ => Self::English }
    }
    pub fn index(self) -> usize {
        match self { Self::English => 0, Self::SimplifiedChinese => 1 }
    }
    fn from_language_tag(tag: &str) -> Option<Self> {
        let tag = tag.split([':', '.', '@']).next().unwrap_or(tag).replace('_', "-").to_ascii_lowercase();
        if tag == "zh" || tag.starts_with("zh-") { Some(Self::SimplifiedChinese) }
        else if tag == "en" || tag.starts_with("en-") { Some(Self::English) }
        else { None }
    }

    pub fn tr(self, key: &str) -> String { self.tr_with(key, None) }
    fn tr_with(self, key: &str, args: Option<&FluentArgs<'_>>) -> String {
        CATALOGS.with(|catalogs| catalogs.borrow().tr(self, key, args))
    }

    pub fn search_placeholder(self) -> String { self.tr("search-placeholder") }
    pub fn color_modes(self) -> [String; 3] {
        [self.tr("color-file-type"), self.tr("color-recent"), self.tr("color-most")]
    }
    pub fn detail_levels(self) -> [String; 4] {
        [self.tr("detail-normal"), self.tr("detail-high"), self.tr("detail-ultra"), self.tr("detail-custom")]
    }
    pub fn language_names(self) -> [String; 2] {
        [self.tr("language-english"), self.tr("language-chinese")]
    }
    pub fn fit(self) -> String { self.tr("toolbar-fit") }
    pub fn show_ignored(self) -> String { self.tr("toolbar-show-ignored") }
    pub fn inspector(self) -> String { self.tr("inspector-heading") }
    pub fn select_hint(self) -> String { self.tr("inspector-select-hint") }
    pub fn help(self) -> String { self.tr("help") }
    pub fn custom_detail(self) -> String { self.tr("detail-heading") }
    pub fn geometry_detail(self) -> String { self.tr("detail-geometry") }
    pub fn text_detail(self) -> String { self.tr("detail-text") }
    pub fn render_budget(self) -> String { self.tr("detail-budget") }

    pub fn scanning(self, path: &str) -> String {
        let mut args = FluentArgs::new(); args.set("path", path);
        self.tr_with("status-scanning", Some(&args))
    }
    pub fn reading_history(self) -> String { self.tr("status-reading-history") }
    pub fn search_matches(self, count: &str, query: &str) -> String {
        let mut args = FluentArgs::new(); args.set("count", count); args.set("query", query);
        self.tr_with("status-search-matches", Some(&args))
    }
    pub fn summary(self, files: &str, lines: &str, commits: Option<&str>) -> String {
        let mut args = FluentArgs::new(); args.set("files", files); args.set("lines", lines);
        if let Some(commits) = commits {
            args.set("commits", commits);
            self.tr_with("status-summary-commits", Some(&args))
        } else {
            self.tr_with("status-summary", Some(&args))
        }
    }
    pub fn scanned(self, summary: &str, seconds: f64, used_git: bool) -> String {
        let seconds = format!("{seconds:.2}");
        let mut args = FluentArgs::new(); args.set("summary", summary); args.set("seconds", seconds.as_str());
        self.tr_with(if used_git { "status-scanned-git" } else { "status-scanned-no-git" }, Some(&args))
    }
    pub fn scan_failed(self, error: &str) -> String {
        let mut args = FluentArgs::new(); args.set("error", error);
        self.tr_with("status-scan-failed", Some(&args))
    }
    pub fn expanded_ignored(self, path: &str, count: &str) -> String {
        let mut args = FluentArgs::new(); args.set("path", path); args.set("count", count);
        self.tr_with("status-expanded-ignored", Some(&args))
    }
    pub fn history_failed(self, error: &str) -> String {
        let mut args = FluentArgs::new(); args.set("error", error);
        self.tr_with("status-history-failed", Some(&args))
    }
    pub fn no_folder(self) -> String { self.tr("status-no-folder") }
    pub fn folder_details(self, files: &str, lines: &str, bytes: &str, children: usize) -> String {
        let children = children.to_string();
        let mut args = FluentArgs::new();
        args.set("files", files); args.set("lines", lines); args.set("bytes", bytes); args.set("children", children.as_str());
        self.tr_with("info-folder", Some(&args))
    }
    pub fn text_file_details(self, lines: &str, comments: &str, bytes: &str) -> String {
        let mut args = FluentArgs::new(); args.set("lines", lines); args.set("comments", comments); args.set("bytes", bytes);
        self.tr_with("info-text-file", Some(&args))
    }
    pub fn binary_details(self, bytes: &str) -> String {
        let mut args = FluentArgs::new(); args.set("bytes", bytes);
        self.tr_with("info-binary", Some(&args))
    }
    pub fn ignored_details(self, directory: bool, loading: bool) -> String {
        let kind = if directory { "info-ignored-folder" } else { "info-ignored-file" };
        let status = if loading { "info-reading" } else { "info-not-read" };
        format!("{}\n{}", self.tr(kind), self.tr(status))
    }
    pub fn git_details(self, commits: &str, age: &str) -> String {
        let mut args = FluentArgs::new(); args.set("commits", commits); args.set("age", age);
        format!("\n\n{}", self.tr_with("info-git", Some(&args)))
    }
    pub fn no_commits(self) -> String { format!("\n\n{}", self.tr("info-no-commits")) }
    pub fn matched_gitignore(self) -> String { format!("\n\n{}", self.tr("info-matched-gitignore")) }
    pub fn reading_suffix(self) -> String { self.tr("label-reading") }
    pub fn ignored_suffix(self) -> String { self.tr("label-ignored") }
    pub fn bytes(self, bytes: u64) -> String {
        match bytes {
            b if b >= 1 << 30 => format!("{:.1} GB", b as f64 / (1u64 << 30) as f64),
            b if b >= 1 << 20 => format!("{:.1} MB", b as f64 / (1u64 << 20) as f64),
            b if b >= 1 << 10 => format!("{:.1} KB", b as f64 / 1024.0),
            b => { let count = b.to_string(); let mut args = FluentArgs::new(); args.set("count", count.as_str()); self.tr_with("unit-bytes", Some(&args)) }
        }
    }
    pub fn age(self, seconds: i64) -> String {
        let days = seconds / 86_400;
        let (key, count) = match days {
            d if d < 1 => return self.tr("age-today"),
            1 => return self.tr("age-yesterday"),
            d if d < 60 => ("age-days", d),
            d if d < 730 => ("age-months", d / 30),
            d => ("age-years", d / 365),
        };
        let count = count.to_string();
        let mut args = FluentArgs::new(); args.set("count", count.as_str());
        self.tr_with(key, Some(&args))
    }
}

#[cfg(target_os = "windows")]
fn windows_locale() -> Option<String> {
    #[link(name = "kernel32")]
    extern "system" {
        fn GetUserDefaultLocaleName(locale_name: *mut u16, locale_name_len: i32) -> i32;
    }
    let mut buffer = [0u16; 85];
    // SAFETY: Windows receives a valid writable buffer and its exact length.
    let len = unsafe { GetUserDefaultLocaleName(buffer.as_mut_ptr(), buffer.len() as i32) };
    if len <= 1 { None } else { String::from_utf16(&buffer[..len as usize - 1]).ok() }
}

#[cfg(test)]
mod tests {
    use super::{Catalogs, Language};
    use std::{fs, path::PathBuf, sync::atomic::{AtomicUsize, Ordering}};

    #[test]
    fn parses_supported_language_tags() {
        assert_eq!(Language::from_language_tag("zh_CN.UTF-8"), Some(Language::SimplifiedChinese));
        assert_eq!(Language::from_language_tag("zh-Hans-CN"), Some(Language::SimplifiedChinese));
        assert_eq!(Language::from_language_tag("en-US"), Some(Language::English));
        assert_eq!(Language::from_language_tag("de-DE"), None);
    }

    fn temp_catalogs(en: &str, zh: &str) -> (PathBuf, Catalogs) {
        static ID: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!("code-map-fluent-test-{}-{}", std::process::id(), ID.fetch_add(1, Ordering::Relaxed)));
        fs::create_dir_all(path.join("en-US")).unwrap();
        fs::create_dir_all(path.join("zh-CN")).unwrap();
        fs::write(path.join("en-US/main.ftl"), en).unwrap();
        fs::write(path.join("zh-CN/main.ftl"), zh).unwrap();
        let catalogs = Catalogs::load(&path);
        (path, catalogs)
    }

    #[test]
    fn loads_external_files_and_falls_back_to_english() {
        let (path, catalogs) = temp_catalogs("hello = English original\nmissing = English fallback\n", "hello = 用户自定义的中文\n");
        assert_eq!(catalogs.tr(Language::SimplifiedChinese, "hello", None), "用户自定义的中文");
        assert_eq!(catalogs.tr(Language::SimplifiedChinese, "missing", None), "English fallback");
        assert_eq!(catalogs.tr(Language::English, "hello", None), "English original");
        fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn malformed_chinese_catalog_uses_english_and_missing_english_shows_key() {
        let (path, catalogs) = temp_catalogs("hello = Original\n", "hello = { $broken\n");
        assert_eq!(catalogs.tr(Language::SimplifiedChinese, "hello", None), "Original");
        assert_eq!(catalogs.tr(Language::English, "unknown", None), "unknown");
        fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn catalog_keys_and_localized_dynamic_messages() {
        let english = fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("locales/en-US/main.ftl")).unwrap();
        let chinese = fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("locales/zh-CN/main.ftl")).unwrap();
        let en_keys: std::collections::HashSet<_> = english.lines().filter_map(|line| line.split_once(" = ").map(|(id, _)| id)).collect();
        let zh_keys: std::collections::HashSet<_> = chinese.lines().filter_map(|line| line.split_once(" = ").map(|(id, _)| id)).collect();
        assert_eq!(en_keys, zh_keys, "Language files have different message IDs");
        let (path, catalogs) = temp_catalogs(&english, &chinese);
        assert!(catalogs.english.is_some(), "English Fluent catalog must parse");
        assert!(catalogs.chinese.is_some(), "Chinese Fluent catalog must parse");
        assert_eq!(catalogs.tr(Language::SimplifiedChinese, "detail-heading", None), "自定义细节");
        assert_eq!(catalogs.tr(Language::SimplifiedChinese, "detail-text", None), "文字细节");
        assert_eq!(catalogs.tr(Language::SimplifiedChinese, "detail-budget", None), "渲染预算 (%)");
        let mut args = fluent_bundle::FluentArgs::new();
        args.set("files", "12"); args.set("lines", "345"); args.set("commits", "8");
        assert_eq!(catalogs.tr(Language::English, "status-summary-commits", Some(&args)), "12 files, 345 lines, 8 commits of history");
        assert_eq!(catalogs.tr(Language::SimplifiedChinese, "status-summary-commits", Some(&args)), "12 个文件，345 行，8 个历史提交");
        fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn language_switch_changes_translation_results() {
        assert_eq!(Language::English.detail_levels()[0], "Normal");
        assert_eq!(Language::SimplifiedChinese.detail_levels()[0], "普通");
        assert_eq!(Language::English.geometry_detail(), "Geometry detail");
        assert_eq!(Language::SimplifiedChinese.geometry_detail(), "几何细节");
        assert_eq!(Language::SimplifiedChinese.summary("12", "345", Some("8")), "12 个文件，345 行，8 个历史提交");
        assert_eq!(Language::SimplifiedChinese.search_matches("3", "地图"), "“地图”有 3 个匹配项。按 Enter 跳转到下一个。");
    }
}
