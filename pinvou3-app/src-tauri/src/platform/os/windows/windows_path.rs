use std::ffi::{OsStr, OsString};
use std::path::{MAIN_SEPARATOR, Path, PathBuf};
use std::process::Command;

const PDF_TOOLS: &[&str] = &["pdftotext", "pdftoppm"];
const PANDOC_TOOL: &str = "pandoc";
const TESSERACT_TOOL: &str = "tesseract";
const ARCHIVE_TOOL: &str = "7z";

pub fn user_home_dir() -> PathBuf {
    if let Ok(home) = std::env::var("USERPROFILE") {
        if !home.trim().is_empty() {
            return PathBuf::from(home);
        }
    }
    if let (Ok(drive), Ok(path)) = (std::env::var("HOMEDRIVE"), std::env::var("HOMEPATH")) {
        let home = format!("{drive}{path}");
        if !home.trim().is_empty() {
            return PathBuf::from(home);
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        if !home.trim().is_empty() {
            return platform_compat_path(&home);
        }
    }
    std::env::temp_dir()
}

pub fn platform_compat_path(value: &str) -> PathBuf {
    let trimmed = value.trim();
    if let Some(rest) = trimmed.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{rest}"));
    }
    if let Some(rest) = trimmed.strip_prefix(r"\\?\") {
        return PathBuf::from(rest);
    }

    let normalized = trimmed.replace('\\', "/");
    if normalized == "/tmp" || normalized.starts_with("/tmp/") {
        let rest = normalized
            .trim_start_matches("/tmp")
            .trim_start_matches('/');
        return if rest.is_empty() {
            std::env::temp_dir()
        } else {
            std::env::temp_dir().join(rest.replace('/', "\\"))
        };
    }

    PathBuf::from(trimmed)
}

pub fn validate_upload_location(canon: &Path) -> Result<(), String> {
    for (label, value) in windows_sensitive_roots() {
        let root = platform_compat_path(
            &std::fs::canonicalize(&value)
                .unwrap_or_else(|_| value.clone())
                .to_string_lossy(),
        );
        if path_starts_with_case_insensitive(canon, &root) {
            return Err(format!(
                "path {} under sensitive system dir {}",
                canon.display(),
                label
            ));
        }
    }
    Ok(())
}

/// NTFS is case-insensitive by default: resolution follows the `$UpCase`
/// table (Unicode uppercase mappings) — besides the `ſ`→`s` and
/// KELVIN SIGN→`k` mappings shared with simple case folding, it also folds
/// ı (U+0131) to `I`, which the fold primitive covers as well. Shares the
/// interface-layer fold primitive with macOS so the lexical judgment matches
/// the volume's real resolution semantics.
pub fn path_component_eq(component: &OsStr, expected: &str) -> bool {
    super::super::interface::volume_fold_eq(component, expected)
}

pub fn filesystem_path_identity_key(path: &str) -> String {
    // `\\?\` and `\\?\UNC\` verbatim spellings (what std::fs::canonicalize
    // returns on this platform) denote the same file as the plain spelling,
    // an equivalence this OS guarantees — so the identity key must fold the
    // prefix or stored verbatim roots never compare equal to compat-spelled
    // candidates (compose's workspace ancestor grant failed exactly that way
    // on Windows: the recorded provenance root stayed `\\?\C:\...` while the
    // delivery judgment compared the compat spelling). The NT object
    // namespace is case-insensitive, so the prefix match itself must fold
    // too: a lowercase `\\?\unc\…` spelling is the same share as the
    // uppercase form (Round-15 review F3; the previous byte-exact strip
    // only mis-judged these into different keys, fail-closed direction).
    let folded_prefix = path.to_ascii_lowercase();
    let verbatim = folded_prefix.starts_with(r"\\?\unc\") || folded_prefix.starts_with(r"\\?\");
    let stripped = if folded_prefix.starts_with(r"\\?\unc\") {
        format!(r"\\{}", &path["\\\\?\\UNC\\".len()..])
    } else if folded_prefix.starts_with(r"\\?\") {
        path[r"\\?\".len()..].to_string()
    } else {
        path.to_string()
    };
    // Same spelling-fold surface as the lexical fold primitive (explicit
    // ſ/K/ı replacement + lowercase): NTFS resolves via $UpCase, and
    // guaranteed equivalences like ı→I were previously missed by
    // to_lowercase into different keys, silently dropping an already-granted
    // ancestor root (Round-15 review F2).
    //
    // Known residual (Round-15 review F1, documented; Round-17 review
    // completed the impact analysis): Rust's to_lowercase maps İ (U+0130)
    // to `i`+U+0307, giving it the same key as literal `i`+U+0307 — while
    // $UpCase treats the two as different files, so an over-fold happens
    // here (two distinct directories may share one key). The impact runs
    // both ways: the grant nesting judgment may treat a sibling directory
    // spelled with İ/ı̇ as an already-granted root (its content then becomes
    // accessible under that granted root), or may land an operation on the
    // new directory (no leak of the original root's content); in the
    // ancestor-exemption lane, an alternate-spelling sibling sharing a key
    // with the granted root may have its real content delivered as that root
    // (lib.rs compose workspace root). Requires an İ spelling and an
    // i+U+0307 spelling to coexist as siblings in the same NTFS directory —
    // extremely rare; perfectly emulating $UpCase needs a simple case
    // mapping outside std, so the residual is recorded as an accepted
    // tradeoff.
    //
    // Added in Round-23 review, corrected in Round-24 review: another member
    // of the same over-fold family is ẞ (U+1E9E, lowercase ß) — it shares a
    // key with the existing ß spelling while $UpCase treats them as
    // different files (same direction as İ: over-fold; likewise requires the
    // two spellings to coexist as siblings in one directory, extremely
    // rare). The Kelvin sign U+212A was once listed as a family member here;
    // Round-24 review corrects that: it has the simple uppercase mapping
    // U+004B, $UpCase is built exactly on simple case mappings, and NTFS
    // already resolves K/k/K (U+212A) to the same file, so folding it to `k`
    // here agrees with the filesystem — no divergence, not a residual. İ and
    // ẞ remain covered by the general $UpCase tradeoff above; the naming
    // convention matches round-21.
    //
    // Two further residuals recorded in Round-20 review (mostly
    // false-split/usability direction; spellings produced by the shipped app
    // are unreachable): (a) forward-slash device spellings (`//?/C:\doc.`,
    // `\?\UNC/server/…`) fold to a key matching neither the plain nor the
    // same file's verbatim key, becoming orphan keys; (b) Greek sigma
    // (ς/σ, in any position — `to_lowercase` additionally has a
    // word-final Σ→ς SpecialCasing rule whose net effect matches this
    // record, also a false-split; $UpCase equivalence pairs like µ/μ→Μ
    // belong to the same class): $UpCase folds both to Σ (one file) while
    // to_lowercase keeps them distinct, so alternate spellings of one file
    // split into different keys here.
    //
    // Round-21 review correction: the "no false-merge" claim above does not
    // hold for the device-spelling family on normalization-sensitive paths —
    // `//?/` and `\\.\` spellings (anything but the backslash form) do not
    // hit the verbatim prefix recognition (the ASCII-lowercased match
    // requires backslashes) and fall into the plain arm: `//?/C:\doc.`
    // (sensitive, dot stripped) and `//?/C:\doc` (after Win32 folds `/` to
    // `\` this is literally `\\?\C:\doc` = plain `C:\doc`, insensitive) share
    // one key — two genuinely different files false-merge, with the same
    // blast radius as the İ residual above (one file's content can be
    // delivered under the other's granted root). Mitigation: the shipped app
    // only produces canonicalize's `\\?\` spelling, so `//?/`/`\\.\` is
    // unreachable; it also requires `doc.` and literal `doc` siblings to
    // coexist in one directory (extremely rare). Same class: `\\.\C:\doc.`
    // and `\\.\C:\doc` (the `.` component is excluded from the sensitivity
    // judgment, so the device namespace takes the plain arm). Recorded as an
    // accepted, documented residual, not fixed in this wave (extending the
    // constant-prefix recognition is a behavior change needing its own
    // evaluation).
    //
    // Another false-split (recorded in round-21 review, no directional
    // risk): with a dot/space-suffixed component followed by `..` navigation,
    // per-component stripping disagrees with Win32's resolve-then-strip —
    // `C:\doc.\..\doc\f.txt` (Win32 resolves it to `C:\doc\f.txt`) keys to
    // `c:/doc/../doc/f.txt` while its resolved plain spelling keys to
    // `c:/doc/f.txt`: one file, two keys. The direction is fail-closed
    // (duplicate key → duplicate grant/dedup miss, no bypass) and the shape
    // is rare; this wave adds an assertion for the shape inside
    // identity_key_folds_verbatim_prefixes_to_the_plain_spelling
    // (C:\doc.\..\doc\f.txt keys to the `..`-preserving c:/doc/../doc/f.txt)
    // to lock the status quo — any strategy change turns it red. Round-25
    // review adds a narrower corner of the same family: a component made up
    // entirely of dots/spaces (`.. `, `...`) misses the `== ".."` special
    // case and becomes an empty component after ordinary-name stripping
    // (leaving a `c://b`-shaped orphan in the key), while Win32 parses
    // `.. ` as parent navigation — again a false-split, fail-closed, and
    // unreachable from app-produced (canonicalize) spellings; recorded
    // without a special-case branch for it.
    let folded = super::super::interface::volume_fold_spelling(&stripped.replace('\\', "/"));
    if !has_win32_normalization_sensitive_component(path) {
        return folded;
    }
    // The prefix fold above is only sound while that equivalence holds.
    // Win32 normalization strips a trailing `.` (or space) from every PLAIN
    // path component before resolution, so the plain spelling
    // `...\folder.\report.txt` resolves to `...\folder\report.txt`, while
    // the verbatim spelling keeps the literal `folder.` directory — two
    // different files that must not share one identity key, or key-based
    // dedup (`unique_existing_files` knowledge-import roots, grant/binding
    // identity matches) silently drops one of them. Mirror the OS: the
    // plain spelling is keyed by its NORMALIZED form (components stripped),
    // so plain dotted/spaced names compare equal to their resolved plain
    // spelling exactly as Win32 resolves them; the verbatim spelling is
    // literal, so its key keeps the unstripped components under a `\\?/`
    // namespace marker that no plain key can produce. (Round-15 fix wave
    // reconcile: the namespace guard was folded into this wave's shared
    // spelling-fold surface — neither direction weakens the other.)
    if verbatim {
        return format!(r"\\?/{folded}");
    }
    let normalized = stripped
        .split(['\\', '/'])
        .map(|component| {
            if component == "." || component == ".." {
                component.to_string()
            } else {
                component.trim_end_matches(['.', ' ']).to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("/");
    super::super::interface::volume_fold_spelling(&normalized)
}

/// Whether any component's spelling is rewritten by Win32 path
/// normalization on resolution (a trailing `.` or trailing space): for
/// those paths the verbatim `\\?\` prefix changes meaning instead of only
/// skipping redundant parsing, so `filesystem_path_identity_key` must not
/// fold the plain and verbatim spellings together. `.`/`..` navigation
/// components are excluded — they are lexical, never trailing-dot names,
/// and a verbatim spelling of one resolves to nothing rather than to a
/// distinct file.
fn has_win32_normalization_sensitive_component(path: &str) -> bool {
    path.split(['\\', '/']).any(|component| {
        component != "."
            && component != ".."
            && (component.ends_with('.') || component.ends_with(' '))
    })
}

/// NTFS is case-insensitive by default: component-prefix comparison uses the
/// same fold primitive as `path_component_eq`.
pub fn path_component_prefix(component: &OsStr, prefix: &str) -> bool {
    super::super::interface::volume_fold_prefix(component, prefix)
}

/// NTFS is case-insensitive by default: component-suffix comparison uses the
/// same fold primitive as `path_component_eq`.
pub fn path_component_suffix(component: &OsStr, suffix: &str) -> bool {
    super::super::interface::volume_fold_suffix(component, suffix)
}

/// The Windows null device name (the "read empty config" input for git and
/// other external tools).
pub fn null_device() -> &'static str {
    "NUL"
}

pub fn python_command() -> String {
    if let Ok(p) = std::env::var("PINVOU3_PYTHON") {
        if !p.is_empty() && is_valid_python_candidate(Path::new(&p)) {
            return p;
        }
    }

    if let Some(bundled) = bundled_python_path() {
        return bundled.to_string_lossy().into_owned();
    }

    resolve_system_python().unwrap_or_else(|| "pythonw".to_string())
}

pub fn connector_cli_command(cli_bin: &str, program: &str) -> Command {
    use std::os::windows::process::CommandExt;

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    let mut cmd = Command::new(connector_cli_program(cli_bin, program));
    cmd.creation_flags(CREATE_NO_WINDOW);
    apply_windows_connector_path(&mut cmd);
    cmd
}

fn connector_cli_program(cli_bin: &str, program: &str) -> OsString {
    if matches!(program, "npm" | "npx") {
        if let Some(path) = bundled_node_command(program) {
            return path.into_os_string();
        }
        return windows_npm_shim(program);
    }

    if !cli_bin.is_empty() && program == cli_bin {
        // Versioned asset store first (lock table pins the version, single
        // resolution point); legacy layout (unmigrated installs) second; if
        // neither exists, fall back to the existing npm global shim.
        if let Some(path) = crate::platform::connector_lock::locked_cli_path(cli_bin) {
            if path.is_file() {
                return path.into_os_string();
            }
        }
        if let Some(bin_dir) = crate::platform::paths::managed_connector_bin_dir() {
            let bundled = bin_dir.join(crate::platform::connector_lock::executable_name(cli_bin));
            if bundled.is_file() {
                return bundled.into_os_string();
            }
        }
        return windows_npm_shim(program);
    }

    program.into()
}

fn windows_npm_shim(program: &str) -> OsString {
    let shim = format!("{program}.cmd");
    for prefix in windows_npm_prefix_candidates() {
        let path = prefix.join(&shim);
        if path.is_file() {
            return path.into_os_string();
        }
    }
    shim.into()
}

pub fn apply_user_npm_prefix(cmd: &mut Command) {
    let prefix_from_env = windows_npm_prefix_from_env();
    let prefix = prefix_from_env.clone().or_else(default_windows_npm_prefix);

    if let Some(prefix) = prefix {
        let _ = std::fs::create_dir_all(&prefix);
        if prefix_from_env.is_none() {
            cmd.env("NPM_CONFIG_PREFIX", &prefix)
                .env("npm_config_prefix", &prefix);
        }

        let mut path_entries = vec![prefix];
        if let Some(node_dir) = bundled_node_dir() {
            path_entries.push(node_dir);
        }
        prepend_connector_path_entries(cmd, path_entries);
    } else if let Some(node_dir) = bundled_node_dir() {
        prepend_connector_path_entries(cmd, [node_dir]);
    }

    if std::env::var_os("NPM_CONFIG_CACHE").is_none()
        && std::env::var_os("npm_config_cache").is_none()
    {
        let cache = std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir)
            .join("pinvou3")
            .join("npm-cache");
        let _ = std::fs::create_dir_all(&cache);
        cmd.env("NPM_CONFIG_CACHE", &cache)
            .env("npm_config_cache", &cache);
    }
}

pub fn kill_pid_tree(pid: u32) {
    // Delegate to platform::process so the taskkill gets the same 2s budget
    // and null stdio wiring as every other bounded kill there; the local
    // `.output()` below had none, so an app-exit harvest could hang on a
    // wedged WMI/RPC. The fire-and-forget contract (return ()) is preserved.
    let _ = crate::platform::process::kill_process_tree(pid);
}

fn apply_windows_connector_path(cmd: &mut Command) {
    let mut entries = windows_npm_prefix_candidates();
    if let Some(node_dir) = bundled_node_dir() {
        entries.push(node_dir);
    }
    prepend_connector_path_entries(cmd, entries);
}

fn windows_npm_prefix_from_env() -> Option<PathBuf> {
    ["NPM_CONFIG_PREFIX", "npm_config_prefix"]
        .into_iter()
        .filter_map(std::env::var_os)
        .map(PathBuf::from)
        .find(|path| !path.as_os_str().is_empty())
}

fn default_windows_npm_prefix() -> Option<PathBuf> {
    std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .map(|path| path.join("npm"))
}

fn windows_npm_prefix_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(prefix) = windows_npm_prefix_from_env() {
        candidates.push(prefix);
    }
    if let Some(prefix) = default_windows_npm_prefix() {
        if !candidates
            .iter()
            .any(|existing| same_path(existing, &prefix))
        {
            candidates.push(prefix);
        }
    }
    candidates
}

pub(crate) fn bundled_node_dir() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .map(|exe| bundled_runtime_dir_for_exe(&exe, "node"))
        .filter(|path| path.is_dir())
}

fn bundled_node_command(program: &str) -> Option<PathBuf> {
    let filename = match program {
        "npm" => "npm.cmd",
        "npx" => "npx.cmd",
        _ => return None,
    };
    bundled_node_dir()
        .map(|dir| dir.join(filename))
        .filter(|path| path.is_file())
}

fn prepend_connector_path_entries(cmd: &mut Command, dirs: impl IntoIterator<Item = PathBuf>) {
    let mut paths: Vec<PathBuf> = Vec::new();
    for dir in dirs {
        if dir.as_os_str().is_empty() || paths.iter().any(|existing| same_path(existing, &dir)) {
            continue;
        }
        paths.push(dir);
    }

    if let Some(current) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&current) {
            if paths.iter().any(|existing| same_path(existing, &dir)) {
                continue;
            }
            paths.push(dir);
        }
    }

    if let Ok(joined) = std::env::join_paths(paths) {
        cmd.env("PATH", joined);
    }
}

/// Canonical Windows path equality (trailing separators ignored, ASCII
/// case-insensitive); shared with windows_system's PATH dedup.
pub(crate) fn same_path(left: &Path, right: &Path) -> bool {
    left.to_string_lossy()
        .trim_end_matches(['\\', '/'])
        .eq_ignore_ascii_case(right.to_string_lossy().trim_end_matches(['\\', '/']))
}

pub fn bundled_python_path() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|exe| bundled_python_path_for_exe(&exe))
}

pub fn bundled_onnxruntime_dylib_path() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .map(|exe| bundled_runtime_dir_for_exe(&exe, "onnxruntime").join("onnxruntime.dll"))
        .filter(|path| path.is_file())
}

pub fn configure_onnxruntime_dylib() -> Result<(), String> {
    if std::env::var("ORT_DYLIB_PATH")
        .ok()
        .is_some_and(|value| !value.trim().is_empty())
    {
        return Ok(());
    }
    // No runtime env write anymore: under edition 2024 a runtime write racing
    // uncoordinated concurrent readers (the foundation's child-process env
    // snapshots, WebKit/glib libc getenv) is a data race. ORT_DYLIB_PATH is
    // injected by `startup_configure_onnxruntime_dylib` in the
    // single-threaded startup window; reaching here means the bundled runtime
    // is missing or not shipped — return an error so knowledge semantic
    // search degrades gracefully.
    Err(
        "Windows ONNX Runtime CPU runtime is missing: runtime/onnxruntime/onnxruntime.dll"
            .to_string(),
    )
}

/// Write `ORT_DYLIB_PATH` in the single-threaded startup window (locating
/// the ONNX dylib for the embedding runtime; must run before any ort
/// initialization). Not written when the external environment already sets it
/// or the bundled dylib is missing; the missing case is funneled into the
/// runtime `configure_onnxruntime_dylib` Err.
pub fn startup_configure_onnxruntime_dylib() {
    if std::env::var("ORT_DYLIB_PATH")
        .ok()
        .is_some_and(|value| !value.trim().is_empty())
    {
        return;
    }
    let Some(path) = bundled_onnxruntime_dylib_path() else {
        return;
    };
    // SAFETY: called only from the single-threaded startup window
    // (startup_platform_env ← lib.rs `startup_process_env`, the run()/headless
    // startup sequence, before the first thread spawn); no concurrent env
    // readers.
    unsafe { std::env::set_var("ORT_DYLIB_PATH", &path) };
    eprintln!(
        "[platform] ONNX Runtime dynamic library pinned: {}",
        path.display()
    );
}

pub fn obsidian_config_path() -> Option<PathBuf> {
    std::env::var_os("APPDATA")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .map(|app_data| app_data.join("obsidian").join("obsidian.json"))
}

pub fn bundled_python_path_for_exe(exe_path: &Path) -> Option<PathBuf> {
    let path = bundled_runtime_dir_for_exe(exe_path, "python").join("pythonw.exe");
    is_valid_python_candidate(&path).then_some(path)
}

fn resolve_system_python() -> Option<String> {
    if let Ok(path_var) = std::env::var("PATH") {
        for name in ["pythonw.exe", "python.exe"] {
            for dir in std::env::split_paths(&path_var) {
                let cand = dir.join(name);
                if is_valid_python_candidate(&cand) {
                    return Some(cand.to_string_lossy().into_owned());
                }
            }
        }
    }

    let mut roots: Vec<PathBuf> = Vec::new();
    if let Ok(la) = std::env::var("LOCALAPPDATA") {
        roots.push(PathBuf::from(la).join("Programs").join("Python"));
    }
    if let Ok(pf) = std::env::var("ProgramFiles") {
        roots.push(PathBuf::from(pf));
    }
    for root in roots {
        if let Ok(rd) = std::fs::read_dir(&root) {
            let mut vers: Vec<PathBuf> = rd
                .flatten()
                .map(|e| e.path())
                .filter(|p| {
                    p.is_dir()
                        && p.file_name()
                            .and_then(|n| n.to_str())
                            .map(|n| n.starts_with("Python3"))
                            .unwrap_or(false)
                })
                .collect();
            vers.sort();
            for d in vers.iter().rev() {
                for name in ["pythonw.exe", "python.exe"] {
                    let cand = d.join(name);
                    if is_valid_python_candidate(&cand) {
                        return Some(cand.to_string_lossy().into_owned());
                    }
                }
            }
        }
    }

    if let Ok(path_var) = std::env::var("PATH") {
        for dir in std::env::split_paths(&path_var) {
            let cand = dir.join("py.exe");
            if cand.is_file() {
                return Some(cand.to_string_lossy().into_owned());
            }
        }
    }
    let winpy = PathBuf::from(r"C:\Windows\py.exe");
    if winpy.is_file() {
        return Some(winpy.to_string_lossy().into_owned());
    }
    None
}

fn is_valid_python_candidate(path: &Path) -> bool {
    path.is_file() && !is_windowsapps_python_alias(path)
}

fn is_windowsapps_python_alias(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    if !matches!(
        name.to_ascii_lowercase().as_str(),
        "python.exe" | "pythonw.exe" | "python3.exe" | "python3w.exe"
    ) {
        return false;
    }
    path.components().any(|component| {
        component
            .as_os_str()
            .to_string_lossy()
            .eq_ignore_ascii_case("WindowsApps")
    })
}

fn windows_sensitive_roots() -> Vec<(&'static str, PathBuf)> {
    [
        ("WINDIR", "WINDIR"),
        ("SystemRoot", "SystemRoot"),
        ("ProgramFiles", "ProgramFiles"),
        ("ProgramFiles(x86)", "ProgramFiles(x86)"),
        ("ProgramData", "ProgramData"),
    ]
    .into_iter()
    .filter_map(|(label, key)| {
        std::env::var(key)
            .ok()
            .filter(|value| !value.trim().is_empty())
            .map(|value| (label, PathBuf::from(value)))
    })
    .collect()
}

fn path_starts_with_case_insensitive(path: &Path, base: &Path) -> bool {
    let path = normalized_path_key(path);
    let base = normalized_path_key(base);
    path == base || path.starts_with(&(base + "/"))
}

fn normalized_path_key(path: &Path) -> String {
    // Round-21 review (residual record; the original body carried no note):
    // this matcher uses to_ascii_lowercase and does not go through the
    // volume_fold_* shared fold table — when base contains non-ASCII
    // case-equivalent characters ($UpCase equivalences: K Kelvin, ſ, ı,
    // etc.) it under-matches, fail-open in the sensitive-root exclusion
    // direction. Mitigation: callers canonicalize upstream first (both the
    // file_ingest root and candidates go through canonicalize; candidates
    // whose canonicalize fails are rejected even earlier by the readability
    // gate), and a successful canonicalize re-exposes the real spelling, so
    // the shape is unreachable on shipped call paths. Same family as the
    // identity-key İ residual: perfectly emulating $UpCase needs a simple
    // case mapping outside std; widening this to the shared fold table is a
    // behavior change, recorded as an accepted tradeoff.
    path.to_string_lossy()
        .replace('\\', "/")
        .trim_end_matches('/')
        .to_ascii_lowercase()
}

pub fn bundled_tesseract_dir() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .map(|exe| bundled_runtime_dir_for_exe(&exe, "tesseract"))
        .filter(|path| path.is_dir())
}

#[cfg(test)]
fn bundled_poppler_dir_for_exe(exe_path: &Path) -> PathBuf {
    bundled_runtime_dir_for_exe(exe_path, "poppler")
}

#[cfg(test)]
fn bundled_pandoc_dir_for_exe(exe_path: &Path) -> PathBuf {
    bundled_runtime_dir_for_exe(exe_path, "pandoc")
}

#[cfg(test)]
fn bundled_tesseract_dir_for_exe(exe_path: &Path) -> PathBuf {
    bundled_runtime_dir_for_exe(exe_path, "tesseract")
}

#[cfg(test)]
fn bundled_archive_dir_for_exe(exe_path: &Path) -> PathBuf {
    bundled_runtime_dir_for_exe(exe_path, "7zip")
}

pub fn bundled_pdf_tool_path(command: &str) -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|exe| bundled_pdf_tool_path_for_exe(&exe, command))
}

pub fn bundled_pandoc_tool_path() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|exe| bundled_pandoc_tool_path_for_exe(&exe))
}

pub fn bundled_asr_tool_path() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|exe| bundled_asr_tool_path_for_exe(&exe))
}

pub fn bundled_asr_backend_path() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|exe| bundled_asr_backend_path_for_exe(&exe))
}

pub fn user_asr_model_path() -> PathBuf {
    crate::platform::paths::pinvou3_home()
        .join("asr")
        .join(asr_q8_model_filename())
}

pub fn asr_model_path() -> PathBuf {
    if let Ok(path) = std::env::var("PINVOU3_SENSEVOICE_MODEL") {
        if !path.trim().is_empty() {
            return PathBuf::from(path.trim());
        }
    }
    std::env::current_exe()
        .ok()
        .map(|exe| {
            asr_model_path_for_exe_and_user_home(&exe, &crate::platform::paths::pinvou3_home())
        })
        .unwrap_or_else(user_asr_model_path)
}

pub fn asr_model_path_for_exe_and_user_home(exe_path: &Path, pinvou_home: &Path) -> PathBuf {
    let user_model = pinvou_home.join("asr").join(asr_q8_model_filename());
    if user_model.is_file() {
        return user_model;
    }
    bundled_asr_model_path_for_exe(exe_path).unwrap_or(user_model)
}

pub fn bundled_tesseract_tool_path() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|exe| bundled_tesseract_tool_path_for_exe(&exe))
}

pub fn bundled_archive_tool_path() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|exe| bundled_archive_tool_path_for_exe(&exe))
}

pub fn bundled_tessdata_dir() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|exe| bundled_tessdata_dir_for_exe(&exe))
}

pub fn bundled_pdf_tool_path_for_exe(exe_path: &Path, command: &str) -> Option<PathBuf> {
    let filename = pdf_tool_filename(command)?;
    let path = bundled_runtime_dir_for_exe(exe_path, "poppler").join(filename);
    path.is_file().then_some(path)
}

pub fn bundled_pandoc_tool_path_for_exe(exe_path: &Path) -> Option<PathBuf> {
    let path = bundled_runtime_dir_for_exe(exe_path, "pandoc").join(pandoc_tool_filename());
    path.is_file().then_some(path)
}

pub fn bundled_asr_tool_path_for_exe(exe_path: &Path) -> Option<PathBuf> {
    let path = bundled_runtime_dir_for_exe(exe_path, "asr").join(asr_tool_filename());
    path.is_file().then_some(path)
}

pub fn bundled_asr_backend_path_for_exe(exe_path: &Path) -> Option<PathBuf> {
    let path = bundled_runtime_dir_for_exe(exe_path, "asr").join("llama-funasr-sensevoice.exe");
    path.is_file().then_some(path)
}

pub fn bundled_asr_model_path_for_exe(exe_path: &Path) -> Option<PathBuf> {
    let dir = bundled_runtime_dir_for_exe(exe_path, "asr");
    [
        dir.join("models").join(asr_q8_model_filename()),
        dir.join("gguf").join(asr_q8_model_filename()),
        dir.join(asr_q8_model_filename()),
    ]
    .into_iter()
    .find(|path| path.is_file())
}

pub fn bundled_tesseract_tool_path_for_exe(exe_path: &Path) -> Option<PathBuf> {
    let path = bundled_runtime_dir_for_exe(exe_path, "tesseract").join(tesseract_tool_filename());
    path.is_file().then_some(path)
}

pub fn bundled_archive_tool_path_for_exe(exe_path: &Path) -> Option<PathBuf> {
    let path = bundled_runtime_dir_for_exe(exe_path, "7zip").join(archive_tool_filename());
    path.is_file().then_some(path)
}

pub fn bundled_tessdata_dir_for_exe(exe_path: &Path) -> Option<PathBuf> {
    let path = bundled_runtime_dir_for_exe(exe_path, "tesseract").join("tessdata");
    path.is_dir().then_some(path)
}

pub fn pdf_tool_path(command: &str) -> PathBuf {
    bundled_pdf_tool_path(command).unwrap_or_else(|| PathBuf::from(command))
}

pub fn pandoc_tool_path() -> PathBuf {
    bundled_pandoc_tool_path().unwrap_or_else(|| PathBuf::from(PANDOC_TOOL))
}

pub fn tesseract_tool_path() -> PathBuf {
    bundled_tesseract_tool_path().unwrap_or_else(|| PathBuf::from(TESSERACT_TOOL))
}

pub fn archive_tool_path() -> PathBuf {
    bundled_archive_tool_path().unwrap_or_else(|| PathBuf::from(ARCHIVE_TOOL))
}

pub fn bundled_tessdata_has_required_languages() -> bool {
    std::env::current_exe()
        .ok()
        .map(|exe| bundled_tessdata_has_required_languages_for_exe(&exe))
        .unwrap_or(false)
}

pub fn bundled_tessdata_has_required_languages_for_exe(exe_path: &Path) -> bool {
    let Some(dir) = bundled_tessdata_dir_for_exe(exe_path) else {
        return false;
    };
    dir.join("chi_sim.traineddata").is_file() && dir.join("eng.traineddata").is_file()
}

fn bundled_runtime_dir_for_exe(exe_path: &Path, name: &str) -> PathBuf {
    install_dir_for_exe(exe_path).join("runtime").join(name)
}

fn install_dir_for_exe(exe_path: &Path) -> &Path {
    exe_path.parent().unwrap_or_else(|| Path::new("."))
}

fn pdf_tool_filename(command: &str) -> Option<String> {
    if command.contains(['/', '\\', MAIN_SEPARATOR]) {
        return None;
    }
    let stem = command.strip_suffix(".exe").unwrap_or(command);
    PDF_TOOLS
        .iter()
        .any(|tool| tool.eq_ignore_ascii_case(stem))
        .then(|| format!("{stem}.exe"))
}

fn pandoc_tool_filename() -> &'static str {
    "pandoc.exe"
}

fn asr_tool_filename() -> &'static str {
    "pinvou-asr.exe"
}

fn asr_q8_model_filename() -> &'static str {
    "sensevoice-small-q8.gguf"
}

fn tesseract_tool_filename() -> &'static str {
    "tesseract.exe"
}

fn archive_tool_filename() -> &'static str {
    "7z.exe"
}

#[cfg(test)]
mod tests {
    use super::*;
    // No local ENV_LOCK anymore: borrow the crate's single
    // platform::paths::tests::ENV_LOCK instead, sharing one lock with every
    // test that mutates env vars (PINVOU3_PYTHON here is an env write),
    // eliminating the data race under parallel tests.

    fn test_temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "pinvou3-windows-path-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn identity_key_folds_verbatim_prefixes_to_the_plain_spelling() {
        // The verbatim spellings std::fs::canonicalize returns denote the
        // same file as the plain spelling, so stored verbatim roots and
        // compat-spelled candidates must collapse onto one identity key —
        // otherwise every stored-vs-resolved comparison (compose's workspace
        // ancestor grant, project backing checks) silently misses on
        // Windows while staying green on prefix-free POSIX spellings.
        let plain = filesystem_path_identity_key(r"C:\Users\pinvou\project");
        assert_eq!(
            filesystem_path_identity_key(r"\\?\C:\Users\pinvou\project"),
            plain,
            "the verbatim drive prefix denotes the same file as the plain spelling"
        );
        assert_eq!(
            filesystem_path_identity_key(r"\\?\UNC\server\share\dir"),
            filesystem_path_identity_key(r"\\server\share\dir"),
            "the verbatim UNC prefix denotes the same file as the plain UNC spelling"
        );
        // Round-15 review F3: the NT object namespace is case-insensitive, a
        // lowercase verbatim prefix spells the same share, and the identity
        // key must fold it to the same key (the previous byte-exact strip
        // produced a separate key — a fail-closed miss).
        assert_eq!(
            filesystem_path_identity_key(r"\\?\unc\server\share\dir"),
            filesystem_path_identity_key(r"\\server\share\dir"),
            "a lowercase verbatim-UNC spelling is the same NT path and must fold"
        );
        assert_eq!(
            filesystem_path_identity_key(r"\\?\c:\Users\pinvou\project"),
            plain,
            "a lowercase verbatim drive prefix is the same NT path and must fold"
        );
        // Round-15 review F2: NTFS $UpCase folds ı to I, so different
        // spellings of one file must share one key (to_lowercase previously
        // missed this and silently dropped granted ancestor roots); same
        // table as path_component_eq's volume_fold_spelling.
        assert_eq!(
            filesystem_path_identity_key(r"C:\Users\p\ıd_rsa"),
            filesystem_path_identity_key(r"C:\Users\p\ID_RSA"),
            "dotless-i folds to I under $UpCase, matching the volume-fold primitive"
        );
        // Round-21 review: the known false-split of a dot/space-suffixed
        // component followed by `..` navigation (the status-quo lock from
        // the "another false-split" paragraph in the function comment
        // above): per-component stripping keeps the `..`, keying to
        // `c:/doc/../doc/f.txt`; Win32's resolve-then-strip real shape is
        // `C:\doc\f.txt` (key `c:/doc/f.txt`) — one file, two keys,
        // fail-closed direction. This pin locks the status quo (changing
        // the keying strategy is a behavior change that must consciously
        // turn this pin red), not an always-true contract.
        assert_eq!(
            filesystem_path_identity_key(r"C:\doc.\..\doc\f.txt"),
            "c:/doc/../doc/f.txt",
            "the dotted-component + `..` shape keys per-component (documented \
             false-split residual; changing the strategy is a behavior change)"
        );
        // Separators and case fold exactly as before the prefix stripping.
        assert_eq!(
            filesystem_path_identity_key(r"C:\USERS\PINVOU\project"),
            plain,
            "NTFS case-insensitivity still folds"
        );
        assert_eq!(
            filesystem_path_identity_key(r"C:/Users/pinvou/project"),
            plain,
            "forward separators still fold"
        );
    }

    #[test]
    fn identity_key_keeps_namespace_spellings_distinct_for_sensitive_components() {
        // Negative regression for the fold that
        // `identity_key_folds_verbatim_prefixes_to_the_plain_spelling`
        // pins for ordinary paths. Win32 normalization strips a trailing
        // `.`/space from every PLAIN component, so the plain spelling
        // `...\folder.\report.txt` resolves to `...\folder\report.txt`
        // while the verbatim `\\?\...\folder.\report.txt` keeps the
        // literal `folder.` directory — two different files. The prefix
        // fold must NOT collapse the namespace spellings once such a
        // component appears, or key-based dedup (`unique_existing_files`
        // import roots, grant/binding identity) silently drops one source.
        let plain_sensitive = filesystem_path_identity_key(r"C:\data\import\folder.\report.txt");
        let verbatim_sensitive =
            filesystem_path_identity_key(r"\\?\C:\data\import\folder.\report.txt");
        assert_ne!(
            plain_sensitive, verbatim_sensitive,
            "plain and verbatim spellings of a trailing-dot path denote different files"
        );
        // The plain spelling is keyed by its NORMALIZED form: Win32 strips
        // the trailing `.`/space before resolution, so the dotted spelling
        // and the dotless spelling are one file and must share one key
        // (mutation pin for the plain-side normalization — losing it would
        // over-split dedup for the same file).
        assert_eq!(
            filesystem_path_identity_key(r"C:\data\import\folder.\report.txt"),
            filesystem_path_identity_key(r"C:\data\import\folder\report.txt"),
            "plain dotted spelling resolves to the dotless name — one file, one key"
        );
        assert_eq!(
            filesystem_path_identity_key(r"C:\data\import\folder \report.txt"),
            filesystem_path_identity_key(r"C:\data\import\folder\report.txt"),
            "plain trailing-space spelling resolves to the stripped name — one file, one key"
        );
        // Within one namespace the fold still applies: case and separators
        // fold exactly as before, only the namespace forms stay apart.
        assert_eq!(
            filesystem_path_identity_key(r"C:\DATA\import\folder.\report.txt"),
            plain_sensitive,
            "case folding still applies to the plain spelling"
        );
        assert_eq!(
            filesystem_path_identity_key(r"\\?\C:/data/import/folder./report.txt"),
            verbatim_sensitive,
            "separator folding still applies to the verbatim spelling"
        );
        // The guard is component-targeted: a verbatim spelling of an
        // ordinary path (no trailing dot/space) keeps folding onto the
        // plain key — that equivalence really is OS-guaranteed.
        assert_eq!(
            filesystem_path_identity_key(r"\\?\C:\data\import\folder\report.txt"),
            filesystem_path_identity_key(r"C:\data\import\folder\report.txt"),
            "ordinary paths keep the plain/verbatim fold"
        );
        // Trailing-space components are equally normalization-sensitive.
        assert_ne!(
            filesystem_path_identity_key(r"C:\data\import\folder \report.txt"),
            filesystem_path_identity_key(r"\\?\C:\data\import\folder \report.txt"),
            "trailing-space components keep the namespace forms distinct too"
        );
        // UNC spellings get the same namespace distinction.
        assert_ne!(
            filesystem_path_identity_key(r"\\server\share\folder.\report.txt"),
            filesystem_path_identity_key(r"\\?\UNC\server\share\folder.\report.txt"),
            "verbatim UNC spellings of sensitive paths stay distinct from plain UNC"
        );
        // `.`/`..` navigation components are not trailing-dot names: they
        // must not flip a path into the marked verbatim namespace.
        assert_eq!(
            filesystem_path_identity_key(r"\\?\C:\data\.\folder\..\folder\report.txt"),
            filesystem_path_identity_key(r"C:\data\.\folder\..\folder\report.txt"),
            "navigation components are not normalization-sensitive names"
        );
    }

    #[test]
    fn unix_tmp_path_maps_to_temp_dir() {
        assert_eq!(
            platform_compat_path("/tmp/pinvou3-test-override"),
            std::env::temp_dir().join("pinvou3-test-override")
        );
    }

    #[test]
    fn bundled_poppler_dir_handles_spaces_and_chinese_chars() {
        let exe = Path::new(r"C:\Program Files\品眸 pinvou\pinvou3.exe");
        assert_eq!(
            bundled_poppler_dir_for_exe(exe),
            PathBuf::from(r"C:\Program Files\品眸 pinvou")
                .join("runtime")
                .join("poppler")
        );
    }

    #[test]
    fn bundled_pandoc_dir_handles_spaces_and_chinese_chars() {
        let exe = Path::new(r"C:\Program Files\品眸 pinvou\pinvou3.exe");
        assert_eq!(
            bundled_pandoc_dir_for_exe(exe),
            PathBuf::from(r"C:\Program Files\品眸 pinvou")
                .join("runtime")
                .join("pandoc")
        );
    }

    #[test]
    fn python_command_respects_valid_pinvou3_python() {
        let _g = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let dir = test_temp_dir("pinvou3-python-env");
        let python = dir.join("pythonw.exe");
        std::fs::write(&python, b"").unwrap();
        let prev = std::env::var("PINVOU3_PYTHON").ok();

        // SAFETY: holding platform::paths::tests::ENV_LOCK (see _g above); env
        // writes serialized.
        unsafe { std::env::set_var("PINVOU3_PYTHON", &python) };
        assert_eq!(python_command(), python.to_string_lossy());

        match prev {
            // SAFETY: same as above; restore-side write under ENV_LOCK serialization.
            Some(v) => unsafe { std::env::set_var("PINVOU3_PYTHON", v) },
            None => unsafe { std::env::remove_var("PINVOU3_PYTHON") },
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn bundled_python_uses_runtime_python_dir() {
        let dir = test_temp_dir("bundled-python");
        let exe = dir.join("pinvou3-tauri.exe");
        let python_dir = dir.join("runtime").join("python");
        std::fs::create_dir_all(&python_dir).unwrap();
        let python = python_dir.join("pythonw.exe");
        std::fs::write(&exe, b"").unwrap();
        std::fs::write(&python, b"").unwrap();

        assert_eq!(bundled_python_path_for_exe(&exe), Some(python));

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn bundled_onnxruntime_uses_runtime_onnxruntime_dir() {
        let dir = test_temp_dir("bundled-onnxruntime");
        let exe = dir.join("pinvou3-tauri.exe");
        let ort_dir = dir.join("runtime").join("onnxruntime");
        std::fs::create_dir_all(&ort_dir).unwrap();
        let ort = ort_dir.join("onnxruntime.dll");
        std::fs::write(&exe, b"").unwrap();
        std::fs::write(&ort, b"").unwrap();

        let resolved = bundled_runtime_dir_for_exe(&exe, "onnxruntime").join("onnxruntime.dll");
        assert_eq!(resolved, ort);

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn windowsapps_python_alias_is_not_valid_candidate() {
        let dir = test_temp_dir("windowsapps-alias");
        let alias_dir = dir.join("Microsoft").join("WindowsApps");
        std::fs::create_dir_all(&alias_dir).unwrap();
        let alias = alias_dir.join("python.exe");
        std::fs::write(&alias, b"").unwrap();

        assert!(is_windowsapps_python_alias(&alias));
        assert!(!is_valid_python_candidate(&alias));

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn extended_length_path_maps_to_normal_windows_path() {
        assert_eq!(
            platform_compat_path(r"\\?\C:\Users\z27014\Downloads\a.pdf"),
            PathBuf::from(r"C:\Users\z27014\Downloads\a.pdf")
        );
        assert_eq!(
            platform_compat_path(r"\\?\UNC\server\share\a.pdf"),
            PathBuf::from(r"\\server\share\a.pdf")
        );
    }

    #[test]
    fn upload_location_allows_windows_upload_file_outside_home() {
        let file = Path::new(r"D:\company docs\report.pptx");
        assert!(validate_upload_location(file).is_ok());
    }

    #[test]
    fn upload_location_matches_windows_roots_case_insensitively() {
        assert!(path_starts_with_case_insensitive(
            Path::new(r"C:\WINDOWS\System32\drivers\etc\hosts"),
            Path::new(r"c:\windows")
        ));
        assert!(!path_starts_with_case_insensitive(
            Path::new(r"D:\company docs\report.pptx"),
            Path::new(r"C:\Windows")
        ));
    }

    #[test]
    fn bundled_pdf_tool_path_prefers_bundled_exe() {
        let root =
            std::env::temp_dir().join(format!("pinvou3 poppler 路径测试 {}", std::process::id()));
        let poppler = root.join("runtime").join("poppler");
        std::fs::create_dir_all(&poppler).unwrap();
        let tool = poppler.join("pdftotext.exe");
        std::fs::write(&tool, b"fake exe").unwrap();
        let exe = root.join("pinvou3.exe");

        assert_eq!(bundled_pdf_tool_path_for_exe(&exe, "pdftotext"), Some(tool));

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn bundled_pandoc_tool_path_prefers_bundled_exe() {
        let root =
            std::env::temp_dir().join(format!("pinvou3 pandoc 路径测试 {}", std::process::id()));
        let pandoc = root.join("runtime").join("pandoc");
        std::fs::create_dir_all(&pandoc).unwrap();
        let tool = pandoc.join("pandoc.exe");
        std::fs::write(&tool, b"fake exe").unwrap();
        let exe = root.join("pinvou3.exe");

        assert_eq!(bundled_pandoc_tool_path_for_exe(&exe), Some(tool));

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn bundled_tesseract_dir_handles_spaces_and_chinese_chars() {
        let exe = Path::new(r"C:\Program Files\pinvou app\pinvou3.exe");
        assert_eq!(
            bundled_tesseract_dir_for_exe(exe),
            PathBuf::from(r"C:\Program Files\pinvou app")
                .join("runtime")
                .join("tesseract")
        );
    }

    #[test]
    fn bundled_tesseract_tool_and_tessdata_paths_prefer_bundled_runtime() {
        let root = std::env::temp_dir().join(format!(
            "pinvou3 tesseract path test {}",
            std::process::id()
        ));
        let tesseract = root.join("runtime").join("tesseract");
        let tessdata = tesseract.join("tessdata");
        std::fs::create_dir_all(&tessdata).unwrap();
        let tool = tesseract.join("tesseract.exe");
        let chi = tessdata.join("chi_sim.traineddata");
        let eng = tessdata.join("eng.traineddata");
        std::fs::write(&tool, b"fake exe").unwrap();
        std::fs::write(&chi, b"fake chi").unwrap();
        std::fs::write(&eng, b"fake eng").unwrap();
        let exe = root.join("pinvou3.exe");

        assert_eq!(bundled_tesseract_tool_path_for_exe(&exe), Some(tool));
        assert_eq!(bundled_tessdata_dir_for_exe(&exe), Some(tessdata));
        assert!(bundled_tessdata_has_required_languages_for_exe(&exe));

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn bundled_asr_paths_allow_runtime_without_q8_model() {
        let root =
            std::env::temp_dir().join(format!("pinvou3 asr path test {}", std::process::id()));
        let asr = root.join("runtime").join("asr");
        let models = asr.join("models");
        std::fs::create_dir_all(&models).unwrap();
        let wrapper = asr.join("pinvou-asr.exe");
        let backend = asr.join("llama-funasr-sensevoice.exe");
        let model = models.join("sensevoice-small-q8.gguf");
        std::fs::write(&wrapper, b"fake wrapper").unwrap();
        std::fs::write(&backend, b"fake backend").unwrap();
        std::fs::write(&model, b"fake model").unwrap();
        let exe = root.join("pinvou3.exe");

        assert_eq!(bundled_asr_tool_path_for_exe(&exe), Some(wrapper));
        assert_eq!(bundled_asr_backend_path_for_exe(&exe), Some(backend));
        assert_eq!(bundled_asr_model_path_for_exe(&exe), Some(model));

        std::fs::remove_file(models.join("sensevoice-small-q8.gguf")).unwrap();
        assert!(bundled_asr_tool_path_for_exe(&exe).is_some());
        assert!(bundled_asr_backend_path_for_exe(&exe).is_some());
        assert_eq!(bundled_asr_model_path_for_exe(&exe), None);

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn asr_model_path_prefers_user_model_over_bundled_model() {
        let root = std::env::temp_dir().join(format!(
            "pinvou3 asr user model path {}",
            std::process::id()
        ));
        let install_asr = root
            .join("install")
            .join("runtime")
            .join("asr")
            .join("models");
        let user_asr = root.join("home").join("asr");
        std::fs::create_dir_all(&install_asr).unwrap();
        std::fs::create_dir_all(&user_asr).unwrap();
        let bundled = install_asr.join("sensevoice-small-q8.gguf");
        let user = user_asr.join("sensevoice-small-q8.gguf");
        std::fs::write(&bundled, b"bundled").unwrap();
        std::fs::write(&user, b"user").unwrap();
        let exe = root.join("install").join("pinvou3.exe");

        assert_eq!(
            asr_model_path_for_exe_and_user_home(&exe, &root.join("home")),
            user
        );

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn asr_model_path_falls_back_to_bundled_runtime_model() {
        let root = std::env::temp_dir().join(format!(
            "pinvou3 asr bundled model path {}",
            std::process::id()
        ));
        let install_asr = root
            .join("install")
            .join("runtime")
            .join("asr")
            .join("models");
        std::fs::create_dir_all(&install_asr).unwrap();
        let bundled = install_asr.join("sensevoice-small-q8.gguf");
        std::fs::write(&bundled, b"bundled").unwrap();
        let exe = root.join("install").join("pinvou3.exe");

        assert_eq!(
            asr_model_path_for_exe_and_user_home(&exe, &root.join("home")),
            bundled
        );

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn bundled_archive_tool_path_prefers_bundled_7z() {
        let root =
            std::env::temp_dir().join(format!("pinvou3 7zip path test {}", std::process::id()));
        let archive = root.join("runtime").join("7zip");
        std::fs::create_dir_all(&archive).unwrap();
        let tool = archive.join("7z.exe");
        std::fs::write(&tool, b"fake exe").unwrap();
        let exe = root.join("pinvou3.exe");

        assert_eq!(bundled_archive_dir_for_exe(&exe), archive);
        assert_eq!(bundled_archive_tool_path_for_exe(&exe), Some(tool));

        std::fs::remove_dir_all(&root).ok();
    }
}
