# Code Map: English source strings. Keep message IDs stable.
# Override this file in the published locales directory to customize wording.
detail-heading = CUSTOM DETAIL
detail-geometry = Geometry detail
detail-text = Text detail
detail-budget = Render budget (%)
toolbar-fit = Fit
language-english = English
language-chinese = 简体中文

search-placeholder = Search files and folders
color-file-type = File type
color-recent = Recently changed
color-most = Most changed
detail-normal = Normal
detail-high = High
detail-ultra = Ultra
detail-custom = Custom
toolbar-show-ignored = Show ignored
inspector-heading = INSPECTOR
inspector-select-hint = Click something on the map
help = Scroll: zoom
    Drag: pan (2D) or orbit (3D)
    Shift or right drag: pan (3D)
    Click: inspect
    Double click: fly to it
    Enter in search: next match
    Striped boxes are ignored by git.
    Click one to read it.

status-scanning = Scanning { $path } ...
status-reading-history = Reading git history ...
status-search-matches = { $count } matches for "{ $query }". Press Enter to fly to the next one.
status-summary-commits = { $files } files, { $lines } lines, { $commits } commits of history
status-summary = { $files } files, { $lines } lines
status-scanned-git = { $summary }, scanned in { $seconds }s (ignore rules from git)
status-scanned-no-git = { $summary }, scanned in { $seconds }s (no git: read .gitignore files)
status-scan-failed = Could not scan: { $error }
status-expanded-ignored = Read ignored { $path }: { $count } files
status-history-failed = No git history: { $error }
status-no-folder = No folder opened

info-folder = Folder
    { $files } files
    { $lines } lines
    { $bytes }
    { $children } direct children
info-text-file = Text file
    { $lines } lines ({ $comments } comment lines)
    { $bytes }
info-binary = Binary or very large file
    { $bytes }
info-ignored-folder = Ignored folder
info-ignored-file = Ignored file
info-reading = Reading it now ...
info-not-read = Not read yet. Click it to read it.
info-git = Git: { $commits } commits
    last changed { $age }
info-no-commits = Git: no commits in history
info-matched-gitignore = Matched by .gitignore
label-reading = reading...
label-ignored = ignored
unit-bytes = { $count } bytes
age-today = today
age-yesterday = yesterday
age-days = { $count } days ago
age-months = { $count } months ago
age-years = { $count } years ago
