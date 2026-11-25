//! File downloads for the embedded system WebView.
//!
//! wry reports every download on the native UI thread and expects an immediate
//! allow/deny answer, so the destination question is split in two:
//!
//! 1. `Requested` points the engine at a private staging file. The system
//!    WebView streams the transfer to disk itself; nothing is buffered by the
//!    app and no size limit is applied.
//! 2. A deferred, non-blocking OS "Save" dialog asks the user where the file
//!    belongs. The staged file is moved there once the user's choice and the
//!    finished transfer both exist.
//!
//! The dialog is never opened from the engine callback: its blocking form must
//! not run on the UI thread, and a second download arriving meanwhile would
//! re-enter wry's borrowed handler. Dialogs are also serialized. A page that
//! fires several downloads gets one prompt at a time while every transfer is
//! already streaming.
//!
//! Events carry an opaque download id, the file name, and an outcome. The URL
//! (which may hold one-time tokens) and local paths stay in this process.

use std::collections::{HashMap, VecDeque};
use std::ffi::OsStr;
use std::io;
use std::path::{Path, PathBuf};

use parking_lot::Mutex;
use serde_json::{Value, json};
use tauri::{AppHandle, Emitter, Manager, Url};
use tauri_plugin_dialog::DialogExt;

use crate::platform::strings::truncate_utf8;

const EVENT_STARTED: &str = "browser:download-started";
const EVENT_FINISHED: &str = "browser:download-finished";
/// No staging file could be created, so the engine download is cancelled.
const EVENT_BLOCKED: &str = "browser:download-blocked";
const MAIN_WINDOW: &str = "main";
const FALLBACK_FILE_NAME: &str = "download";
/// Longest suggested name kept for staging and for the dialog. It bounds one
/// path component only; the transfer size is never limited.
const MAX_FILE_NAME_BYTES: usize = 120;
/// Longer "extensions" are treated as part of the name, so truncating a long
/// name cannot spend its whole budget on a suffix.
const MAX_EXTENSION_BYTES: usize = 24;

/// The page that requested a download. Identifies the UI surface that shows
/// the notice.
pub(crate) struct DownloadPage {
    pub(crate) session_id: String,
    pub(crate) tab: String,
}

/// UI-facing identity of a download. Never the URL or a local path.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Notice {
    id: String,
    session_id: String,
    tab: String,
    file_name: String,
}

impl Notice {
    fn payload(&self) -> Value {
        json!({
            "sessionId": self.session_id,
            "tab": self.tab,
            "downloadId": self.id,
            "fileName": self.file_name,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Transfer {
    InFlight,
    Succeeded,
    Failed,
}

#[derive(Debug, PartialEq, Eq)]
enum Intent {
    /// The save dialog is queued or open. `again` marks a retry after a
    /// placement failed.
    Asking {
        again: bool,
    },
    /// The transfer is complete but has no place yet; it waits for a retry.
    Unsaved,
    SaveTo(PathBuf),
    /// A background task is placing the file.
    Placing,
    /// The user declined. The transfer is dropped when it ends.
    Discard,
}

#[derive(Debug)]
struct Download {
    notice: Notice,
    /// Matches `Finished` on engines that do not report the written path.
    url: String,
    staging_dir: PathBuf,
    staged_file: PathBuf,
    transfer: Transfer,
    intent: Intent,
    /// Breaks ties when several transfers share a URL.
    order: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    Saved,
    Cancelled,
    Failed,
    Unsaved,
}

impl Outcome {
    fn as_str(self) -> &'static str {
        match self {
            Self::Saved => "saved",
            Self::Cancelled => "cancelled",
            Self::Failed => "failed",
            Self::Unsaved => "unsaved",
        }
    }
}

/// Work for the host. Everything here runs outside the registry lock.
#[derive(Debug, PartialEq, Eq)]
enum Effect {
    Started(Notice),
    Finished {
        notice: Notice,
        outcome: Outcome,
    },
    /// Open the save dialog. Always the last effect of a batch.
    Prompt(Notice),
    /// Move a finished transfer to the chosen path.
    Place {
        id: String,
        staging_dir: PathBuf,
        staged_file: PathBuf,
        destination: PathBuf,
    },
    Discard(PathBuf),
}

/// What a record is ready for.
#[derive(Debug, PartialEq, Eq)]
enum Step {
    Wait,
    Place(PathBuf),
    /// `None` stays silent because the outcome was already reported.
    Discard(Option<Outcome>),
}

fn step(download: &Download) -> Step {
    match (download.transfer, &download.intent) {
        (Transfer::Failed, intent) => {
            Step::Discard((intent != &Intent::Discard).then_some(Outcome::Failed))
        }
        (Transfer::Succeeded, Intent::SaveTo(destination)) => Step::Place(destination.clone()),
        (Transfer::Succeeded, Intent::Discard) => Step::Discard(None),
        _ => Step::Wait,
    }
}

#[derive(Debug, Default)]
struct Registry {
    downloads: HashMap<String, Download>,
    /// Transfers waiting for their turn in front of the save dialog.
    waiting: VecDeque<String>,
    /// The transfer whose save dialog is open.
    prompting: Option<String>,
    next_order: u64,
}

impl Registry {
    fn begin(&mut self, download: Download) -> Vec<Effect> {
        let effects = vec![Effect::Started(download.notice.clone())];
        self.waiting.push_back(download.notice.id.clone());
        self.downloads.insert(download.notice.id.clone(), download);
        effects.into_iter().chain(self.next_prompt()).collect()
    }

    /// Hands the dialog to the next live transfer, if it is free.
    fn next_prompt(&mut self) -> Option<Effect> {
        if self.prompting.is_some() {
            return None;
        }
        while let Some(id) = self.waiting.pop_front() {
            if let Some(download) = self.downloads.get(&id) {
                self.prompting = Some(id);
                return Some(Effect::Prompt(download.notice.clone()));
            }
        }
        None
    }

    /// Applies the step a record became ready for and reports the work.
    fn settle(&mut self, id: &str) -> Vec<Effect> {
        match self.downloads.get(id).map(step) {
            Some(Step::Wait) | None => Vec::new(),
            Some(Step::Place(destination)) => {
                let Some(download) = self.downloads.get_mut(id) else {
                    return Vec::new();
                };
                download.intent = Intent::Placing;
                vec![Effect::Place {
                    id: id.to_string(),
                    staging_dir: download.staging_dir.clone(),
                    staged_file: download.staged_file.clone(),
                    destination,
                }]
            }
            Some(Step::Discard(outcome)) => {
                let Some(download) = self.downloads.remove(id) else {
                    return Vec::new();
                };
                let mut effects = vec![Effect::Discard(download.staging_dir)];
                if let Some(outcome) = outcome {
                    effects.push(Effect::Finished {
                        notice: download.notice,
                        outcome,
                    });
                }
                effects
            }
        }
    }

    /// Applies the user's answer for the open dialog.
    fn answer(&mut self, id: &str, destination: Option<PathBuf>) -> Vec<Effect> {
        if self.prompting.as_deref() != Some(id) {
            return Vec::new();
        }
        self.prompting = None;
        let mut effects = Vec::new();
        if let Some(mut download) = self.downloads.remove(id) {
            match destination {
                Some(destination) => {
                    download.intent = Intent::SaveTo(destination);
                    self.downloads.insert(id.to_string(), download);
                    effects.extend(self.settle(id));
                }
                // A retry the user postponed keeps waiting.
                None if matches!(download.intent, Intent::Asking { again: true }) => {
                    download.intent = Intent::Unsaved;
                    self.downloads.insert(id.to_string(), download);
                }
                // Declining a finished transfer drops it right away.
                None if download.transfer == Transfer::Succeeded => {
                    effects.push(Effect::Discard(std::mem::take(&mut download.staging_dir)));
                    effects.push(Effect::Finished {
                        notice: download.notice,
                        outcome: Outcome::Cancelled,
                    });
                }
                // Declining a streaming transfer is reported now and dropped
                // when it ends.
                None => {
                    download.intent = Intent::Discard;
                    effects.push(Effect::Finished {
                        notice: download.notice.clone(),
                        outcome: Outcome::Cancelled,
                    });
                    self.downloads.insert(id.to_string(), download);
                }
            }
        }
        effects.extend(self.next_prompt());
        effects
    }

    /// Reports whether the dialog still belongs to a transfer waiting for it.
    fn is_asking(&self, id: &str) -> bool {
        self.downloads
            .get(id)
            .is_some_and(|download| matches!(download.intent, Intent::Asking { .. }))
    }

    /// Releases the dialog slot of a transfer that no longer waits for it.
    fn abandon_prompt(&mut self, id: &str) -> Vec<Effect> {
        if self.prompting.as_deref() == Some(id) {
            self.prompting = None;
        }
        self.next_prompt().into_iter().collect()
    }

    fn transfer_finished(&mut self, url: &str, path: Option<&Path>, success: bool) -> Vec<Effect> {
        let in_flight = |download: &Download| download.transfer == Transfer::InFlight;
        let id = path
            .and_then(|path| {
                self.downloads
                    .iter()
                    .find(|(_, download)| in_flight(download) && download.staged_file == path)
                    .map(|(id, _)| id.clone())
            })
            .or_else(|| {
                self.downloads
                    .iter()
                    .filter(|(_, download)| in_flight(download) && download.url == url)
                    .min_by_key(|(_, download)| download.order)
                    .map(|(id, _)| id.clone())
            });
        let Some(id) = id else {
            return Vec::new();
        };
        let Some(download) = self.downloads.get_mut(&id) else {
            return Vec::new();
        };
        download.transfer = if success {
            Transfer::Succeeded
        } else {
            Transfer::Failed
        };
        self.settle(&id)
    }

    /// Queues a new save dialog for a finished transfer that has no place yet.
    fn ask_again(&mut self, id: &str) -> Result<Vec<Effect>, &'static str> {
        let Some(download) = self.downloads.get_mut(id) else {
            return Err("download_not_retryable");
        };
        // A second request while that retry is already queued changes nothing,
        // so a repeated click neither fails nor opens a second dialog.
        if matches!(download.intent, Intent::Asking { again: true }) {
            return Ok(Vec::new());
        }
        if download.transfer != Transfer::Succeeded || download.intent != Intent::Unsaved {
            return Err("download_not_retryable");
        }
        download.intent = Intent::Asking { again: true };
        self.waiting.push_back(id.to_string());
        Ok(self.next_prompt().into_iter().collect())
    }

    /// Applies the result of a placement. Only a failure keeps the staged file.
    fn placed(&mut self, id: &str, placed: bool) -> Vec<Effect> {
        let placing = self
            .downloads
            .get(id)
            .is_some_and(|download| download.intent == Intent::Placing);
        if !placing {
            return Vec::new();
        }
        if placed {
            return self
                .downloads
                .remove(id)
                .map(|download| {
                    vec![Effect::Finished {
                        notice: download.notice,
                        outcome: Outcome::Saved,
                    }]
                })
                .unwrap_or_default();
        }
        let mut effects = Vec::new();
        if let Some(download) = self.downloads.get_mut(id) {
            download.intent = Intent::Unsaved;
            effects.push(Effect::Finished {
                notice: download.notice.clone(),
                outcome: Outcome::Unsaved,
            });
        }
        effects
    }
}

/// Coordinates embedded downloads for the whole process. The lock only guards
/// the state machine; dialogs, events, and file work happen outside it.
pub(crate) struct BrowserDownloads {
    staging_root: PathBuf,
    registry: Mutex<Registry>,
    /// Folder of the last file the user saved in this session.
    last_directory: Mutex<Option<PathBuf>>,
}

impl BrowserDownloads {
    /// Creates the coordinator and deletes staging left by an earlier process.
    /// Call it before any browser WebView exists, so nothing under the root
    /// belongs to a live transfer.
    pub(crate) fn new(staging_root: PathBuf) -> Self {
        discard_directory(&staging_root);
        Self {
            staging_root,
            registry: Mutex::new(Registry::default()),
            last_directory: Mutex::new(None),
        }
    }

    /// `DownloadEvent::Requested`. Runs on the native UI thread, so it only
    /// creates the staging directory and registers the transfer. Returns
    /// whether the engine may write `destination`.
    pub(crate) fn requested(
        &self,
        app: &AppHandle,
        page: DownloadPage,
        url: &Url,
        destination: &mut PathBuf,
    ) -> bool {
        let file_name = sanitize_file_name(destination.file_name());
        let id = new_download_id();
        let staging_dir = self.staging_root.join(&id);
        // Fail closed: without a staging file the engine would write to its
        // own default location.
        if let Err(error) = std::fs::create_dir_all(&staging_dir) {
            eprintln!(
                "[browser] Download staging directory unavailable: {}",
                error.kind()
            );
            let _ = app.emit(
                EVENT_BLOCKED,
                json!({
                    "sessionId": page.session_id,
                    "tab": page.tab,
                    // URLs may carry one-time tokens or query parameters.
                    "source": download_source(url),
                }),
            );
            return false;
        }
        let staged_file = staging_dir.join(&file_name);
        *destination = staged_file.clone();
        let effects = {
            let mut registry = self.registry.lock();
            registry.next_order += 1;
            let order = registry.next_order;
            registry.begin(Download {
                notice: Notice {
                    id,
                    session_id: page.session_id,
                    tab: page.tab,
                    file_name,
                },
                url: url.to_string(),
                staging_dir,
                staged_file,
                transfer: Transfer::InFlight,
                intent: Intent::Asking { again: false },
                order,
            })
        };
        self.perform(app, effects);
        true
    }

    /// `DownloadEvent::Finished`. Runs on the native UI thread; placement is
    /// moved to a blocking task.
    pub(crate) fn finished(&self, app: &AppHandle, url: &Url, path: Option<&Path>, success: bool) {
        let effects = self
            .registry
            .lock()
            .transfer_finished(url.as_str(), path, success);
        self.perform(app, effects);
    }

    /// Asks again where a finished transfer should be saved.
    pub(crate) fn save_as(&self, app: &AppHandle, id: &str) -> Result<(), String> {
        let effects = self.registry.lock().ask_again(id).map_err(str::to_string)?;
        self.perform(app, effects);
        Ok(())
    }

    fn perform(&self, app: &AppHandle, effects: Vec<Effect>) {
        for effect in effects {
            match effect {
                Effect::Started(notice) => {
                    let _ = app.emit(EVENT_STARTED, notice.payload());
                }
                Effect::Finished { notice, outcome } => {
                    let _ = app.emit(EVENT_FINISHED, finished_payload(&notice, outcome));
                }
                Effect::Prompt(notice) => {
                    // Leave the engine callback before touching the dialog.
                    let app = app.clone();
                    tauri::async_runtime::spawn(async move {
                        app.state::<BrowserDownloads>()
                            .prompt(&app, &notice.id, &notice.file_name);
                    });
                }
                Effect::Place {
                    id,
                    staging_dir,
                    staged_file,
                    destination,
                } => {
                    let app = app.clone();
                    tauri::async_runtime::spawn_blocking(move || {
                        let placed = place_staged_file(&staged_file, &destination).is_ok();
                        if placed {
                            discard_directory(&staging_dir);
                        }
                        let coordinator = app.state::<BrowserDownloads>();
                        let effects = coordinator.registry.lock().placed(&id, placed);
                        coordinator.perform(&app, effects);
                    });
                }
                Effect::Discard(staging_dir) => {
                    tauri::async_runtime::spawn_blocking(move || discard_directory(&staging_dir));
                }
            }
        }
    }

    fn prompt(&self, app: &AppHandle, id: &str, file_name: &str) {
        {
            let mut registry = self.registry.lock();
            if !registry.is_asking(id) {
                let effects = registry.abandon_prompt(id);
                drop(registry);
                self.perform(app, effects);
                return;
            }
        }
        let fallback = app
            .path()
            .download_dir()
            .or_else(|_| app.path().home_dir())
            .ok();
        let directory = dialog_directory(self.last_directory.lock().as_deref(), fallback);
        let mut builder = app.dialog().file().set_file_name(file_name);
        if let Some(directory) = directory {
            builder = builder.set_directory(directory);
        }
        if let Some(window) = app.get_window(MAIN_WINDOW) {
            builder = builder.set_parent(&window);
        }
        let app = app.clone();
        let id = id.to_string();
        builder.save_file(move |picked| {
            // An unusable picker result counts as declining.
            let destination = picked.and_then(|picked| picked.into_path().ok());
            let coordinator = app.state::<BrowserDownloads>();
            let folder = destination
                .as_deref()
                .and_then(Path::parent)
                .filter(|folder| !folder.as_os_str().is_empty());
            if let Some(folder) = folder {
                *coordinator.last_directory.lock() = Some(folder.to_path_buf());
            }
            let effects = coordinator.registry.lock().answer(&id, destination);
            coordinator.perform(&app, effects);
        });
    }
}

/// Folder the save dialog opens in: where the user last saved, else the
/// system Downloads folder. Without one the engine picks its own default,
/// which on Linux is the process working directory.
fn dialog_directory(last: Option<&Path>, fallback: Option<PathBuf>) -> Option<PathBuf> {
    last.filter(|last| last.is_dir())
        .map(Path::to_path_buf)
        .or(fallback)
}

fn finished_payload(notice: &Notice, outcome: Outcome) -> Value {
    let mut payload = notice.payload();
    if let Value::Object(fields) = &mut payload {
        fields.insert("outcome".into(), json!(outcome.as_str()));
    }
    payload
}

/// Narrows an engine-suggested name to one harmless path component. Pages
/// control it through `download=` attributes and `Content-Disposition`.
fn sanitize_file_name(raw: Option<&OsStr>) -> String {
    let raw = raw.map_or_else(String::new, |name| name.to_string_lossy().into_owned());
    let name: String = raw
        .chars()
        .map(|character| {
            if character.is_control()
                || matches!(
                    character,
                    '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|'
                )
            {
                '_'
            } else {
                character
            }
        })
        .collect();
    let trimmed =
        name.trim_matches(|character: char| character.is_whitespace() || character == '.');
    let name = if trimmed.is_empty() {
        FALLBACK_FILE_NAME
    } else {
        trimmed
    };
    let (stem, extension) = split_extension(name);
    let stem = if is_reserved_device_name(stem) {
        format!("_{stem}")
    } else {
        stem.to_string()
    };
    let budget = MAX_FILE_NAME_BYTES.saturating_sub(extension.len());
    let stem = if stem.len() > budget {
        // Only a cut is cleaned up: it can end on a space or dot, which
        // Windows drops from a name. An untouched stem keeps its own dots.
        truncate_utf8(&stem, budget)
            .trim_end_matches(|character: char| character.is_whitespace() || character == '.')
    } else {
        stem.as_str()
    };
    let stem = if stem.is_empty() {
        FALLBACK_FILE_NAME
    } else {
        stem
    };
    format!("{stem}{extension}")
}

fn split_extension(name: &str) -> (&str, &str) {
    name.rfind('.')
        .filter(|&dot| dot > 0 && name.len() - dot <= MAX_EXTENSION_BYTES)
        .map_or((name, ""), |dot| (&name[..dot], &name[dot..]))
}

/// Windows refuses these stems even with an extension and in any case.
fn is_reserved_device_name(stem: &str) -> bool {
    let upper = stem.to_ascii_uppercase();
    let numbered = upper
        .strip_prefix("COM")
        .or_else(|| upper.strip_prefix("LPT"));
    matches!(upper.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || numbered.is_some_and(|suffix| {
            matches!(
                suffix,
                "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
            )
        })
}

fn new_download_id() -> String {
    format!("{:032x}", rand::random::<u128>())
}

/// Exposes only the origin of a URL that may carry one-time tokens.
fn download_source(url: &Url) -> String {
    match (url.scheme(), url.host_str()) {
        (scheme, Some(host)) => format!("{scheme}://{host}"),
        (scheme, None) => format!("{scheme}:"),
    }
}

/// Moves a finished transfer to the user's path.
fn place_staged_file(staged_file: &Path, destination: &Path) -> io::Result<()> {
    place_staged_file_with(staged_file, destination, |from, to| {
        std::fs::rename(from, to)
    })
}

/// `rename` is injected so tests can stand in for a destination on another
/// volume, which a single-volume test machine cannot provide.
fn place_staged_file_with<R>(staged_file: &Path, destination: &Path, rename: R) -> io::Result<()>
where
    R: Fn(&Path, &Path) -> io::Result<()>,
{
    match rename(staged_file, destination) {
        Ok(()) => return Ok(()),
        // Another volume cannot be renamed across; the copy below handles it.
        Err(error) if error.kind() != io::ErrorKind::CrossesDevices => return Err(error),
        Err(_) => {}
    }
    // The destination never holds a partial file: the copy streams into a
    // temporary sibling, and the staged copy is removed only after the swap.
    let parent = destination
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let temporary = parent.join(format!(".pinvou-{}.part", new_download_id()));
    let placed = (|| -> io::Result<()> {
        let mut source = std::fs::File::open(staged_file)?;
        let mut target = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        let copied = io::copy(&mut source, &mut target).and_then(|_| target.sync_all());
        // Windows refuses to rename a file that is still open.
        drop(target);
        copied?;
        rename(&temporary, destination)
    })();
    match placed {
        Ok(()) => std::fs::remove_file(staged_file),
        Err(error) => {
            let _ = std::fs::remove_file(&temporary);
            Err(error)
        }
    }
}

/// Best-effort removal. A leftover is deleted by the next process start.
fn discard_directory(path: &Path) {
    if let Err(error) = std::fs::remove_dir_all(path) {
        if error.kind() != io::ErrorKind::NotFound {
            eprintln!(
                "[browser] Failed to remove download staging directory: {}",
                error.kind()
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const URL_A: &str = "https://site.invalid/a";

    fn record(id: &str, url: &str) -> Download {
        let staging_dir = PathBuf::from("/staging").join(id);
        Download {
            notice: Notice {
                id: id.to_string(),
                session_id: "session".to_string(),
                tab: "tab".to_string(),
                file_name: format!("{id}.bin"),
            },
            url: url.to_string(),
            staged_file: staging_dir.join(format!("{id}.bin")),
            staging_dir,
            transfer: Transfer::InFlight,
            intent: Intent::Asking { again: false },
            order: 0,
        }
    }

    fn registry_with(ids: &[&str]) -> Registry {
        let mut registry = Registry::default();
        for (position, id) in ids.iter().enumerate() {
            let mut download = record(id, &url(id));
            download.order = position as u64 + 1;
            registry.begin(download);
        }
        registry
    }

    fn url(id: &str) -> String {
        format!("https://site.invalid/{id}")
    }

    fn staged_path(id: &str) -> PathBuf {
        record(id, "").staged_file
    }

    fn chosen(id: &str) -> PathBuf {
        PathBuf::from("/chosen").join(format!("{id}.bin"))
    }

    fn started(id: &str) -> Effect {
        Effect::Started(record(id, "").notice)
    }

    fn prompt(id: &str) -> Effect {
        Effect::Prompt(record(id, "").notice)
    }

    fn finished(id: &str, outcome: Outcome) -> Effect {
        Effect::Finished {
            notice: record(id, "").notice,
            outcome,
        }
    }

    fn discard(id: &str) -> Effect {
        Effect::Discard(PathBuf::from("/staging").join(id))
    }

    fn place(id: &str, destination: PathBuf) -> Effect {
        let download = record(id, "");
        Effect::Place {
            id: id.to_string(),
            staging_dir: download.staging_dir,
            staged_file: download.staged_file,
            destination,
        }
    }

    fn finish_ok(registry: &mut Registry, id: &str) -> Vec<Effect> {
        registry.transfer_finished(&url(id), Some(&staged_path(id)), true)
    }

    #[test]
    fn suggested_name_becomes_one_harmless_path_component() {
        assert_eq!(sanitize_file_name(None), "download");
        assert_eq!(
            sanitize_file_name(Some(OsStr::new("../../etc/passwd"))),
            "_.._etc_passwd"
        );
        assert_eq!(
            sanitize_file_name(Some(OsStr::new(" report.pdf "))),
            "report.pdf"
        );
        // Dots inside a name are the page's choice; only a cut cleans them up.
        assert_eq!(sanitize_file_name(Some(OsStr::new("a..txt"))), "a..txt");
        assert_eq!(
            sanitize_file_name(Some(OsStr::new("a\u{0}\n:b?.txt"))),
            "a___b_.txt"
        );
        assert_eq!(sanitize_file_name(Some(OsStr::new("CON"))), "_CON");
        assert_eq!(
            sanitize_file_name(Some(OsStr::new("lpt9.txt"))),
            "_lpt9.txt"
        );
        assert_eq!(
            sanitize_file_name(Some(OsStr::new("console.txt"))),
            "console.txt"
        );
        assert_eq!(sanitize_file_name(Some(OsStr::new("..."))), "download");
        assert_eq!(
            sanitize_file_name(Some(OsStr::new("季度报告.pdf"))),
            "季度报告.pdf"
        );
    }

    #[test]
    fn long_names_are_cut_on_a_character_boundary_and_keep_a_short_extension() {
        let long = format!("{}.tar.gz", "x".repeat(300));
        assert_eq!(
            sanitize_file_name(Some(OsStr::new(&long))),
            format!("{}.gz", "x".repeat(MAX_FILE_NAME_BYTES - ".gz".len()))
        );
        // A long suffix is part of the stem, so it is cut like the rest.
        let long_suffix = format!("{}.{}", "a".repeat(130), "b".repeat(30));
        assert_eq!(
            sanitize_file_name(Some(OsStr::new(&long_suffix))),
            "a".repeat(MAX_FILE_NAME_BYTES)
        );
        let cjk = format!("{}.pdf", "季".repeat(100));
        assert_eq!(
            sanitize_file_name(Some(OsStr::new(&cjk))),
            format!("{}.pdf", "季".repeat(38))
        );
        // A cut that ends on dots drops them; the short extension survives.
        let dotted = format!("a{}.x", ".".repeat(200));
        assert_eq!(sanitize_file_name(Some(OsStr::new(&dotted))), "a.x");
    }

    #[test]
    fn simultaneous_downloads_share_one_dialog_in_order() {
        let mut registry = Registry::default();
        assert_eq!(
            registry.begin(record("a", URL_A)),
            vec![started("a"), prompt("a")]
        );
        assert_eq!(registry.begin(record("b", &url("b"))), vec![started("b")]);
        // Both transfers stream while only the first owns the dialog.
        assert_eq!(registry.prompting.as_deref(), Some("a"));
        assert_eq!(
            registry.waiting.iter().cloned().collect::<Vec<_>>(),
            vec!["b"]
        );
    }

    #[test]
    fn a_place_chosen_while_streaming_is_used_when_the_transfer_ends() {
        let mut registry = registry_with(&["a", "b"]);
        assert_eq!(registry.answer("a", Some(chosen("a"))), vec![prompt("b")]);
        assert_eq!(registry.answer("b", Some(chosen("b"))), vec![]);
        // The engine reports the path it wrote; the file is placed right then.
        assert_eq!(finish_ok(&mut registry, "a"), vec![place("a", chosen("a"))]);
        assert_eq!(
            registry.placed("a", true),
            vec![finished("a", Outcome::Saved)]
        );
        assert_eq!(finish_ok(&mut registry, "b"), vec![place("b", chosen("b"))]);
        assert_eq!(
            registry.placed("b", true),
            vec![finished("b", Outcome::Saved)]
        );
        assert!(registry.downloads.is_empty());
        assert_eq!(registry.prompting, None);
    }

    #[test]
    fn a_transfer_that_finishes_first_waits_for_the_user() {
        let mut registry = registry_with(&["a"]);
        assert_eq!(finish_ok(&mut registry, "a"), vec![]);
        assert_eq!(
            registry.answer("a", Some(chosen("a"))),
            vec![place("a", chosen("a"))]
        );
    }

    #[test]
    fn declining_reports_at_once_and_drops_the_transfer_when_it_ends() {
        let mut registry = registry_with(&["a", "b"]);
        assert_eq!(
            registry.answer("a", None),
            vec![finished("a", Outcome::Cancelled), prompt("b")]
        );
        // Nothing further is reported for a transfer the user already declined.
        assert_eq!(finish_ok(&mut registry, "a"), vec![discard("a")]);
        // A transfer that already finished is dropped as soon as it is declined.
        finish_ok(&mut registry, "b");
        assert_eq!(
            registry.answer("b", None),
            vec![discard("b"), finished("b", Outcome::Cancelled)]
        );
        assert!(registry.downloads.is_empty());
        assert_eq!(registry.prompting, None);
    }

    #[test]
    fn a_failed_transfer_leaves_the_dialog_and_the_queue() {
        let mut registry = registry_with(&["a", "b", "c"]);
        // Fails while its dialog is open. The late answer only frees the dialog.
        assert_eq!(
            registry.transfer_finished(URL_A, Some(&staged_path("a")), false),
            vec![discard("a"), finished("a", Outcome::Failed)]
        );
        assert_eq!(registry.answer("a", Some(chosen("a"))), vec![prompt("b")]);
        // Fails while still queued: it is never prompted.
        assert_eq!(
            registry.transfer_finished(&url("c"), Some(&staged_path("c")), false),
            vec![discard("c"), finished("c", Outcome::Failed)]
        );
        registry.answer("b", Some(chosen("b")));
        assert_eq!(finish_ok(&mut registry, "b"), vec![place("b", chosen("b"))]);
        assert_eq!(
            registry.placed("b", true),
            vec![finished("b", Outcome::Saved)]
        );
        assert_eq!(registry.prompting, None);
        assert!(registry.downloads.is_empty());
    }

    #[test]
    fn a_transfer_failing_after_the_choice_reports_failure_and_never_places() {
        let mut registry = registry_with(&["a"]);
        assert_eq!(registry.answer("a", Some(chosen("a"))), vec![]);
        assert_eq!(
            registry.transfer_finished(URL_A, Some(&staged_path("a")), false),
            vec![discard("a"), finished("a", Outcome::Failed)]
        );
    }

    #[test]
    fn finished_matches_the_written_path_then_the_oldest_matching_url() {
        let same = "https://site.invalid/file?token=one-time";
        let mut registry = Registry::default();
        for (position, id) in ["a", "b", "c"].iter().enumerate() {
            let mut download = record(id, same);
            download.order = position as u64 + 1;
            registry.begin(download);
        }
        // Engines that report no path fall back to the oldest transfer with that URL.
        assert_eq!(
            registry.transfer_finished(same, None, false),
            vec![discard("a"), finished("a", Outcome::Failed)]
        );
        // A reported path wins over the URL.
        assert_eq!(
            registry.transfer_finished(same, Some(&staged_path("c")), false),
            vec![discard("c"), finished("c", Outcome::Failed)]
        );
        assert_eq!(
            registry.transfer_finished(same, None, false),
            vec![discard("b"), finished("b", Outcome::Failed)]
        );
        // Unknown and repeated reports change nothing.
        assert_eq!(registry.transfer_finished(same, None, true), vec![]);
        assert_eq!(
            registry.transfer_finished(&url("other"), Some(&staged_path("a")), true),
            vec![]
        );
        assert!(registry.downloads.is_empty());
    }

    #[test]
    fn a_second_transfer_of_the_same_url_is_not_confused_with_a_finished_one() {
        let mut registry = registry_with(&["a"]);
        finish_ok(&mut registry, "a");
        // The first transfer is complete and still waits for the user when
        // the page starts the same download again.
        let mut again = record("b", URL_A);
        again.order = 2;
        registry.begin(again);
        // An engine that reports no path must match the streaming transfer.
        assert_eq!(
            registry.transfer_finished(URL_A, None, false),
            vec![discard("b"), finished("b", Outcome::Failed)]
        );
        // A late or repeated report never reopens the finished transfer.
        assert_eq!(
            registry.transfer_finished(URL_A, Some(&staged_path("a")), false),
            vec![]
        );
        assert_eq!(
            registry.answer("a", Some(chosen("a"))),
            vec![place("a", chosen("a"))]
        );
    }

    #[test]
    fn a_failed_placement_keeps_the_transfer_retryable() {
        let mut registry = registry_with(&["a"]);
        finish_ok(&mut registry, "a");
        registry.answer("a", Some(chosen("a")));
        // The move failed: a vanished folder, a full disk, or a locked file.
        assert_eq!(
            registry.placed("a", false),
            vec![finished("a", Outcome::Unsaved)]
        );
        assert_eq!(registry.ask_again("a"), Ok(vec![prompt("a")]));
        assert_eq!(
            registry.answer("a", Some(PathBuf::from("/elsewhere/a.bin"))),
            vec![place("a", PathBuf::from("/elsewhere/a.bin"))]
        );
        assert_eq!(
            registry.placed("a", true),
            vec![finished("a", Outcome::Saved)]
        );
        assert!(registry.downloads.is_empty());
    }

    #[test]
    fn postponing_a_retry_keeps_the_file_and_frees_the_dialog() {
        let mut registry = registry_with(&["a", "b"]);
        finish_ok(&mut registry, "a");
        registry.answer("a", Some(chosen("a")));
        registry.placed("a", false);
        // The dialog is busy, so the retry waits for its turn.
        assert_eq!(registry.ask_again("a"), Ok(vec![]));
        assert_eq!(
            registry.answer("b", None),
            vec![finished("b", Outcome::Cancelled), prompt("a")]
        );
        // Postponed: no event, and the staged file stays for another attempt.
        assert_eq!(registry.answer("a", None), vec![]);
        assert_eq!(registry.prompting, None);
        assert_eq!(registry.downloads["a"].intent, Intent::Unsaved);
        assert_eq!(registry.ask_again("a"), Ok(vec![prompt("a")]));
    }

    #[test]
    fn a_repeated_retry_request_never_queues_a_second_dialog() {
        let mut registry = registry_with(&["a"]);
        finish_ok(&mut registry, "a");
        registry.answer("a", Some(chosen("a")));
        registry.placed("a", false);
        assert_eq!(registry.ask_again("a"), Ok(vec![prompt("a")]));
        // A second click while that dialog is open neither fails nor queues.
        assert_eq!(registry.ask_again("a"), Ok(vec![]));
        assert!(registry.waiting.is_empty());
        assert_eq!(
            registry.answer("a", Some(chosen("a"))),
            vec![place("a", chosen("a"))]
        );
    }

    #[test]
    fn only_an_unsaved_finished_transfer_is_asked_again() {
        let mut registry = registry_with(&["a"]);
        assert_eq!(registry.ask_again("a"), Err("download_not_retryable"));
        assert_eq!(registry.ask_again("missing"), Err("download_not_retryable"));
        registry.transfer_finished(URL_A, Some(&staged_path("a")), false);
        assert_eq!(registry.ask_again("a"), Err("download_not_retryable"));
        assert!(registry.waiting.is_empty());

        // Declining is final as well.
        let mut registry = registry_with(&["a"]);
        finish_ok(&mut registry, "a");
        registry.answer("a", None);
        assert_eq!(registry.ask_again("a"), Err("download_not_retryable"));
        assert_eq!(registry.prompting, None);
    }

    #[test]
    fn a_stale_answer_never_takes_the_dialog_from_its_owner() {
        let mut registry = registry_with(&["a", "b"]);
        // The dialog for b is not open yet, so an answer for it changes nothing.
        assert_eq!(registry.answer("b", Some(chosen("b"))), vec![]);
        assert_eq!(registry.prompting.as_deref(), Some("a"));
        assert_eq!(
            registry.downloads["b"].intent,
            Intent::Asking { again: false }
        );
        // A transfer that stopped waiting hands the dialog to the next one.
        assert_eq!(registry.abandon_prompt("a"), vec![prompt("b")]);
        assert!(registry.is_asking("b"));
        assert!(!registry.is_asking("missing"));
    }

    #[test]
    fn a_late_placement_result_only_applies_while_placing() {
        let mut registry = registry_with(&["a"]);
        finish_ok(&mut registry, "a");
        registry.answer("a", Some(chosen("a")));
        assert_eq!(registry.placed("missing", true), vec![]);
        assert_eq!(
            registry.placed("a", true),
            vec![finished("a", Outcome::Saved)]
        );
        assert_eq!(registry.placed("a", false), vec![]);
    }

    #[test]
    fn events_carry_names_and_outcomes_but_never_urls_or_paths() {
        let notice = Notice {
            id: "id1".to_string(),
            session_id: "s".to_string(),
            tab: "t".to_string(),
            file_name: "报告.pdf".to_string(),
        };
        assert_eq!(
            finished_payload(&notice, Outcome::Saved),
            json!({
                "sessionId": "s",
                "tab": "t",
                "downloadId": "id1",
                "fileName": "报告.pdf",
                "outcome": "saved",
            })
        );
        assert_eq!(notice.payload()["destination"], Value::Null);
        assert_eq!(notice.payload()["url"], Value::Null);
        assert_eq!(
            download_source(
                &Url::parse("https://user:secret@site.invalid/path?token=one-time").unwrap()
            ),
            "https://site.invalid"
        );
        assert_eq!(
            download_source(&Url::parse("blob:https://site.invalid/uuid").unwrap()),
            "blob:"
        );
    }

    fn staged(root: &Path, bytes: &[u8]) -> PathBuf {
        let staged_file = root.join("staging").join("file.bin");
        std::fs::create_dir_all(staged_file.parent().unwrap()).unwrap();
        std::fs::write(&staged_file, bytes).unwrap();
        staged_file
    }

    fn entries(folder: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(folder)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    /// Stands in for a destination on another volume, where a rename cannot work.
    fn rename_across_volumes(staged_file: &Path) -> impl Fn(&Path, &Path) -> io::Result<()> + '_ {
        move |from, to| {
            if from == staged_file {
                Err(io::Error::new(
                    io::ErrorKind::CrossesDevices,
                    "simulated EXDEV",
                ))
            } else {
                std::fs::rename(from, to)
            }
        }
    }

    #[test]
    fn placement_renames_within_a_volume_and_replaces_an_existing_file() {
        let temp = tempfile::tempdir().unwrap();
        let staged_file = staged(temp.path(), b"new contents");
        let destination = temp.path().join("file.bin");
        std::fs::write(&destination, b"old contents").unwrap();
        place_staged_file(&staged_file, &destination).unwrap();
        assert_eq!(std::fs::read(&destination).unwrap(), b"new contents");
        assert!(!staged_file.exists());
    }

    #[test]
    fn placement_streams_across_volumes_with_whole_file_copy() {
        let temp = tempfile::tempdir().unwrap();
        // Larger than every capped reader in the app: a placement never truncates.
        let payload: Vec<u8> = (0..17 * 1024 * 1024u32)
            .map(|index| (index % 251) as u8)
            .collect();
        let staged_file = staged(temp.path(), &payload);
        let folder = temp.path().join("other-volume");
        std::fs::create_dir(&folder).unwrap();
        let destination = folder.join("file.bin");
        std::fs::write(&destination, b"old").unwrap();
        place_staged_file_with(
            &staged_file,
            &destination,
            rename_across_volumes(&staged_file),
        )
        .unwrap();
        assert_eq!(std::fs::read(&destination).unwrap(), payload);
        assert!(!staged_file.exists());
        // No temporary sibling is left behind.
        assert_eq!(entries(&folder), vec!["file.bin"]);
    }

    #[test]
    fn a_failed_rename_is_reported_and_keeps_the_staged_copy() {
        let temp = tempfile::tempdir().unwrap();
        let staged_file = staged(temp.path(), b"contents");
        let destination = temp.path().join("file.bin");
        // Every move is refused, the way a destination no process may touch is.
        let rename =
            |_: &Path, _: &Path| Err(io::Error::new(io::ErrorKind::PermissionDenied, "locked"));
        assert!(place_staged_file_with(&staged_file, &destination, rename).is_err());
        assert_eq!(std::fs::read(&staged_file).unwrap(), b"contents");
        assert!(!destination.exists());
    }

    #[test]
    fn a_failed_swap_leaves_the_staged_copy_and_no_partial_file() {
        let temp = tempfile::tempdir().unwrap();
        let staged_file = staged(temp.path(), b"contents");
        let folder = temp.path().join("other-volume");
        std::fs::create_dir(&folder).unwrap();
        let destination = folder.join("file.bin");
        // The swap fails the way a locked destination or a full disk makes it.
        let rename = |_: &Path, _: &Path| Err(io::Error::new(io::ErrorKind::Other, "locked"));
        assert!(place_staged_file_with(&staged_file, &destination, rename).is_err());
        assert_eq!(std::fs::read(&staged_file).unwrap(), b"contents");
        assert!(entries(&folder).is_empty());
    }

    #[test]
    fn a_destination_that_cannot_hold_a_file_never_loses_the_staged_copy() {
        let temp = tempfile::tempdir().unwrap();
        let staged_file = staged(temp.path(), b"contents");
        // The "folder" is a regular file, so no temporary sibling can exist.
        let blocker = temp.path().join("not-a-folder");
        std::fs::write(&blocker, b"").unwrap();
        let destination = blocker.join("file.bin");
        assert!(
            place_staged_file_with(
                &staged_file,
                &destination,
                rename_across_volumes(&staged_file)
            )
            .is_err()
        );
        assert_eq!(std::fs::read(&staged_file).unwrap(), b"contents");
    }

    #[test]
    fn a_new_coordinator_sweeps_staging_from_the_previous_process() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("staging");
        let leftover = root.join("0123456789abcdef").join("partial.bin");
        std::fs::create_dir_all(leftover.parent().unwrap()).unwrap();
        std::fs::write(&leftover, b"interrupted transfer").unwrap();
        let _coordinator = BrowserDownloads::new(root.clone());
        assert!(!root.exists());
        // A missing root is not an error.
        let _coordinator = BrowserDownloads::new(temp.path().join("missing"));
    }

    #[test]
    fn the_dialog_opens_where_the_user_last_saved_while_that_folder_exists() {
        let temp = tempfile::tempdir().unwrap();
        let last = temp.path().join("last");
        let downloads = temp.path().join("downloads");
        // First save of the session, and a folder that has disappeared since.
        assert_eq!(
            dialog_directory(None, Some(downloads.clone())),
            Some(downloads.clone())
        );
        assert_eq!(
            dialog_directory(Some(&last), Some(downloads.clone())),
            Some(downloads.clone())
        );
        assert_eq!(dialog_directory(Some(&last), None), None);
        std::fs::create_dir(&last).unwrap();
        assert_eq!(dialog_directory(Some(&last), Some(downloads)), Some(last));
    }
}
