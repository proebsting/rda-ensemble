// A window, not a second implementation. Every command here is a thin call
// into rda-ensemble, which is the same library the CLI drives.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use rda_ensemble::datasets::{self, Dataset};
use rda_ensemble::dra::{self, Cached, Listing};
use rda_ensemble::events::{Events, Sink};
use rda_ensemble::run::{self, Cancel};
use rda_ensemble::settings::Settings;
use rda_ensemble::extend;
use tauri::{AppHandle, Emitter, Manager};

/// What the window is doing, if anything.
#[derive(Default)]
struct Job {
    cancel: Option<Cancel>,
}

#[derive(Default)]
struct App {
    job: Mutex<Job>,
}

/// Narration from a run, forwarded to the window.
///
/// The same trait the CLI's terminal sink implements. Progress is throttled
/// here for the same reason it is there: a chain reports steps far faster
/// than anything needs to redraw, and every one of these crosses an IPC
/// boundary.
struct Forward {
    app: AppHandle,
    last_progress: Mutex<Instant>,
}

impl Events for Forward {
    fn status(&self, line: &str) {
        let _ = self.app.emit("status", line);
    }

    fn warn(&self, line: &str) {
        let _ = self.app.emit("warn", line);
    }

    fn progress(&self, step: u64, total: u64) {
        let now = Instant::now();
        {
            let mut last = self.last_progress.lock().expect("progress clock");
            // The final step always reports, so the bar does not stop short.
            if step + 1 < total && now.duration_since(*last).as_millis() < 100 {
                return;
            }
            *last = now;
        }
        let _ = self.app.emit("progress", (step + 1, total));
    }

    fn progress_done(&self) {
        let _ = self.app.emit("progress-done", ());
    }
}

fn cache_of(cache: Option<String>) -> Option<PathBuf> {
    cache.filter(|c| !c.trim().is_empty()).map(PathBuf::from)
}

/// Where packages are kept when the window does not say.
#[tauri::command]
fn default_cache() -> String {
    dra::default_cache().display().to_string()
}

/// The states DRA publishes, for the state menu.
#[tauri::command]
fn states(cache: Option<String>) -> Result<Vec<Listing>, String> {
    let cache = cache_of(cache).unwrap_or_else(dra::default_cache);
    let inventory = dra::Inventory::load(&cache).map_err(|e| format!("{e:#}"))?;
    dra::listings(&inventory, None).map_err(|e| format!("{e:#}"))
}

/// What a state's GeoJSON carries, for the dataset menus.
///
/// Downloads the package if it is not cached, which is why it reports
/// through the sink: it can take a while the first time.
#[tauri::command]
fn datasets(
    app: AppHandle,
    state: String,
    dra_version: Option<String>,
    cache: Option<String>,
) -> Result<Vec<Dataset>, String> {
    let ev: Sink = Arc::new(Forward {
        app,
        last_progress: Mutex::new(Instant::now()),
    });
    let cache = cache_of(cache).unwrap_or_else(dra::default_cache);
    let package = dra::resolve(&cache, &state, dra_version.as_deref(), &ev)
        .map_err(|e| format!("{e:#}"))?;
    datasets::read(&package.geojson).map_err(|e| format!("{e:#}"))
}

/// The values the form starts with.
///
/// Read out of clap rather than written again in JavaScript, so the window
/// and the command line cannot drift apart about what `--chains` defaults
/// to. Only the fields that genuinely have a default are meaningful here;
/// the rest are placeholders the form overwrites.
#[tauri::command]
fn form_defaults() -> Settings {
    Settings::starting_point()
}

/// Read an uploaded file: a settings file, or someone's `manifest.json`.
#[tauri::command]
fn parse_settings(text: String) -> Result<Settings, String> {
    Settings::parse(&text).map_err(|e| format!("{e:#}"))
}

/// The settings as the file that will be downloaded.
#[tauri::command]
fn settings_json(settings: Settings) -> String {
    settings.to_json()
}

/// Anything that would stop someone else reproducing this run.
#[tauri::command]
fn portability_warnings(settings: Settings) -> Vec<String> {
    settings.portability_warnings()
}

/// Check the settings without running anything, as `--dry-run` does.
#[tauri::command]
fn check(settings: Settings, out: String, cache: Option<String>) -> Result<(), String> {
    let mut args = settings
        .into_args(PathBuf::from(out), cache_of(cache))
        .map_err(|e| format!("{e:#}"))?;
    args.dry_run = true;
    let ev: Sink = Arc::new(rda_ensemble::events::Silent);
    let keep = rda_ensemble::KeepArg::expand(&args.keep);
    run::run(&args, &keep, &Cancel::default(), &ev).map_err(|e| format!("{e:#}"))
}

/// Claim the one job slot, and hand back the pieces a job needs.
///
/// One run at a time: two chains writing the same directory would
/// interleave their output, and the Stop button has one thing to point at.
fn begin(app: &AppHandle) -> Result<(Cancel, Sink), String> {
    let state = app.state::<App>();
    let mut job = state.job.lock().expect("job");
    if job.cancel.is_some() {
        return Err("a run is already going".into());
    }
    let cancel = Cancel::default();
    job.cancel = Some(cancel.clone());
    let ev: Sink = Arc::new(Forward {
        app: app.clone(),
        last_progress: Mutex::new(Instant::now()),
    });
    Ok((cancel, ev))
}

/// Run `work` off the UI thread, free the job slot, and say how it went.
fn in_background(
    app: AppHandle,
    work: impl FnOnce() -> anyhow::Result<()> + Send + 'static,
) {
    std::thread::spawn(move || {
        let outcome = work();
        app.state::<App>().job.lock().expect("job").cancel = None;
        let _ = match outcome {
            Ok(()) => app.emit("done", serde_json::json!({ "ok": true })),
            Err(e) => app.emit(
                "done",
                serde_json::json!({ "ok": false, "error": format!("{e:#}") }),
            ),
        };
    });
}

/// Start a run. Returns as soon as it has started; everything after that
/// arrives as events.
#[tauri::command]
fn start(
    app: AppHandle,
    settings: Settings,
    out: String,
    cache: Option<String>,
) -> Result<(), String> {
    let (cancel, ev) = begin(&app)?;
    let args = settings
        .into_args(PathBuf::from(out), cache_of(cache))
        .map_err(|e| {
            app.state::<App>().job.lock().expect("job").cancel = None;
            format!("{e:#}")
        })?;
    in_background(app, move || {
        let keep = rda_ensemble::KeepArg::expand(&args.keep);
        run::run(&args, &keep, &cancel, &ev)
    });
    Ok(())
}

/// Run an existing ensemble's chain for longer.
///
/// The settings come from the earlier run, not from the form: extending is
/// about continuing one chain, and changing its parameters halfway would
/// make it a different one.
#[tauri::command]
fn start_extend(
    app: AppHandle,
    from: String,
    out: String,
    plans: Option<u64>,
    steps: Option<u64>,
    segment_steps: Option<u64>,
    cache: Option<String>,
) -> Result<(), String> {
    let (cancel, ev) = begin(&app)?;
    let (from, out) = (PathBuf::from(from), PathBuf::from(out));
    let cache = cache_of(cache);
    in_background(app, move || {
        extend::extend(&from, &out, plans, steps, segment_steps, cache, &ev, &cancel)
    });
    Ok(())
}

/// The settings a finished run used, read from its directory.
///
/// The window cannot read the filesystem itself, so this is also how a
/// settings file is loaded by path rather than through the file picker.
#[tauri::command]
fn settings_at(path: String) -> Result<Settings, String> {
    let path = PathBuf::from(path);
    let path = if path.is_dir() {
        extend::record_of(&path).map_err(|e| format!("{e:#}"))?
    } else {
        path
    };
    Settings::read(&path).map_err(|e| format!("{e:#}"))
}

/// What has been downloaded, and how much room it takes.
#[tauri::command]
fn cache_contents(cache: Option<String>) -> Vec<Cached> {
    dra::contents(&cache_of(cache).unwrap_or_else(dra::default_cache))
}

/// Delete cached packages, and say which went.
#[tauri::command]
fn cache_forget(
    cache: Option<String>,
    state: Option<String>,
    version: Option<String>,
    index: bool,
) -> Result<Vec<Cached>, String> {
    let dir = cache_of(cache).unwrap_or_else(dra::default_cache);
    dra::forget(&dir, state.as_deref(), version.as_deref(), index)
        .map_err(|e| format!("{e:#}"))
}

/// Ask the chain to stop at the end of the current segment.
///
/// Without `segment_steps` there is no boundary before the end, so this has
/// nothing to act on. The window says so rather than appearing to work.
#[tauri::command]
fn stop(app: AppHandle) -> Result<(), String> {
    match &app.state::<App>().job.lock().expect("job").cancel {
        Some(cancel) => {
            cancel.stop();
            Ok(())
        }
        None => Err("nothing is running".into()),
    }
}

fn main() {
    tauri::Builder::default()
        .manage(App::default())
        .invoke_handler(tauri::generate_handler![
            default_cache,
            states,
            datasets,
            form_defaults,
            parse_settings,
            settings_json,
            portability_warnings,
            check,
            start,
            start_extend,
            settings_at,
            cache_contents,
            cache_forget,
            stop,
        ])
        .run(tauri::generate_context!())
        .expect("starting the window");
}
