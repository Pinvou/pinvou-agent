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
//!
//! The engine reports completions either with the written path or, on engines
//! without one, only by URL; a URL shared by several streaming transfers is
//! attributed only once all of them have reported, failing closed when the
//! reports disagree.

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
/// The app refused the download: no staging file could be created, or too
/// many downloads are already live (`reason` says which).
const EVENT_BLOCKED: &str = "browser:download-blocked";
const MAIN_WINDOW: &str = "main";
const FALLBACK_FILE_NAME: &str = "download";
/// Longest suggested name kept for staging and for the dialog. It bounds one
/// path component only; the transfer size is never limited.
const MAX_FILE_NAME_BYTES: usize = 120;
/// Live transfers tracked at once (streaming, asking, unsaved, or placing).
/// A page that asks for more is denied with the `blocked` notice, so one
/// page cannot grow staging or the serialized dialog queue without bound.
const MAX_LIVE_DOWNLOADS: usize = 16;
/// Refusal credits live until the engine's cancellation report for the
/// refused download arrives. The engines report refused downloads (verified
/// per WebKitGTK, WKDownload, and WebView2), so the map drains in practice;
/// the cap only bounds a pathological page that gets thousands of distinct
/// URLs refused. Past the cap a refusal's report votes as ordinary noise,
/// which the outcome mix already tolerates.
const MAX_REFUSAL_CREDITS: usize = 1024;
/// Longer "extensions" are treated as part of the name, so truncating a long
/// name cannot spend its whole budget on a suffix.
const MAX_EXTENSION_BYTES: usize = 24;

/// The page that requested a download. Identifies the UI surface that shows
/// the notice.
#[derive(Clone, Debug, PartialEq, Eq)]
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

/// The live cap is full, so the download was refused.
#[derive(Debug, PartialEq, Eq)]
struct CapacityFull;

#[derive(Debug, Default)]
struct Registry {
    downloads: HashMap<String, Download>,
    /// Transfers waiting for their turn in front of the save dialog.
    waiting: VecDeque<String>,
    /// The transfer whose save dialog is open.
    prompting: Option<String>,
    /// Completion reports that cannot yet be attributed: engines without a
    /// written path (macOS always, Windows on failure) can only match by URL,
    /// and while several in-flight transfers share that URL no single report
    /// names its owner. One batch per URL counts its members down; see
    /// `transfer_finished`.
    pending_finishes: HashMap<String, UrlBatch>,
    /// Pathless failure reports still expected from downloads the app refused
    /// before admission (the live cap, unusable staging, or no shown
    /// workspace on Linux): the engine cancels them and reports a pathless
    /// failure that belongs to no member. One credit per refusal is spent on
    /// the URL's first failure report, so the noise can neither open nor vote
    /// in a batch of live same-URL transfers.
    refusal_credits: HashMap<String, usize>,
}

/// The transfers of one URL whose completions cannot be told apart. Every
/// member reports exactly once — Linux registers one download handler for the
/// whole process and the other engines hand each event to its own webview's
/// handler — so the batch resolves when every member has voted.
#[derive(Debug)]
struct UrlBatch {
    /// Members that have not reported yet. A transfer that resolves through
    /// its written path also leaves this count, carrying its vote into
    /// `outcomes`.
    remaining: usize,
    /// One entry per reported member.
    outcomes: Vec<bool>,
}

impl Registry {
    /// A new download is denied beyond the live cap, so one page cannot grow
    /// staging or the dialog queue without bound.
    fn has_capacity(&self) -> bool {
        self.downloads.len() < MAX_LIVE_DOWNLOADS
    }

    /// Admits a download or refuses it at the live cap, so one page cannot
    /// grow staging or the dialog queue without bound.
    fn begin(&mut self, download: Download) -> Result<Vec<Effect>, CapacityFull> {
        if !self.has_capacity() {
            return Err(CapacityFull);
        }
        // The transfer will report exactly once; an open batch for its URL
        // counts it in as an unreported member.
        if let Some(batch) = self.pending_finishes.get_mut(&download.url) {
            batch.remaining += 1;
        }
        let effects = vec![Effect::Started(download.notice.clone())];
        self.waiting.push_back(download.notice.id.clone());
        self.downloads.insert(download.notice.id.clone(), download);
        Ok(effects.into_iter().chain(self.next_prompt()).collect())
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
        // A written path names exactly one transfer; engines that supply one
        // are matched by it.
        let matched = path.and_then(|path| {
            self.downloads
                .iter()
                .find(|(_, download)| in_flight(download) && download.staged_file == path)
                .map(|(id, _)| id.clone())
        });
        let Some(id) = matched else {
            if path.is_some() {
                return Vec::new();
            }
            return self.vote_pathless_finish(url, success);
        };
        if let Some(download) = self.downloads.get_mut(&id) {
            download.transfer = if success {
                Transfer::Succeeded
            } else {
                Transfer::Failed
            };
        }
        let mut effects = self.settle(&id);
        // A member that resolves through its written path still votes in an
        // open batch for its URL; without the vote the batch would wait for a
        // report that already happened and hold the URL's other members — and
        // a stale batch would poison the URL's next generation.
        if let Some(batch) = self.pending_finishes.get_mut(url) {
            batch.remaining = batch.remaining.saturating_sub(1);
            batch.outcomes.push(success);
        }
        effects.extend(self.resolve_finished_batch(url));
        effects
    }

    /// Records that a download of `url` was refused before admission, so the
    /// cancellation report the engine still sends for it is consumed as noise
    /// (see `refusal_credits`).
    fn note_refusal(&mut self, url: &str) {
        if self.refusal_credits.len() >= MAX_REFUSAL_CREDITS
            && !self.refusal_credits.contains_key(url)
        {
            return;
        }
        *self.refusal_credits.entry(url.to_string()).or_insert(0) += 1;
    }

    /// The ids and urls of every in-flight download attributed to `page`,
    /// sorted for deterministic effect order.
    fn in_flight_ids_of_page(&self, page: &DownloadPage) -> Vec<(String, String)> {
        let mut found: Vec<(String, String)> = self
            .downloads
            .iter()
            .filter(|(_, download)| {
                download.transfer == Transfer::InFlight
                    && download.notice.session_id == page.session_id
                    && download.notice.tab == page.tab
            })
            .map(|(id, download)| (id.clone(), download.url.clone()))
            .collect();
        found.sort();
        found
    }

    /// Settles a member whose engine report will never arrive (its per-tab
    /// download delegate died with the webview): the record fails like any
    /// other failed transfer, and its seat in an open batch is released
    /// without a vote, so the batch resolves on its real reports alone.
    /// Without this, the dead member holds its live slot until the app
    /// restarts and keeps its URL's batch from ever resolving — stranding
    /// every later same-URL transfer behind it.
    fn report_lost(&mut self, id: &str, url: &str) -> Vec<Effect> {
        let Some(download) = self.downloads.get_mut(id) else {
            return Vec::new();
        };
        if download.transfer != Transfer::InFlight {
            return Vec::new();
        }
        download.transfer = Transfer::Failed;
        let mut effects = self.settle(id);
        if let Some(batch) = self.pending_finishes.get_mut(url) {
            batch.remaining = batch.remaining.saturating_sub(1);
        }
        effects.extend(self.resolve_finished_batch(url));
        effects
    }

    /// Attributes a pathless completion by URL. The report joins the URL's
    /// open batch, or opens one that counts every in-flight transfer of the
    /// URL down; uniform outcomes attribute cleanly, and disagreeing ones all
    /// fail closed instead of guessing.
    fn vote_pathless_finish(&mut self, url: &str, success: bool) -> Vec<Effect> {
        // Refusal noise first: a refused transfer can only ever report a
        // failure, so a pending credit is spent on this report and it can
        // neither open a batch nor vote. Spending happens before the
        // candidates check, so a report whose members are already gone still
        // consumes the credit instead of leaving it to eat a live member's
        // report in the URL's next generation.
        if !success {
            let spends_credit = self.refusal_credits.get_mut(url).is_some_and(|credit| {
                if *credit > 0 {
                    *credit -= 1;
                    true
                } else {
                    false
                }
            });
            if spends_credit {
                self.refusal_credits.retain(|_, credit| *credit > 0);
                return Vec::new();
            }
        }
        let in_flight = |download: &Download| download.transfer == Transfer::InFlight;
        let candidates = self
            .downloads
            .values()
            .filter(|download| in_flight(download) && download.url == url)
            .count();
        if candidates == 0 {
            return Vec::new();
        }
        let batch = self
            .pending_finishes
            .entry(url.to_string())
            .or_insert(UrlBatch {
                remaining: candidates,
                outcomes: Vec::new(),
            });
        batch.remaining = batch.remaining.saturating_sub(1);
        batch.outcomes.push(success);
        self.resolve_finished_batch(url)
    }

    /// Applies a batch whose members have all reported. With one outcome
    /// across the reports, every assignment is the same; with differing
    /// outcomes no assignment is provable, and no staged copy may ship
    /// unverified, so all of them fail closed.
    fn resolve_finished_batch(&mut self, url: &str) -> Vec<Effect> {
        let complete = self
            .pending_finishes
            .get(url)
            .is_some_and(|batch| batch.remaining == 0);
        if !complete {
            return Vec::new();
        }
        let all_succeeded = self
            .pending_finishes
            .get(url)
            .is_some_and(|batch| batch.outcomes.iter().all(|success| *success));
        self.pending_finishes.remove(url);
        let in_flight = |download: &Download| download.transfer == Transfer::InFlight;
        let mut ids: Vec<String> = self
            .downloads
            .iter()
            .filter(|(_, download)| in_flight(download) && download.url == url)
            .map(|(id, _)| id.clone())
            .collect();
        // Sorted so the reported events do not depend on map order.
        ids.sort();
        let outcome = if all_succeeded {
            Transfer::Succeeded
        } else {
            Transfer::Failed
        };
        let mut effects = Vec::new();
        for id in ids {
            if let Some(download) = self.downloads.get_mut(&id) {
                download.transfer = outcome;
            }
            effects.extend(self.settle(&id));
        }
        effects
    }

    /// Drops a finished-but-unplaced transfer and its staged file. That state
    /// has no other exit, so this is the user's way out of a stranded record.
    /// A record that is gone or not discardable changes nothing.
    fn discard(&mut self, id: &str) -> Vec<Effect> {
        let discardable = self
            .downloads
            .get(id)
            .is_some_and(|download| download.intent == Intent::Unsaved);
        if !discardable {
            return Vec::new();
        }
        match self.downloads.remove(id) {
            Some(download) => vec![
                Effect::Discard(download.staging_dir),
                Effect::Finished {
                    notice: download.notice,
                    outcome: Outcome::Cancelled,
                },
            ],
            None => Vec::new(),
        }
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

    /// The live `unsaved` records as finished-event payloads, so the frontend
    /// can re-report them on mount (see `BrowserDownloads::pending_unsaved`).
    fn unsaved_payloads(&self) -> Vec<Value> {
        let mut ids: Vec<&String> = self
            .downloads
            .iter()
            .filter(|(_, download)| download.intent == Intent::Unsaved)
            .map(|(id, _)| id)
            .collect();
        ids.sort();
        ids.into_iter()
            .filter_map(|id| self.downloads.get(id))
            .map(|download| finished_payload(&download.notice, Outcome::Unsaved))
            .collect()
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
    /// Webview label -> owning page. Engines that attribute downloads per
    /// tab (macOS, Windows) fire each event in the downloading webview's own
    /// handler, so the label names the requesting tab and the page is
    /// resolved from it at event time. Labels are unique per tab and never
    /// reused across sessions; a closed tab's entry is dropped at once, and a
    /// rebuilt tab overwrites its own entry in place.
    pages: Mutex<HashMap<String, DownloadPage>>,
    /// The page whose browser workspace is currently shown, recorded by the
    /// host at every visibility transition. Linux registers one download
    /// handler for the whole process on a never-closed surface (see
    /// `ensure_download_surface`), so a download event cannot name its
    /// requesting tab there; its notice is attributed to the shown workspace
    /// instead. A download that arrives while no workspace is shown is
    /// refused.
    shown_page: Mutex<Option<DownloadPage>>,
    /// Set once when the Linux download surface cannot be created: every
    /// further request is refused with the `unavailable` reason instead of
    /// being admitted through the degraded per-tab handlers.
    unavailable: std::sync::atomic::AtomicBool,
}

impl BrowserDownloads {
    /// Creates the coordinator and deletes staging left by an earlier process.
    /// Call it before any browser WebView exists, so nothing under the root
    /// belongs to a live transfer. A staging root that cannot be made
    /// absolute (the process working directory is gone) puts downloads out
    /// of service instead of sending the engine to a cwd-relative path.
    pub(crate) fn new(staging_root: PathBuf) -> Self {
        discard_directory(&staging_root);
        let staging_root = absolute_staging_root_with(
            staging_root.as_path(),
            std::env::current_dir().ok().as_deref(),
        );
        let usable = staging_root.is_some();
        Self {
            staging_root: staging_root.unwrap_or_default(),
            registry: Mutex::new(Registry::default()),
            last_directory: Mutex::new(None),
            pages: Mutex::new(HashMap::new()),
            shown_page: Mutex::new(None),
            unavailable: std::sync::atomic::AtomicBool::new(!usable),
        }
    }

    /// Records which page owns a webview label. Called by the platform adapter
    /// when a browser webview is built; see the `pages` field.
    pub(crate) fn register_page(&self, label: &str, page: DownloadPage) {
        self.pages.lock().insert(label.to_string(), page);
    }

    /// Forgets a closed tab's page, mirroring the core-binding lifecycle.
    pub(crate) fn unregister_page(&self, label: &str) {
        self.pages.lock().remove(label);
    }

    /// Forgets every page of one closed session; all of its tabs are gone.
    pub(crate) fn forget_session_pages(&self, session_id: &str) {
        self.pages
            .lock()
            .retain(|_, page| page.session_id != session_id);
    }

    /// Forgets every tab's page; called when the whole browser surface has
    /// closed. The download surface itself never registers, so nothing needs
    /// to survive.
    pub(crate) fn forget_all_pages(&self) {
        self.pages.lock().clear();
    }

    pub(crate) fn page_for_label(&self, label: &str) -> Option<DownloadPage> {
        self.pages.lock().get(label).cloned()
    }

    /// The page a download is attributed to: the tab whose webview fired when
    /// the engine identifies it (macOS, Windows), else the shown workspace's
    /// page (Linux routes every download through one always-live surface, so
    /// the webview label there is the surface's own).
    pub(crate) fn page_for_request(&self, label: &str) -> Option<DownloadPage> {
        self.page_for_label(label)
            .or_else(|| self.shown_page.lock().clone())
    }

    /// Records or clears the shown workspace's page; called by the host at
    /// every visibility transition.
    pub(crate) fn set_shown_page(&self, page: Option<DownloadPage>) {
        *self.shown_page.lock() = page;
    }

    /// Puts downloads out of service for the rest of the process: used when
    /// the Linux download surface cannot be created, where a per-tab fallback
    /// would mis-attribute every download and cancel them all once the first
    /// tab closes. Refusals then carry the `unavailable` reason.
    pub(crate) fn mark_unavailable(&self) {
        self.unavailable
            .store(true, std::sync::atomic::Ordering::Relaxed);
    }

    /// Whether downloads were put out of service at construction (staging
    /// root not absolute) or by a failed Linux download surface.
    #[cfg(test)]
    fn is_unavailable(&self) -> bool {
        self.unavailable.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Settles every in-flight download attributed to `page` as one whose
    /// engine report will never arrive. Called when a tab webview closes on
    /// the engines whose download delegate dies with the webview (macOS,
    /// Windows): the completion can no longer be delivered, so without this
    /// the record would hold a live slot until restart and its URL's open
    /// batch would wait forever, poisoning every later same-URL transfer.
    /// Linux is excluded: its completions arrive through the never-closed
    /// download surface and survive tab close.
    pub(crate) fn reports_lost(&self, app: Option<&tauri::AppHandle>, page: &DownloadPage) {
        let mut effects = {
            let mut registry = self.registry.lock();
            let lost = registry.in_flight_ids_of_page(page);
            let mut effects = Vec::new();
            for (id, url) in lost {
                effects.extend(registry.report_lost(&id, &url));
            }
            effects
        };
        if let Some(app) = app {
            self.perform(app, std::mem::take(&mut effects));
        }
    }

    /// The live `unsaved` records as finished-event payloads. The frontend
    /// re-reports them on mount, so a notice whose event landed while the
    /// workspace was not mounted (a view switch or a background session) can
    /// still reach its Save as / Discard actions.
    pub(crate) fn pending_unsaved(&self) -> Vec<Value> {
        self.registry.lock().unsaved_payloads()
    }

    /// Records a download the app refused before admission, so the engine's
    /// cancellation report for it is spent as noise instead of failing a live
    /// same-URL transfer (see `Registry::refusal_credits`).
    pub(crate) fn note_refusal(&self, url: &Url) {
        self.registry.lock().note_refusal(url.as_str());
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
        // Out of service (Linux download surface could not be created): a
        // per-tab fallback would mis-attribute every download and cancel them
        // all once the first tab closes, so this fails closed instead.
        if self.unavailable.load(std::sync::atomic::Ordering::Relaxed) {
            eprintln!("[browser] Download denied: downloads are unavailable");
            emit_blocked(app, &page, url, "unavailable");
            self.note_refusal(url);
            return false;
        }
        if !self.registry.lock().has_capacity() {
            eprintln!("[browser] Download denied: too many live downloads");
            emit_blocked(app, &page, url, "limit");
            self.note_refusal(url);
            return false;
        }
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
            emit_blocked(app, &page, url, "staging");
            self.note_refusal(url);
            return false;
        }
        // The staged bytes are private while they wait: the per-download
        // directory is owner-only, so another local user on a shared host
        // cannot read an in-flight transfer.
        if let Err(error) = crate::platform::os::try_make_private_dir(&staging_dir) {
            eprintln!(
                "[browser] Download staging directory could not be restricted: {}",
                error.kind()
            );
            let _ = std::fs::remove_dir(&staging_dir);
            emit_blocked(app, &page, url, "staging");
            self.note_refusal(url);
            return false;
        }
        let staged_file = staging_dir.join(&file_name);
        *destination = staged_file.clone();
        let effects = {
            let mut registry = self.registry.lock();
            // The page fields are cloned so a refused download can still
            // report through `emit_blocked` below.
            match registry.begin(Download {
                notice: Notice {
                    id,
                    session_id: page.session_id.clone(),
                    tab: page.tab.clone(),
                    file_name,
                },
                url: url.to_string(),
                staging_dir: staging_dir.clone(),
                staged_file,
                transfer: Transfer::InFlight,
                intent: Intent::Asking { again: false },
            }) {
                Ok(effects) => effects,
                // The authoritative cap gate inside `begin`; the pre-check
                // above keeps the common refusal free of staging I/O.
                Err(CapacityFull) => {
                    registry.note_refusal(url.as_str());
                    drop(registry);
                    let _ = std::fs::remove_dir(&staging_dir);
                    eprintln!("[browser] Download denied: too many live downloads");
                    emit_blocked(app, &page, url, "limit");
                    return false;
                }
            }
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

    /// Drops an `unsaved` transfer the user no longer wants, deleting its
    /// staged file. Idempotent: an unknown or not discardable id changes
    /// nothing, so a repeated click neither fails nor reports twice.
    pub(crate) fn discard(&self, app: &AppHandle, id: &str) {
        let effects = self.registry.lock().discard(id);
        self.perform(app, effects);
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
                    // A destination under the staging root would be renamed
                    // and then deleted together with its staging directory
                    // while reporting `saved`, so it fails the placement
                    // instead: the staged file stays and the record reports
                    // `unsaved` for another attempt.
                    let rejected = destination_inside_staging(&destination, &self.staging_root);
                    let app = app.clone();
                    tauri::async_runtime::spawn_blocking(move || {
                        let placed =
                            !rejected && place_staged_file(&staged_file, &destination, &id).is_ok();
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

/// The engine writes the transfer itself and requires an absolute destination;
/// a relative `PINVOU3_HOME` override would otherwise send it to a
/// cwd-relative path that placement can never find again. Without a usable
/// working directory there is no absolute form, and the caller must put
/// downloads out of service rather than degrade.
fn absolute_staging_root_with(root: &Path, current_dir: Option<&Path>) -> Option<PathBuf> {
    if root.is_absolute() {
        return Some(root.to_path_buf());
    }
    current_dir.map(|current| current.join(root))
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
            if is_forbidden_in_file_name(character) {
                '_'
            } else {
                character
            }
        })
        .collect();
    // Windows drops trailing dots and spaces from a name; leading dots are
    // legal everywhere and meaningful on Unix (.bashrc), so dots are cleaned
    // from the trailing end only. Whitespace goes from both ends.
    let trimmed = name.trim_matches(|character: char| character.is_whitespace());
    let trimmed =
        trimmed.trim_end_matches(|character: char| character.is_whitespace() || character == '.');
    let name = if trimmed.is_empty() {
        FALLBACK_FILE_NAME
    } else {
        trimmed
    };
    let (stem, extension) = split_extension(name);
    // Checking before the cut keeps the one-byte "_" prefix inside the
    // budget for long reserved stems; checking again after the cut catches a
    // cut that strips padding around a reserved name ("CON" + spaces + "zzz"
    // cuts to "CON"). A stem that already carries the prefix is left alone.
    let stem = if starts_with_reserved_device_name(stem) {
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
    let stem = if !stem.starts_with('_') && starts_with_reserved_device_name(stem) {
        format!("_{stem}")
    } else {
        stem.to_string()
    };
    format!("{stem}{extension}")
}

fn split_extension(name: &str) -> (&str, &str) {
    name.rfind('.')
        .filter(|&dot| dot > 0 && name.len() - dot <= MAX_EXTENSION_BYTES)
        .map_or((name, ""), |dot| (&name[..dot], &name[dot..]))
}

/// Control characters cannot render, and bidirectional controls (the Arabic
/// letter mark, overrides, isolates, the left-to-right marks), word joiners,
/// Hangul fillers, Mongolian variation selectors, zero-width marks, the soft
/// hyphen, and the byte-order mark render deceptively: a reversed suffix can
/// make an executable look like a document.
fn is_forbidden_in_file_name(character: char) -> bool {
    character.is_control()
        || matches!(
            character,
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|'
        )
        || matches!(
            character as u32,
            0x00AD | 0x115F..=0x1160 | 0x180B..=0x180E | 0x200B..=0x200F
                | 0x202A..=0x202E
                | 0x2060..=0x2069
                | 0xFEFF
                | 0x061C
        )
}

/// Windows refuses these stems even with an extension and in any case. It
/// reads the device name from the segment before the first dot — and ignores
/// trailing spaces there — so `CON.tar.gz` and `CON  .txt` are both devices.
fn starts_with_reserved_device_name(stem: &str) -> bool {
    stem.split('.')
        .next()
        .is_some_and(|segment| is_reserved_device_name(segment.trim_end()))
}

/// Windows refuses these stems even with an extension and in any case.
/// Current Windows documentation reserves `COM0`-`COM9` and `LPT0`-`LPT9`
/// (including the superscript digits), and Windows 11 adds `CONIN$`/`CONOUT$`.
fn is_reserved_device_name(stem: &str) -> bool {
    let upper = stem.to_ascii_uppercase();
    let numbered = upper
        .strip_prefix("COM")
        .or_else(|| upper.strip_prefix("LPT"));
    matches!(
        upper.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
    ) || numbered.is_some_and(|suffix| {
        matches!(
            suffix,
            "0" | "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
        )
    })
}

fn new_download_id() -> String {
    format!("{:032x}", rand::random::<u128>())
}

/// Emits the notice for a download the app refused: no staging file could be
/// created, or too many downloads are already live.
fn emit_blocked(app: &AppHandle, page: &DownloadPage, url: &Url, reason: &str) {
    let _ = app.emit(
        EVENT_BLOCKED,
        json!({
            "sessionId": page.session_id,
            "tab": page.tab,
            // URLs may carry one-time tokens or query parameters.
            "source": download_source(url),
            "reason": reason,
        }),
    );
}

/// Exposes only the origin of a URL that may carry one-time tokens.
fn download_source(url: &Url) -> String {
    let port = match url.port() {
        Some(port) => format!(":{port}"),
        None => String::new(),
    };
    match (url.scheme(), url.host_str()) {
        (scheme, Some(host)) => format!("{scheme}://{host}{port}"),
        (scheme, None) => format!("{scheme}:"),
    }
}

/// A destination under the staging root would be renamed into a directory
/// the placement cleanup then deletes, so such a choice fails the placement.
/// `Path::starts_with` compares whole components, so a sibling directory
/// whose name merely shares a prefix stays allowed.
fn destination_inside_staging(destination: &Path, staging_root: &Path) -> bool {
    destination.starts_with(staging_root)
}

/// Moves a finished transfer to the user's path.
fn place_staged_file(staged_file: &Path, destination: &Path, id: &str) -> io::Result<()> {
    place_staged_file_with(
        staged_file,
        destination,
        &format!(".pinvou-{id}.part"),
        |from, to| std::fs::rename(from, to),
        |source, target| io::copy(source, target),
        sync_staged_bytes,
    )
}

/// Flushes the staged bytes to the disk through a writable handle. Windows's
/// `FlushFileBuffers` refuses a handle that was not opened for writing, so a
/// plain read-only `File::open` would fail every placement there before the
/// rename is even attempted; Linux and macOS flush a read-only handle fine,
/// which is why a read-only open went unnoticed in local testing.
fn sync_staged_bytes(staged: &Path) -> io::Result<()> {
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(staged)
        .and_then(|file| file.sync_all())
}

/// `rename`, `copy`, and `sync` are injected so tests can stand in for a
/// destination on another volume, which a single-volume test machine cannot
/// provide, for a transfer that dies mid-copy, and for a sync whose order and
/// count are pinned.
fn place_staged_file_with<C, R, S>(
    staged_file: &Path,
    destination: &Path,
    temporary_name: &str,
    rename: R,
    copy: C,
    sync: S,
) -> io::Result<()>
where
    C: Fn(&mut std::fs::File, &mut std::fs::File) -> io::Result<u64>,
    R: Fn(&Path, &Path) -> io::Result<()>,
    S: Fn(&Path) -> io::Result<()>,
{
    // Power-loss durability on both paths: the staged bytes reach the disk
    // before the destination name appears — before the rename below and
    // before the copy's swap that follows it.
    sync(staged_file)?;
    match rename(staged_file, destination) {
        Ok(()) => return Ok(()),
        // Another volume cannot be renamed across; the copy below handles it.
        Err(error) if error.kind() != io::ErrorKind::CrossesDevices => return Err(error),
        Err(_) => {}
    }
    // The destination never holds a partial file: the copy streams into a
    // temporary sibling named after the download, and the staged copy is
    // removed only after the swap. A process killed mid-copy leaves exactly
    // one attributable `.pinvou-<id>.part` behind instead of a mystery file.
    let parent = destination
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let temporary = parent.join(temporary_name);
    let placed = (|| -> io::Result<()> {
        let mut source = std::fs::File::open(staged_file)?;
        let mut target = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        let copied = copy(&mut source, &mut target).and_then(|_| target.sync_all());
        // Windows refuses to rename a file that is still open.
        drop(target);
        copied?;
        rename(&temporary, destination)
    })();
    match placed {
        Ok(()) => {
            // The transfer is at its destination; a staged copy that refuses
            // to disappear is swept with the staging directory, not reported
            // as a failed save.
            let _ = std::fs::remove_file(staged_file);
            Ok(())
        }
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
        }
    }

    fn registry_with(ids: &[&str]) -> Registry {
        let mut registry = Registry::default();
        for id in ids {
            let download = record(id, &url(id));
            registry.begin(download).expect("within the live cap");
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
        // Leading dots survive (they are a legal part of a name); only the
        // separators are replaced.
        assert_eq!(
            sanitize_file_name(Some(OsStr::new("../../etc/passwd"))),
            ".._.._etc_passwd"
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
        // Leading dots are a page's or a server's choice (.bashrc); only
        // trailing dots are dropped, the way Windows drops them.
        assert_eq!(sanitize_file_name(Some(OsStr::new(".bashrc"))), ".bashrc");
        assert_eq!(sanitize_file_name(Some(OsStr::new("..x.pdf"))), "..x.pdf");
        assert_eq!(sanitize_file_name(Some(OsStr::new("name.."))), "name");
        // Current Windows documentation reserves the zero-numbered device
        // names, and Windows 11 adds the console streams.
        assert_eq!(
            sanitize_file_name(Some(OsStr::new("com0.txt"))),
            "_com0.txt"
        );
        assert_eq!(sanitize_file_name(Some(OsStr::new("LPT0"))), "_LPT0");
        assert_eq!(
            sanitize_file_name(Some(OsStr::new("conin$.txt"))),
            "_conin$.txt"
        );
        assert_eq!(
            sanitize_file_name(Some(OsStr::new("conout$.log"))),
            "_conout$.log"
        );
        assert_eq!(sanitize_file_name(Some(OsStr::new("..."))), "download");
        assert_eq!(
            sanitize_file_name(Some(OsStr::new("季度报告.pdf"))),
            "季度报告.pdf"
        );
        // Bidirectional overrides and zero-width marks render deceptively;
        // a reversed suffix must not masquerade as a harmless extension.
        assert_eq!(
            sanitize_file_name(Some(OsStr::new("report\u{202E}fdp.pdf"))),
            "report_fdp.pdf"
        );
        assert_eq!(
            sanitize_file_name(Some(OsStr::new("\u{200B}x.pdf"))),
            "_x.pdf"
        );
        // Classic Win32 reads the device name before the first dot.
        assert_eq!(
            sanitize_file_name(Some(OsStr::new("CON.tar.gz"))),
            "_CON.tar.gz"
        );
        assert_eq!(
            sanitize_file_name(Some(OsStr::new("lpt9.bak.gz"))),
            "_lpt9.bak.gz"
        );
    }

    #[test]
    fn downloads_are_capped_so_one_page_cannot_grow_staging_without_bound() {
        // The documented product constant: the cap itself is pinned elsewhere
        // by source, and here by value, so a change cannot pass unnoticed.
        assert_eq!(MAX_LIVE_DOWNLOADS, 16);
        let mut registry = Registry::default();
        let many: Vec<String> = (0..MAX_LIVE_DOWNLOADS - 1)
            .map(|index| format!("d{index}"))
            .collect();
        for id in many.iter() {
            let download = record(id, &url(id));
            registry.begin(download).expect("within the live cap");
        }
        // The last slot is still free at exactly MAX - 1 live downloads.
        assert!(registry.has_capacity());
        let download = record("last", &url("last"));
        // The dialog slot is already held by the first record, so the last
        // admitted transfer only starts.
        assert_eq!(
            registry.begin(download).expect("within the live cap"),
            vec![started("last")]
        );
        // The cap refuses exactly at MAX live downloads, not one later.
        assert!(!registry.has_capacity());
        assert_eq!(
            registry.begin(record("over", &url("over"))),
            Err(CapacityFull)
        );
        assert_eq!(
            registry.begin(record("still-over", &url("still-over"))),
            Err(CapacityFull)
        );
    }

    #[test]
    fn abandoning_a_queued_transfer_leaves_the_open_dialog_alone() {
        let mut registry = registry_with(&["a", "b"]);
        // a owns the dialog; the late prompt task of queued b must not steal it.
        assert_eq!(registry.abandon_prompt("b"), Vec::<Effect>::new());
        assert_eq!(registry.prompting.as_deref(), Some("a"));
        assert!(registry.is_asking("b"));
    }

    #[test]
    fn a_relative_staging_root_is_made_absolute_for_the_engine() {
        // The engine requires an absolute destination; a relative PINVOU3_HOME
        // override would otherwise write the transfer to a cwd-relative path.
        let current = std::env::current_dir().expect("a working directory");
        let absolute = absolute_staging_root_with(Path::new("relative/downloads"), Some(&current))
            .expect("a usable working directory");
        assert!(absolute.is_absolute());
        assert!(absolute.ends_with("relative/downloads"));
        assert_eq!(
            absolute_staging_root_with(Path::new("/already/absolute"), None),
            Some(PathBuf::from("/already/absolute"))
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
    fn reserved_device_names_are_avoided_after_trimming_and_cutting() {
        // Windows strips trailing spaces before the dot, so the padding is
        // part of the device candidate.
        assert_eq!(
            sanitize_file_name(Some(OsStr::new("CON  .txt"))),
            "_CON  .txt"
        );
        assert_eq!(
            sanitize_file_name(Some(OsStr::new("NUL  .pdf"))),
            "_NUL  .pdf"
        );
        assert_eq!(
            sanitize_file_name(Some(OsStr::new("com1 .exe"))),
            "_com1 .exe"
        );
        // A cut that strips the padding can land on a bare device name; the
        // check therefore repeats on the final stem.
        let padded = format!("CON{}zzz", " ".repeat(117));
        assert_eq!(sanitize_file_name(Some(OsStr::new(&padded))), "_CON");
        // The classic segment-before-the-first-dot cases stay covered.
        assert_eq!(
            sanitize_file_name(Some(OsStr::new("CON.tar.gz"))),
            "_CON.tar.gz"
        );
        // Superscript digits are device aliases too.
        assert_eq!(
            sanitize_file_name(Some(OsStr::new("com\u{00B9}.txt"))),
            "_com\u{00B9}.txt"
        );
        assert_eq!(
            sanitize_file_name(Some(OsStr::new("prn.lpt.aux"))),
            "_prn.lpt.aux"
        );
    }

    #[test]
    fn forbidden_characters_and_reserved_names_are_pinned_individually() {
        assert_eq!(
            sanitize_file_name(Some(OsStr::new("a\\b*c\"d<e>f|g"))),
            "a_b_c_d_e_f_g"
        );
        assert_eq!(
            sanitize_file_name(Some(OsStr::new("re\u{00AD}port.pdf"))),
            "re_port.pdf"
        );
        // Hangul fillers and Mongolian variation selectors are invisible too.
        assert_eq!(
            sanitize_file_name(Some(OsStr::new("a\u{115F}b.pdf"))),
            "a_b.pdf"
        );
        assert_eq!(
            sanitize_file_name(Some(OsStr::new("a\u{180B}b.pdf"))),
            "a_b.pdf"
        );
        assert_eq!(
            sanitize_file_name(Some(OsStr::new("a\u{180E}b.pdf"))),
            "a_b.pdf"
        );
        assert_eq!(
            sanitize_file_name(Some(OsStr::new("\u{FEFF}a.pdf"))),
            "_a.pdf"
        );
        assert_eq!(
            sanitize_file_name(Some(OsStr::new("\u{2060}j.pdf"))),
            "_j.pdf"
        );
        assert_eq!(
            sanitize_file_name(Some(OsStr::new("\u{061C}x.pdf"))),
            "_x.pdf"
        );
        assert_eq!(sanitize_file_name(Some(OsStr::new("PRN.txt"))), "_PRN.txt");
        assert_eq!(sanitize_file_name(Some(OsStr::new("aux"))), "_aux");
        assert_eq!(sanitize_file_name(Some(OsStr::new("nul"))), "_nul");
        // A 24-byte extension stays an extension; one byte more is part of
        // the stem and is cut with it.
        let kept = format!("{}.{}", "y".repeat(200), "e".repeat(23));
        assert_eq!(
            sanitize_file_name(Some(OsStr::new(&kept))),
            format!(
                "{}.{}",
                "y".repeat(MAX_FILE_NAME_BYTES - 24),
                "e".repeat(23)
            )
        );
        let demoted = format!("{}.{}", "y".repeat(200), "e".repeat(24));
        assert_eq!(
            sanitize_file_name(Some(OsStr::new(&demoted))),
            "y".repeat(MAX_FILE_NAME_BYTES)
        );
    }

    #[test]
    fn a_refused_transfers_cancellation_never_fails_a_live_transfer() {
        let mut registry = registry_with(&["a"]);
        // The refusal happened before admission: no record exists for it, but
        // the engine still reports a pathless failure for the URL. The credit
        // spends that report instead of failing the live member.
        registry.note_refusal(&url("a"));
        assert_eq!(
            registry.transfer_finished(&url("a"), None, false),
            Vec::<Effect>::new()
        );
        assert_eq!(registry.downloads["a"].transfer, Transfer::InFlight);
        // The live member still resolves normally afterwards.
        assert_eq!(finish_ok(&mut registry, "a"), Vec::<Effect>::new());
        assert_eq!(registry.downloads["a"].transfer, Transfer::Succeeded);
        assert_eq!(
            registry.answer("a", Some(chosen("a"))),
            vec![place("a", chosen("a"))]
        );
        assert!(registry.refusal_credits.is_empty());
    }

    #[test]
    fn a_refusal_credit_is_spent_on_a_failure_not_a_success() {
        let mut registry = registry_with(&["a"]);
        registry.note_refusal(&url("a"));
        // A success report is never refusal noise: it resolves the single
        // member, and the credit survives for the noise report to come.
        assert_eq!(
            registry.transfer_finished(&url("a"), None, true),
            Vec::<Effect>::new()
        );
        assert_eq!(registry.downloads["a"].transfer, Transfer::Succeeded);
        assert_eq!(registry.refusal_credits[&url("a")], 1);
        assert_eq!(
            registry.answer("a", Some(chosen("a"))),
            vec![place("a", chosen("a"))]
        );
        // The noise failure afterwards is spent, not voted.
        assert_eq!(
            registry.transfer_finished(&url("a"), None, false),
            Vec::<Effect>::new()
        );
        assert!(registry.refusal_credits.is_empty());
    }

    #[test]
    fn refusal_noise_cannot_touch_another_url_or_the_next_generation() {
        let mut registry = registry_with(&["a", "b"]);
        registry.note_refusal(&url("a"));
        // The noise report is spent even though another URL has live members.
        assert_eq!(
            registry.transfer_finished(&url("a"), None, false),
            Vec::<Effect>::new()
        );
        assert_eq!(registry.downloads["a"].transfer, Transfer::InFlight);
        // With the credit spent, a's own failure report is a member report
        // again, and b of another URL is never involved.
        assert_eq!(
            registry.transfer_finished(&url("a"), None, false),
            vec![discard("a"), finished("a", Outcome::Failed)]
        );
        assert_eq!(registry.downloads["b"].transfer, Transfer::InFlight);
        // b itself resolves through its written path, so the URL has no
        // live member when the next generation begins.
        assert_eq!(finish_ok(&mut registry, "b"), Vec::<Effect>::new());
        // A credit noted in that state is still spent before the next
        // generation's member reports, so that report resolves it.
        registry.note_refusal(&url("b"));
        registry
            .begin(record("c", &url("b")))
            .expect("within the live cap");
        assert_eq!(
            registry.transfer_finished(&url("b"), None, false),
            Vec::<Effect>::new()
        );
        assert_eq!(
            registry.transfer_finished(&url("b"), None, false),
            vec![discard("c"), finished("c", Outcome::Failed)]
        );
        assert!(registry.pending_finishes.is_empty());
        assert!(registry.refusal_credits.is_empty());
    }

    #[test]
    fn refusal_noise_swaps_a_failure_report_without_changing_the_verdict() {
        let mut registry = Registry::default();
        registry
            .begin(record("a", URL_A))
            .expect("within the live cap");
        registry
            .begin(record("b", URL_A))
            .expect("within the live cap");
        registry.answer("a", Some(chosen("a")));
        registry.answer("b", Some(chosen("b")));
        registry.note_refusal(URL_A);
        // A member failure, the noise, and a member success — the noise
        // arrives between them: the verdict stays the member outcome mix
        // (disagreement fails both closed), exactly as without the noise.
        assert_eq!(
            registry.transfer_finished(URL_A, None, false),
            Vec::<Effect>::new()
        );
        assert_eq!(
            registry.transfer_finished(URL_A, None, false),
            Vec::<Effect>::new()
        );
        assert_eq!(
            registry.transfer_finished(URL_A, None, true),
            vec![
                discard("a"),
                finished("a", Outcome::Failed),
                discard("b"),
                finished("b", Outcome::Failed),
            ]
        );
        assert!(registry.pending_finishes.is_empty());
        assert!(registry.refusal_credits.is_empty());
    }

    #[test]
    fn refusal_noise_never_poisons_uniform_member_successes() {
        let mut registry = Registry::default();
        registry
            .begin(record("a", URL_A))
            .expect("within the live cap");
        registry
            .begin(record("b", URL_A))
            .expect("within the live cap");
        registry.answer("a", Some(chosen("a")));
        registry.answer("b", Some(chosen("b")));
        registry.note_refusal(URL_A);
        // Both members succeed; their reports resolve the batch before the
        // noise failure arrives, which is then spent on the credit.
        assert_eq!(
            registry.transfer_finished(URL_A, None, true),
            Vec::<Effect>::new()
        );
        assert_eq!(
            registry.transfer_finished(URL_A, None, true),
            vec![place("a", chosen("a")), place("b", chosen("b"))]
        );
        assert_eq!(
            registry.transfer_finished(URL_A, None, false),
            Vec::<Effect>::new()
        );
        assert!(registry.refusal_credits.is_empty());
    }

    #[test]
    fn pathless_completions_of_one_url_never_touch_another_urls_transfers() {
        let mut registry = Registry::default();
        for id in ["a1", "a2"] {
            registry
                .begin(record(id, &url("a")))
                .expect("within the live cap");
        }
        for id in ["b1", "b2"] {
            registry
                .begin(record(id, &url("b")))
                .expect("within the live cap");
        }
        // The first failure waits in its own URL's batch; it must not count
        // the other URL's transfers as members.
        assert_eq!(
            registry.transfer_finished(&url("a"), None, false),
            Vec::<Effect>::new()
        );
        for id in ["a1", "a2", "b1", "b2"] {
            assert_eq!(registry.downloads[id].transfer, Transfer::InFlight);
        }
        // The other URL's failure resolves only its own two members.
        assert_eq!(
            registry.transfer_finished(&url("b"), None, false),
            Vec::<Effect>::new()
        );
        assert_eq!(
            registry.transfer_finished(&url("b"), None, false),
            vec![
                discard("b1"),
                finished("b1", Outcome::Failed),
                discard("b2"),
                finished("b2", Outcome::Failed),
            ]
        );
        // a1/a2 are still waiting for their own second report.
        for id in ["a1", "a2"] {
            assert_eq!(registry.downloads[id].transfer, Transfer::InFlight);
        }
        assert_eq!(
            registry.transfer_finished(&url("a"), None, false),
            vec![
                discard("a1"),
                finished("a1", Outcome::Failed),
                discard("a2"),
                finished("a2", Outcome::Failed),
            ]
        );
    }

    #[test]
    fn a_written_path_matches_only_its_own_staged_file() {
        let mut registry = Registry::default();
        for id in ["a", "b"] {
            let mut download = record(id, &url(id));
            // Two transfers of different URLs whose suggestions share a file
            // name: only the staging directory tells them apart.
            download.notice.file_name = "same.bin".to_string();
            download.staged_file = PathBuf::from("/staging").join(id).join("same.bin");
            registry.begin(download).expect("within the live cap");
        }
        let b_path = PathBuf::from("/staging").join("b").join("same.bin");
        // The written path resolves b only; a keeps streaming.
        assert_eq!(
            registry.transfer_finished(&url("b"), Some(&b_path), true),
            Vec::<Effect>::new()
        );
        assert_eq!(registry.downloads["b"].transfer, Transfer::Succeeded);
        assert_eq!(registry.downloads["a"].transfer, Transfer::InFlight);
        // a frees the dialog, handing it to the queued b, which then places.
        assert_eq!(
            registry.answer("a", Some(chosen("a"))),
            vec![Effect::Prompt(Notice {
                id: "b".to_string(),
                session_id: "session".to_string(),
                tab: "tab".to_string(),
                file_name: "same.bin".to_string(),
            })]
        );
        assert_eq!(
            registry.answer("b", Some(chosen("b"))),
            vec![Effect::Place {
                id: "b".to_string(),
                staging_dir: PathBuf::from("/staging").join("b"),
                staged_file: b_path,
                destination: chosen("b"),
            }]
        );
    }

    #[test]
    fn an_unknown_path_report_never_joins_a_url_batch() {
        let mut registry = registry_with(&["a"]);
        // A completion whose written path matches no record is stale noise;
        // it must not consume the URL's batch vote.
        assert_eq!(
            registry.transfer_finished(
                URL_A,
                Some(&PathBuf::from("/staging/ghost/ghost.bin")),
                true
            ),
            Vec::<Effect>::new()
        );
        assert_eq!(registry.downloads["a"].transfer, Transfer::InFlight);
        // The member's own report still resolves the batch.
        assert_eq!(
            registry.transfer_finished(URL_A, None, true),
            Vec::<Effect>::new()
        );
        assert_eq!(registry.downloads["a"].transfer, Transfer::Succeeded);
    }

    #[test]
    fn simultaneous_downloads_share_one_dialog_in_order() {
        let mut registry = Registry::default();
        assert_eq!(
            registry
                .begin(record("a", URL_A))
                .expect("within the live cap"),
            vec![started("a"), prompt("a")]
        );
        assert_eq!(
            registry
                .begin(record("b", &url("b")))
                .expect("within the live cap"),
            vec![started("b")]
        );
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
    fn finished_matches_the_written_path_then_the_one_matching_url() {
        let same = "https://site.invalid/file?token=one-time";
        let mut registry = Registry::default();
        for id in ["a", "b", "c"] {
            registry.begin(record(id, same));
        }
        // A completion without a written path cannot name its owner while two
        // other transfers stream the same URL: it waits.
        assert_eq!(registry.transfer_finished(same, None, false), vec![]);
        assert_eq!(registry.downloads["a"].transfer, Transfer::InFlight);
        // A reported path wins over the URL at any time.
        assert_eq!(
            registry.transfer_finished(same, Some(&staged_path("c")), false),
            vec![discard("c"), finished("c", Outcome::Failed)]
        );
        // Now every transfer of the URL has reported a failure, so both
        // remaining transfers fail no matter which report named which.
        assert_eq!(
            registry.transfer_finished(same, None, false),
            vec![
                discard("a"),
                finished("a", Outcome::Failed),
                discard("b"),
                finished("b", Outcome::Failed)
            ]
        );
        // Unknown and repeated reports change nothing.
        assert_eq!(registry.transfer_finished(same, None, true), vec![]);
        assert_eq!(
            registry.transfer_finished(&url("other"), Some(&staged_path("a")), true),
            vec![]
        );
        assert!(registry.downloads.is_empty());
        assert!(registry.pending_finishes.is_empty());
    }

    #[test]
    fn same_url_completions_without_a_path_wait_until_all_have_reported() {
        let mut registry = Registry::default();
        registry.begin(record("a", URL_A));
        registry.begin(record("b", URL_A));
        // The first pathless completion is ambiguous and waits.
        assert_eq!(registry.transfer_finished(URL_A, None, true), vec![]);
        assert_eq!(registry.downloads["a"].transfer, Transfer::InFlight);
        assert_eq!(registry.downloads["b"].transfer, Transfer::InFlight);
        // With every transfer of the URL completed, uniform outcomes attribute
        // cleanly even though the owner of each report is unknown.
        assert_eq!(registry.transfer_finished(URL_A, None, true), vec![]);
        assert_eq!(registry.downloads["a"].transfer, Transfer::Succeeded);
        assert_eq!(registry.downloads["b"].transfer, Transfer::Succeeded);
        assert!(registry.pending_finishes.is_empty());
        // Each waits for its own dialog answer; answering a hands the dialog
        // to b, and neither places early.
        assert_eq!(
            registry.answer("a", Some(chosen("a"))),
            vec![place("a", chosen("a")), prompt("b")]
        );
        assert_eq!(
            registry.answer("b", Some(chosen("b"))),
            vec![place("b", chosen("b"))]
        );
        assert_eq!(
            registry.placed("a", true),
            vec![finished("a", Outcome::Saved)]
        );
        assert_eq!(
            registry.placed("b", true),
            vec![finished("b", Outcome::Saved)]
        );
        assert!(registry.downloads.is_empty());
    }

    #[test]
    fn ambiguous_completions_that_disagree_fail_closed_instead_of_guessing() {
        let mut registry = Registry::default();
        registry.begin(record("a", URL_A));
        registry.begin(record("b", URL_A));
        assert_eq!(registry.answer("a", Some(chosen("a"))), vec![prompt("b")]);
        assert_eq!(registry.answer("b", Some(chosen("b"))), vec![]);
        // One transfer succeeded and one failed, but which is which is unknowable:
        // nothing ships as saved, and both are discarded as failed.
        assert_eq!(registry.transfer_finished(URL_A, None, true), vec![]);
        assert_eq!(
            registry.transfer_finished(URL_A, None, false),
            vec![
                discard("a"),
                finished("a", Outcome::Failed),
                discard("b"),
                finished("b", Outcome::Failed)
            ]
        );
        assert!(registry.downloads.is_empty());
        assert!(registry.pending_finishes.is_empty());
    }

    #[test]
    fn a_path_matched_member_votes_so_no_batch_strands_or_poisons() {
        let mut registry = Registry::default();
        registry
            .begin(record("a", URL_A))
            .expect("within the live cap");
        registry
            .begin(record("b", URL_A))
            .expect("within the live cap");
        assert_eq!(registry.answer("a", Some(chosen("a"))), vec![prompt("b")]);
        assert_eq!(registry.answer("b", Some(chosen("b"))), vec![]);
        // One transfer fails without a written path while its sibling still
        // streams: the batch waits for both members.
        assert_eq!(registry.transfer_finished(URL_A, None, false), vec![]);
        // The sibling resolves through its written path, votes, and completes
        // the batch on the spot: the failed member settles now instead of
        // stranding until restart, and no stale batch is left for the URL's
        // next generation.
        assert_eq!(
            registry.transfer_finished(URL_A, Some(&staged_path("b")), true),
            vec![
                place("b", chosen("b")),
                discard("a"),
                finished("a", Outcome::Failed)
            ]
        );
        assert!(registry.pending_finishes.is_empty());
        // The next generation of the same URL starts with clean accounting.
        registry
            .begin(record("c", URL_A))
            .expect("within the live cap");
        assert_eq!(
            registry.transfer_finished(URL_A, None, false),
            vec![discard("c"), finished("c", Outcome::Failed)]
        );
        assert!(registry.pending_finishes.is_empty());
    }

    #[test]
    fn a_transfer_beginning_during_a_batch_joins_it() {
        let mut registry = Registry::default();
        registry
            .begin(record("a", URL_A))
            .expect("within the live cap");
        registry
            .begin(record("b", URL_A))
            .expect("within the live cap");
        assert_eq!(registry.transfer_finished(URL_A, None, false), vec![]);
        // A fresh transfer of the same URL begins while the batch is open; it
        // will also report, so the batch counts it in instead of resolving
        // early against the members it already knows.
        registry
            .begin(record("c", URL_A))
            .expect("within the live cap");
        assert_eq!(registry.transfer_finished(URL_A, None, false), vec![]);
        assert_eq!(registry.downloads["a"].transfer, Transfer::InFlight);
        // The last member's report completes the batch: with mixed outcomes
        // every member fails closed.
        assert_eq!(
            registry.transfer_finished(URL_A, None, false),
            vec![
                discard("a"),
                finished("a", Outcome::Failed),
                discard("b"),
                finished("b", Outcome::Failed),
                discard("c"),
                finished("c", Outcome::Failed),
            ]
        );
        assert!(registry.pending_finishes.is_empty());
    }

    #[test]
    fn a_second_transfer_of_the_same_url_is_not_confused_with_a_finished_one() {
        let mut registry = registry_with(&["a"]);
        finish_ok(&mut registry, "a");
        // The first transfer is complete and still waits for the user when
        // the page starts the same download again.
        registry.begin(record("b", URL_A));
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
    fn only_an_unsaved_transfer_can_be_discarded() {
        let mut registry = registry_with(&["a", "b"]);
        // Still streaming: not discardable.
        assert_eq!(registry.discard("a"), vec![]);
        // Unknown ids change nothing, so a repeated click never reports twice.
        assert_eq!(registry.discard("missing"), vec![]);
        // An unsaved record leaves with its staged file, reported cancelled.
        finish_ok(&mut registry, "a");
        registry.answer("a", Some(chosen("a")));
        assert_eq!(
            registry.placed("a", false),
            vec![finished("a", Outcome::Unsaved)]
        );
        assert_eq!(
            registry.discard("a"),
            vec![discard("a"), finished("a", Outcome::Cancelled)]
        );
        assert_eq!(registry.discard("a"), vec![]);
        // A finished record still waiting for its dialog is not discardable.
        finish_ok(&mut registry, "b");
        assert_eq!(registry.discard("b"), vec![]);
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
    fn a_placement_result_never_applies_to_a_record_that_left_the_placing_state() {
        let mut registry = registry_with(&["a"]);
        finish_ok(&mut registry, "a");
        registry.answer("a", Some(chosen("a")));
        assert_eq!(
            registry.placed("a", false),
            vec![finished("a", Outcome::Unsaved)]
        );
        // The record is back in `unsaved` (waiting for a retry dialog): a
        // late or duplicate placement result neither re-saves nor re-reports
        // it, so the user never sees a second `unsaved` notice for one click.
        assert_eq!(registry.placed("a", true), vec![]);
        assert_eq!(registry.placed("a", false), vec![]);
        assert!(registry.downloads.contains_key("a"));
    }

    #[test]
    fn a_lost_tab_report_settles_its_record_and_releases_its_batch_seat() {
        let mut registry = Registry::default();
        registry
            .begin(record("a", URL_A))
            .expect("within the live cap");
        registry
            .begin(record("b", URL_A))
            .expect("within the live cap");
        registry.answer("a", Some(chosen("a")));
        registry.answer("b", Some(chosen("b")));
        let page = DownloadPage {
            session_id: "session".to_string(),
            tab: "tab".to_string(),
        };
        assert_eq!(
            registry.in_flight_ids_of_page(&page),
            vec![
                ("a".to_string(), URL_A.to_string()),
                ("b".to_string(), URL_A.to_string())
            ]
        );
        // a's tab closed (macOS, Windows): the completion can never arrive,
        // so the record fails like any failed transfer.
        assert_eq!(
            registry.report_lost("a", URL_A),
            vec![discard("a"), finished("a", Outcome::Failed)]
        );
        assert!(!registry.downloads.contains_key("a"));
        assert_eq!(registry.downloads["b"].transfer, Transfer::InFlight);
        // b's own report still resolves it: the lost member cannot hold the
        // batch back.
        assert_eq!(
            registry.transfer_finished(URL_A, None, true),
            vec![place("b", chosen("b"))]
        );
        assert!(registry.pending_finishes.is_empty());
    }

    #[test]
    fn a_lost_member_does_not_fail_its_successful_sibling() {
        let mut registry = Registry::default();
        registry
            .begin(record("a", URL_A))
            .expect("within the live cap");
        registry
            .begin(record("b", URL_A))
            .expect("within the live cap");
        registry.answer("a", Some(chosen("a")));
        registry.answer("b", Some(chosen("b")));
        // b reports success (pathless, macOS), which opens the batch and
        // leaves one seat for a.
        assert_eq!(registry.transfer_finished(URL_A, None, true), vec![]);
        // a's tab closes. Its seat is released without a vote, so b's lone
        // success stays uniform and b is placed — not failed alongside the
        // member whose completion was lost.
        assert_eq!(
            registry.report_lost("a", URL_A),
            vec![
                discard("a"),
                finished("a", Outcome::Failed),
                place("b", chosen("b")),
            ]
        );
        assert!(registry.pending_finishes.is_empty());
        // b now sits in `placing` (the host executes the Place effect); the
        // registry-level test ends here, the placement result is its business.
        assert_eq!(registry.downloads["b"].intent, Intent::Placing);
        // The next generation of the URL resolves on its own report alone:
        // the lost member left nothing behind for it.
        registry
            .begin(record("c", URL_A))
            .expect("within the live cap");
        registry.answer("c", Some(chosen("c")));
        assert_eq!(
            registry.transfer_finished(URL_A, None, true),
            vec![place("c", chosen("c"))]
        );
        assert!(registry.pending_finishes.is_empty());
    }

    #[test]
    fn a_lost_member_without_a_report_leaves_no_batch_behind() {
        let mut registry = Registry::default();
        registry
            .begin(record("a", URL_A))
            .expect("within the live cap");
        // Lost before any report of its URL: nothing opened, nothing waits.
        assert_eq!(
            registry.report_lost("a", URL_A),
            vec![discard("a"), finished("a", Outcome::Failed)]
        );
        assert!(registry.pending_finishes.is_empty());
        // A loss for an unknown id changes nothing.
        assert_eq!(registry.report_lost("missing", URL_A), vec![]);
        // And a loss after the transfer already reported changes nothing.
        registry
            .begin(record("b", URL_A))
            .expect("within the live cap");
        finish_ok(&mut registry, "b");
        assert_eq!(registry.report_lost("b", URL_A), vec![]);
    }

    #[test]
    fn refusal_credits_are_capped_so_a_storm_cannot_grow_them_without_bound() {
        let mut registry = Registry::default();
        for index in 0..MAX_REFUSAL_CREDITS {
            registry.note_refusal(&format!("https://site.invalid/{index}"));
        }
        assert_eq!(registry.refusal_credits.len(), MAX_REFUSAL_CREDITS);
        // A further refusal past the cap is not credited; its cancellation
        // report then votes as ordinary noise, which the outcome mix
        // already tolerates.
        registry.note_refusal(&url("a"));
        assert_eq!(registry.refusal_credits.len(), MAX_REFUSAL_CREDITS);
        assert!(!registry.refusal_credits.contains_key(&url("a")));
        // An already-credited URL keeps counting within its entry.
        registry.note_refusal("https://site.invalid/0");
        assert_eq!(registry.refusal_credits["https://site.invalid/0"], 2);
    }

    #[test]
    fn unsaved_payloads_carry_only_unsaved_records() {
        let mut registry = registry_with(&["a", "b"]);
        finish_ok(&mut registry, "a");
        registry.answer("a", Some(chosen("a")));
        assert_eq!(
            registry.placed("a", false),
            vec![finished("a", Outcome::Unsaved)]
        );
        // b is still streaming and asking: it is not unsaved.
        let payloads = registry.unsaved_payloads();
        assert_eq!(payloads.len(), 1);
        assert_eq!(payloads[0]["downloadId"], json!("a"));
        assert_eq!(payloads[0]["sessionId"], json!("session"));
        assert_eq!(payloads[0]["outcome"], json!("unsaved"));
        assert!(payloads[0].get("url").is_none());
        assert!(payloads[0].get("destination").is_none());
    }

    #[test]
    fn a_missing_working_directory_leaves_no_usable_staging_root() {
        // Relative + no working directory: no absolute form exists, and the
        // coordinator must come up out of service rather than degrade to a
        // cwd-relative root the engine writes beside.
        assert_eq!(
            absolute_staging_root_with(Path::new("rel/downloads"), None),
            None
        );
        assert_eq!(
            absolute_staging_root_with(Path::new("rel/downloads"), Some(Path::new("/cwd"))),
            Some(PathBuf::from("/cwd/rel/downloads"))
        );
        assert_eq!(
            absolute_staging_root_with(Path::new("/already/absolute"), None),
            Some(PathBuf::from("/already/absolute"))
        );
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
        // The outcome strings cross the wire verbatim and are the keys the
        // frontend notice map understands; renaming one silently drops notices.
        assert_eq!(Outcome::Saved.as_str(), "saved");
        assert_eq!(Outcome::Cancelled.as_str(), "cancelled");
        assert_eq!(Outcome::Failed.as_str(), "failed");
        assert_eq!(Outcome::Unsaved.as_str(), "unsaved");
        // An explicit port is part of the origin a user can act on.
        assert_eq!(
            download_source(&Url::parse("https://site.invalid:8443/a").unwrap()),
            "https://site.invalid:8443"
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

    /// A sync stand-in that exercises the real flush on the real file.
    fn sync_staged(staged_file: &Path) -> io::Result<()> {
        sync_staged_bytes(staged_file)
    }

    #[test]
    fn staged_bytes_sync_through_a_writable_handle() {
        let temp = tempfile::tempdir().unwrap();
        let staged_file = staged(temp.path(), b"contents");
        // Windows refuses FlushFileBuffers on a read-only handle, so the
        // production sync must hold the file open for writing.
        sync_staged_bytes(&staged_file).expect("the staged file syncs");
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
        place_staged_file(&staged_file, &destination, "id").unwrap();
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
            ".pinvou-id.part",
            rename_across_volumes(&staged_file),
            |source, target| io::copy(source, target),
            sync_staged,
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
        assert!(
            place_staged_file_with(
                &staged_file,
                &destination,
                ".pinvou-id.part",
                rename,
                |source, target| io::copy(source, target),
                sync_staged,
            )
            .is_err()
        );
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
        // The first rename refuses like another volume, which forces the copy;
        // the swap onto the destination then fails the way a locked file or a
        // full disk makes it.
        let rename = |from: &Path, _to: &Path| {
            if from == staged_file {
                Err(io::Error::new(
                    io::ErrorKind::CrossesDevices,
                    "simulated EXDEV",
                ))
            } else {
                Err(io::Error::new(io::ErrorKind::Other, "locked"))
            }
        };
        assert!(
            place_staged_file_with(
                &staged_file,
                &destination,
                ".pinvou-id.part",
                rename,
                |source, target| io::copy(source, target),
                sync_staged,
            )
            .is_err()
        );
        assert_eq!(std::fs::read(&staged_file).unwrap(), b"contents");
        assert!(!destination.exists());
        assert!(entries(&folder).is_empty());
    }

    #[test]
    fn a_copy_that_dies_mid_stream_never_places_a_partial_file() {
        let temp = tempfile::tempdir().unwrap();
        let staged_file = staged(temp.path(), b"contents");
        let folder = temp.path().join("other-volume");
        std::fs::create_dir(&folder).unwrap();
        let destination = folder.join("file.bin");
        // The copy is forced onto another volume and dies mid-stream; the swap
        // itself would have succeeded, so only the copied-error guard stands
        // between a truncated transfer and the user's folder.
        let rename = |from: &Path, to: &Path| {
            if from == staged_file {
                Err(io::Error::new(
                    io::ErrorKind::CrossesDevices,
                    "simulated EXDEV",
                ))
            } else {
                std::fs::rename(from, to)
            }
        };
        let copy = |_: &mut std::fs::File, _: &mut std::fs::File| {
            Err(io::Error::new(io::ErrorKind::UnexpectedEof, "truncated"))
        };
        assert!(
            place_staged_file_with(
                &staged_file,
                &destination,
                ".pinvou-id.part",
                rename,
                copy,
                sync_staged,
            )
            .is_err()
        );
        assert!(!destination.exists());
        assert!(entries(&folder).is_empty());
        assert_eq!(std::fs::read(&staged_file).unwrap(), b"contents");
    }

    #[test]
    fn a_destination_inside_the_staging_root_is_rejected_for_placement() {
        let root = PathBuf::from("/home/u/.pinvou3/browser/downloads");
        assert!(destination_inside_staging(&root.join("id/file.bin"), &root));
        assert!(destination_inside_staging(&root, &root));
        assert!(!destination_inside_staging(
            &PathBuf::from("/home/u/Downloads/a.bin"),
            &root
        ));
        // Component-wise: a sibling whose name merely shares a prefix is
        // outside the root and stays allowed.
        assert!(!destination_inside_staging(
            &PathBuf::from("/home/u/.pinvou3/browser/downloads-backup/a.bin"),
            &root
        ));
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
                ".pinvou-id.part",
                rename_across_volumes(&staged_file),
                |source, target| io::copy(source, target),
                sync_staged,
            )
            .is_err()
        );
        assert_eq!(std::fs::read(&staged_file).unwrap(), b"contents");
    }

    #[test]
    fn placement_syncs_the_staged_bytes_before_any_rename() {
        let temp = tempfile::tempdir().unwrap();
        let staged_file = staged(temp.path(), b"contents");
        let folder = temp.path().join("other-volume");
        std::fs::create_dir(&folder).unwrap();
        let destination = folder.join("file.bin");
        // The first rename refuses like another volume, which forces the
        // cross-volume copy; its swap then succeeds.
        let log: std::sync::Arc<std::sync::Mutex<Vec<&'static str>>> =
            std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let sync_log = std::sync::Arc::clone(&log);
        let sync = move |_: &Path| {
            sync_log.lock().unwrap().push("sync");
            Ok(())
        };
        let rename_log = std::sync::Arc::clone(&log);
        let staged = staged_file.clone();
        let rename = move |from: &Path, to: &Path| {
            let step = if from == staged { "rename" } else { "swap" };
            rename_log.lock().unwrap().push(step);
            if from == staged {
                Err(io::Error::new(
                    io::ErrorKind::CrossesDevices,
                    "simulated EXDEV",
                ))
            } else {
                std::fs::rename(from, to)
            }
        };
        place_staged_file_with(
            &staged_file,
            &destination,
            ".pinvou-id.part",
            rename,
            |source, target| io::copy(source, target),
            sync,
        )
        .unwrap();
        // The staged bytes are flushed once, before the same-volume rename
        // attempt and before the copy's swap: a power loss can never leave a
        // destination name whose data never reached the disk.
        assert_eq!(*log.lock().unwrap(), vec!["sync", "rename", "swap"]);
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
    fn requests_fall_back_to_the_shown_workspace_page() {
        let temp = tempfile::tempdir().unwrap();
        let downloads = BrowserDownloads::new(temp.path().join("staging"));
        let shown = DownloadPage {
            session_id: "shown-session".to_string(),
            tab: "shown-tab".to_string(),
        };
        // No label page and nothing shown: the request is refused.
        assert_eq!(downloads.page_for_request("agent-browser-x"), None);
        downloads.set_shown_page(Some(shown.clone()));
        assert_eq!(
            downloads.page_for_request("agent-browser-x"),
            Some(shown.clone())
        );
        // A label page wins over the shown workspace (macOS, Windows).
        let tabbed = DownloadPage {
            session_id: "tab-session".to_string(),
            tab: "tab".to_string(),
        };
        downloads.register_page("agent-browser-tab", tabbed.clone());
        assert_eq!(
            downloads.page_for_request("agent-browser-tab"),
            Some(tabbed)
        );
        // A closed tab's page is dropped, returning the request to the shown
        // workspace.
        downloads.unregister_page("agent-browser-tab");
        assert_eq!(downloads.page_for_request("agent-browser-tab"), Some(shown));
        downloads.set_shown_page(None);
        assert_eq!(downloads.page_for_request("agent-browser-tab"), None);
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
