//! The window's state and the actions behind its buttons. Long work (folder checks, VIctory's
//! translate and game) runs on threads; this module drains their results once per frame.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};

use crate::app_check::ManifestState;
use crate::cache;
use crate::host::{self, Adapter, GpuVendor, Host};
use crate::job::{self, Event, Step};
use crate::launch;
use crate::manifest::Outcome;
use crate::picker::{self, Pick};
use crate::processes::Tasklist;
use crate::profile::{Profile, Profiles};
use crate::progress::{Progress, Summary};
use crate::session::{self, Settings};
use crate::setup::{self, Item};
use crate::splash::Splash;
use crate::store::{self, Stored};
use crate::toast::Toasts;
use crate::verify::Report;

/// Lines kept in the log view.
const LOG_CAPACITY: usize = 2000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Home,
    Setup,
    Files,
    Shaders,
    Play,
}

/// The game folder check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verify {
    NotRun,
    Running,
    Done(Report),
    Failed(String),
}

/// A job running on its thread, and whether it is the shader step.
pub struct Running {
    pub events: Receiver<Event>,
    pub shaders_only: bool,
}

pub struct App {
    pub repo: Option<PathBuf>,
    pub profiles: Profiles,
    pub host: Host,
    pub gpu: GpuVendor,
    pub adapter_name: Option<String>,
    adapters: Option<Vec<Adapter>>,
    pub setup: Vec<Item>,
    pub settings: Settings,
    pub tab: Tab,
    pub verify: Verify,
    pub(crate) verify_rx: Option<Receiver<Result<Report, String>>>,
    pub(crate) verify_started: Option<std::time::Instant>,
    /// The optional SHA-256 manifest job; it never gates PLAY.
    pub manifest: ManifestState,
    pub(crate) manifest_rx: Option<Receiver<Outcome>>,
    pub running: Option<Running>,
    pub log: VecDeque<String>,
    pub summary: Summary,
    pub notice: Option<String>,
    /// Set when the game is up and the launcher may close (see `keep_open_after_launch`).
    pub close_requested: bool,
    pub confirm_clean: bool,
    pub cache_dir: Option<PathBuf>,
    pub splash: Splash,
    pub toasts: Toasts,
    /// The step the running job is on (`Building VIctory`, `Running the game`...).
    pub step: Option<String>,
    pub drawer_open: bool,
    /// Whether the shader cache folder holds something (looked at every couple of seconds).
    pub cache_ready: bool,
    pub cache_checked_at: f64,
    /// When the page on screen was switched to, in egui seconds (for the page transition).
    pub tab_changed_at: f64,
    pub shown_tab: Tab,
    /// Start the game as soon as the folder check passes (PLAY pressed before it ended).
    pub launch_when_ready: bool,
    pick_rx: Option<Receiver<(Pick, Option<PathBuf>)>>,
    settings_path: Option<PathBuf>,
    last_saved: Stored,
}

/// The checklist as the machine is now; `adapters` is what DXGI listed (None when it failed).
fn read_setup(adapters: Option<&[crate::host::Adapter]>) -> Vec<Item> {
    let system_root = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
    let program_files = std::env::var_os("ProgramFiles(x86)")
        .map_or_else(|| PathBuf::from("C:\\Program Files (x86)"), PathBuf::from);
    let kits = program_files.join("Windows Kits").join("10").join("bin");
    let facts = setup::probe(
        &PathBuf::from(system_root),
        &kits,
        adapters.map(<[_]>::to_vec),
    );
    setup::checklist(&facts)
}

impl App {
    /// The window's first state: the setup checklist, the GPU and the host are read now.
    pub fn new() -> Result<Self, String> {
        let profiles = Profiles::embedded().map_err(|e| e.to_string())?;
        let settings_path = store::current_path();
        let settings = settings_path
            .as_deref()
            .and_then(store::load)
            .map(Stored::into_settings)
            .unwrap_or_default();
        let settings_copy = settings.clone();
        let host = host::detect_host();
        let adapters = host::enumerate_adapters().ok();
        let gpu = adapters
            .as_deref()
            .map_or(GpuVendor::Unknown, host::vendor_of);
        let adapter_name = adapters
            .as_deref()
            .and_then(host::main_adapter)
            .map(|a| a.name.clone());
        let mut app = App {
            repo: anyxboxx::run::find_repo(None),
            profiles,
            host,
            gpu,
            adapter_name,
            setup: read_setup(adapters.as_deref()),
            adapters,
            settings,
            tab: Tab::Home,
            verify: Verify::NotRun,
            verify_rx: None,
            verify_started: None,
            manifest: ManifestState::NotRun,
            manifest_rx: None,
            running: None,
            log: VecDeque::new(),
            summary: Summary::default(),
            notice: None,
            close_requested: false,
            confirm_clean: false,
            cache_dir: cache::current_cache_dir(),
            splash: Splash::new(),
            toasts: Toasts::default(),
            step: None,
            drawer_open: false,
            cache_ready: false,
            cache_checked_at: 0.0,
            tab_changed_at: 0.0,
            shown_tab: Tab::Home,
            last_saved: Stored::from_settings(&settings_copy),
            launch_when_ready: false,
            pick_rx: None,
            settings_path,
        };
        app.find_game_folder();
        if app.settings.game_path().is_some() {
            app.start_verify();
        }
        Ok(app)
    }

    /// Keeps the saved game folder when it still holds the game, else looks for one next to the
    /// launcher and in its parents.
    fn find_game_folder(&mut self) {
        let last = self.settings.game_path();
        let launcher = picker::launcher_dir();
        let found = picker::detect(launcher.as_deref(), last.as_deref());
        if let Some(dir) = found {
            self.settings.game_dir = dir.display().to_string();
        }
    }

    /// Opens the folder dialog on its own thread.
    pub fn pick_game_folder(&mut self) {
        self.open_dialog(Pick::GameFolder);
    }

    /// Opens the file dialog for the xg_xs.dll.
    pub fn pick_xg_library(&mut self) {
        self.open_dialog(Pick::XgLibrary);
    }

    fn open_dialog(&mut self, kind: Pick) {
        if self.pick_rx.is_some() {
            return;
        }
        let start = match kind {
            Pick::GameFolder => self.settings.game_path(),
            Pick::XgLibrary => PathBuf::from(self.settings.xg_dll.trim())
                .parent()
                .map(PathBuf::from),
        };
        self.pick_rx = Some(picker::open(kind, start));
    }

    /// Uses `dir` as the game folder when it is one (error toast otherwise), then checks it.
    pub fn set_game_folder(&mut self, dir: &std::path::Path) {
        match picker::validate_game_folder(dir) {
            Ok(()) => {
                self.settings.game_dir = dir.display().to_string();
                self.manifest = ManifestState::NotRun;
                self.notice = Some(format!("Game folder saved: {}", dir.display()));
                self.start_verify();
            }
            Err(reason) => self.notice = Some(format!("Cannot use this folder: {reason}")),
        }
    }

    /// Takes the answer of a dialog, if one came.
    fn poll_dialog(&mut self) {
        let Some(rx) = &self.pick_rx else {
            return;
        };
        let Ok((kind, chosen)) = rx.try_recv() else {
            return;
        };
        self.pick_rx = None;
        let Some(path) = chosen else {
            return;
        };
        match kind {
            Pick::GameFolder => self.set_game_folder(&path),
            Pick::XgLibrary => self.settings.xg_dll = path.display().to_string(),
        }
    }

    /// A folder (or a file of it) dropped on the window becomes the game folder.
    pub fn handle_drops(&mut self, ctx: &eframe::egui::Context) {
        let dropped = ctx.input(|i| i.raw.dropped_files.first().and_then(|f| f.path.clone()));
        if let Some(path) = dropped {
            self.set_game_folder(&picker::folder_of_drop(&path));
        }
    }

    /// The profile for this host and GPU.
    pub fn profile(&self) -> &Profile {
        self.profiles.select(self.host, self.gpu)
    }

    /// Reads the checklist again (after the user installed something).
    pub fn refresh_setup(&mut self) {
        self.setup = read_setup(self.adapters.as_deref());
    }

    /// The repository folder, or an empty path when none was found (the plan is then only shown).
    pub fn repo_or_empty(&self) -> PathBuf {
        self.repo.clone().unwrap_or_default()
    }

    /// Where VIctory keeps saves and data: `VICTORY_DATA_DIR`, else `%LOCALAPPDATA%\victory\data`.
    pub fn data_dir_label(&self) -> String {
        let data = std::env::var_os("VICTORY_DATA_DIR")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("LOCALAPPDATA")
                    .map(|l| PathBuf::from(l).join("victory").join("data"))
            });
        data.map_or("unknown".to_string(), |d| d.display().to_string())
    }

    pub fn is_busy(&self) -> bool {
        self.running.is_some() || matches!(self.verify, Verify::Running)
    }

    /// The last progress report of the translate, for the bar.
    pub fn progress(&self) -> Option<&Progress> {
        self.summary.last_progress.as_ref()
    }

    /// Starts the play steps, or the shader preloading alone; refused while a game runs.
    pub fn start(&mut self, precompile_only: bool) {
        if self.is_busy() {
            return;
        }
        if let Err(error) = session::guard(&self.settings, &Tasklist) {
            self.notice = Some(error.to_string());
            return;
        }
        let profile = self.profile().clone();
        let Some(repo) = self.repo.clone() else {
            self.notice = Some(
                "AnyXboxX folder not found: run the launcher from the repository (target\\release).".to_string(),
            );
            return;
        };
        let plan = match session::make_plan(&self.settings, &repo, &profile) {
            Ok(plan) => plan,
            Err(error) => {
                self.notice = Some(error.to_string());
                return;
            }
        };
        let steps: Result<Vec<Step>, anyxboxx::run::RunError> = if precompile_only {
            launch::precompile_steps(&plan)
        } else {
            launch::play_steps(&plan, self.settings.options.warm_first)
        };
        match steps {
            Ok(steps) => self.spawn_steps(steps, precompile_only),
            Err(error) => self.notice = Some(error.to_string()),
        }
    }

    fn spawn_steps(&mut self, steps: Vec<Step>, shaders_only: bool) {
        let (tx, rx) = mpsc::channel();
        job::spawn(steps, tx);
        self.running = Some(Running {
            events: rx,
            shaders_only,
        });
        self.step = None;
        self.summary = Summary::default();
        self.notice = None;
    }

    /// Cleans the shader cache after the user confirmed.
    pub fn clean_cache(&mut self) {
        self.confirm_clean = false;
        let Some(dir) = self.cache_dir.clone() else {
            self.notice = Some("No cache folder: LOCALAPPDATA is not set.".to_string());
            return;
        };
        self.notice = Some(match cache::clean_cache(&dir) {
            Ok(cache::CleanOutcome::Removed) => format!("Shader cache removed: {}", dir.display()),
            Ok(cache::CleanOutcome::NothingToClean) => "No shader cache to remove.".to_string(),
            Err(error) => error.to_string(),
        });
    }

    /// Drains the results of the threads started earlier.
    pub fn poll(&mut self) {
        self.poll_dialog();
        self.poll_verify();
        self.poll_manifest();
        let Some(running) = &self.running else {
            return;
        };
        let shaders_only = running.shaders_only;
        let events: Vec<Event> = running.events.try_iter().collect();
        let mut outcome = None;
        for event in events {
            match &event {
                Event::Finished(code) => outcome = Some(finish_message(*code, shaders_only)),
                Event::Failed(error) => outcome = Some(format!("Could not run: {error}")),
                Event::Detached { pid, log } => {
                    outcome = Some(format!(
                        "VIctory is running (process {pid}). Its log: {}",
                        log.display()
                    ));
                    self.close_requested = !self.settings.keep_open_after_launch;
                }
                Event::Started(_) | Event::Line(_) => {}
            }
            self.record(event);
        }
        if let Some(message) = outcome {
            self.running = None;
            self.step = None;
            self.notice = Some(message);
        }
    }

    /// Writes the settings file when something changed since the last write.
    pub fn save_settings_if_changed(&mut self) {
        let now = Stored::from_settings(&self.settings);
        if now == self.last_saved {
            return;
        }
        if let Some(path) = &self.settings_path
            && let Err(error) = store::save(path, &now)
        {
            self.notice = Some(format!("Could not save the settings: {error}"));
        }
        self.last_saved = now;
    }

    pub(crate) fn log_line(&mut self, line: String) {
        self.log.push_back(line);
        while self.log.len() > LOG_CAPACITY {
            self.log.pop_front();
        }
    }

    fn record(&mut self, event: Event) {
        let line = match event {
            Event::Started(label) => {
                let line = format!("== {label} ==");
                self.step = Some(label);
                line
            }
            Event::Line(line) => {
                self.summary = self.summary.with_line(&line);
                line
            }
            Event::Finished(code) => format!("== finished, exit code {code} =="),
            Event::Failed(error) => format!("!! {error}"),
            Event::Detached { pid, log } => format!(
                "== VIctory is running (process {pid}), log: {} ==",
                log.display()
            ),
        };
        self.log.push_back(line);
        while self.log.len() > LOG_CAPACITY {
            self.log.pop_front();
        }
    }
}

fn finish_message(code: i32, shaders_only: bool) -> String {
    match (code, shaders_only) {
        (0, true) => "Shader preloading finished.".to_string(),
        (0, false) => "VIctory exited normally.".to_string(),
        (code, _) => format!("VIctory stopped with exit code {code}: see the log."),
    }
}
