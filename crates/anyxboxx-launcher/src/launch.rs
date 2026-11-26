//! The launch steps, built on the shared pieces of `anyxboxx::run` (the same commands as
//! `anyxboxx run`): build VIctory, warm the shader cache when it is not ready (see `warm_state`),
//! then run the game. The environment is the same for the warm and the game.

use std::path::{Path, PathBuf};

use anyxboxx::run::{Env, RunError, Victory, warm_archives};

use crate::job::Step;
use crate::warm_state;

/// What a launch needs, decided by the window.
#[derive(Debug, Clone)]
pub struct Plan {
    pub repo: PathBuf,
    pub game_dir: PathBuf,
    pub env: Env,
    /// Worker threads of the warm step; `None` lets VIctory use every core.
    pub jobs: Option<usize>,
    /// The game's log file: when set, the game starts detached and writes its output there.
    pub game_log: Option<PathBuf>,
    /// The shader cache check of this launch; `None` when it could not be made.
    pub warm: Option<WarmState>,
}

/// Whether the shader cache must be warmed before the game, and where the marker goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WarmState {
    pub needed: bool,
    pub marker: Option<PathBuf>,
    pub fingerprint: String,
}

/// The steps of a launch, in order. `warm_first` also runs the shaders.rpf/exe/DLL preload.
pub fn play_steps(plan: &Plan, warm_first: bool) -> Result<Vec<Step>, RunError> {
    let victory = Victory::locate(&plan.repo, false)?;
    anyxboxx::run::game_exe(&plan.game_dir)?;
    let mut steps: Vec<Step> = build_step(&victory).into_iter().collect();
    if let Some(warm) = plan.warm.as_ref().filter(|w| w.needed) {
        steps.push(warm_step(&victory, plan, marker_of(warm))?);
    }
    if warm_first {
        steps.push(precompile_step(&victory, plan)?);
    }
    let command = victory.game_command(&plan.game_dir, &plan.env);
    steps.push(match &plan.game_log {
        Some(log) => Step::detached("Running the game", command, log.clone()),
        None => Step::new("Running the game", command),
    });
    Ok(steps)
}

/// "Warm now": the full warm of the game's archives, whatever the marker says.
pub fn precompile_steps(plan: &Plan) -> Result<Vec<Step>, RunError> {
    let victory = Victory::locate(&plan.repo, false)?;
    anyxboxx::run::game_exe(&plan.game_dir)?;
    let mut steps: Vec<Step> = build_step(&victory).into_iter().collect();
    let marker = plan.warm.as_ref().and_then(marker_of);
    steps.push(warm_step(&victory, plan, marker)?);
    Ok(steps)
}

/// The marker path and the fingerprint to write once the warm succeeded.
fn marker_of(warm: &WarmState) -> Option<(PathBuf, String)> {
    warm.marker
        .clone()
        .map(|path| (path, warm.fingerprint.clone()))
}

/// The cargo build of VIctory; `None` for a `victory.exe` built beforehand.
fn build_step(victory: &Victory) -> Option<Step> {
    victory
        .build_command()
        .map(|command| Step::new("Building VIctory", command))
}

/// `victory warm` on the game folder's own .rpf archives, with the game's environment. After a
/// success the marker gets the fingerprint, so the next launch skips the warm while nothing changed.
fn warm_step(
    victory: &Victory,
    plan: &Plan,
    marker: Option<(PathBuf, String)>,
) -> Result<Step, RunError> {
    let archives = warm_archives(&plan.game_dir)?;
    let mut command = victory.warm_command(&archives, &plan.env);
    if let Some(jobs) = plan.jobs {
        command.arg("--jobs").arg(jobs.to_string());
    }
    let step = Step::new("Warming the shader cache", command);
    Ok(match marker {
        Some((path, fingerprint)) => step.with_after(Box::new(move || {
            warm_state::write_marker(&path, &fingerprint)
                .map_err(|e| format!("could not write {}: {e}", path.display()))
        })),
        None => step,
    })
}

/// `victory warm` on shaders.rpf, the game and its DLLs (the `warm_first` option), as main's
/// `-Precompile`. It does not mark the cache as complete: the archive warm does that.
fn precompile_step(victory: &Victory, plan: &Plan) -> Result<Step, RunError> {
    let mut command = victory.precompile_command(&plan.game_dir, &plan.env)?;
    if let Some(jobs) = plan.jobs {
        command.arg("--jobs").arg(jobs.to_string());
    }
    Ok(Step::new("Preloading shaders", command))
}

/// The fingerprint of this launch: the victory.exe identity, the environment, the archives.
pub fn warm_check(
    victory_exe: &Path,
    env: &Env,
    game_dir: &Path,
    always: bool,
    marker: Option<PathBuf>,
) -> Option<WarmState> {
    let archives = warm_archives(game_dir).ok()?;
    let identities = warm_state::archive_identities(&archives);
    let fingerprint = warm_state::fingerprint(&warm_state::build_id(victory_exe), env, &identities);
    let stored = marker.as_deref().and_then(warm_state::read_marker);
    let needed = warm_state::needs_warm(always, stored.as_deref(), &fingerprint);
    Some(WarmState {
        needed,
        marker,
        fingerprint,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;
    use std::path::Path;
    use std::process::Command;

    /// The arguments a command receives, for the assertions.
    fn args_of(command: &Command) -> Vec<String> {
        command
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    fn has_victory(repo: &Path) -> bool {
        repo.join("third_party")
            .join("victory")
            .join("Cargo.toml")
            .is_file()
    }

    /// A repository with a VIctory manifest and a game folder holding the files translate needs.
    /// The temp folders are returned so they live as long as the test.
    fn fixture() -> (tempfile::TempDir, tempfile::TempDir, Plan) {
        let repo = tempfile::tempdir().expect("tempdir");
        let victory = repo.path().join("third_party").join("victory");
        std::fs::create_dir_all(&victory).expect("mkdir");
        std::fs::write(victory.join("Cargo.toml"), b"[workspace]\n").expect("write");
        let game = tempfile::tempdir().expect("tempdir");
        for name in [
            "game_scarlett_master.exe",
            "shaders.rpf",
            "XboxBvhBuilder.dll",
            "amd_fidelityfx_upscaler_xbox.dll",
        ] {
            std::fs::write(game.path().join(name), b"x").expect("write");
        }
        let plan = Plan {
            repo: repo.path().to_path_buf(),
            game_dir: game.path().to_path_buf(),
            game_log: None,
            warm: None,
            env: vec![("VICTORY_LATE_WAIT".to_string(), OsString::from("3"))],
            jobs: Some(4),
        };
        (repo, game, plan)
    }

    #[test]
    fn play_runs_build_then_game_without_precompile() {
        let (_repo, _game, plan) = fixture();
        let steps = play_steps(&plan, false).expect("steps");
        let labels: Vec<&str> = steps.iter().map(|s| s.label.as_str()).collect();
        assert_eq!(labels, vec!["Building VIctory", "Running the game"]);
        let game_args = args_of(&steps[1].command);
        assert_eq!(game_args.first().map(String::as_str), Some("run"));
        assert!(game_args.iter().any(|a| a == "--execute"));
    }

    #[test]
    fn precompile_step_passes_the_jobs_limit_after_the_inputs() {
        let (_repo, _game, plan) = fixture();
        let steps = precompile_steps(&plan).expect("steps");
        let args = args_of(&steps[1].command);
        assert_eq!(args.first().map(String::as_str), Some("warm"));
        let at = args
            .iter()
            .position(|a| a == "--jobs")
            .expect("--jobs given");
        assert_eq!(args.get(at + 1).map(String::as_str), Some("4"));
    }

    #[test]
    fn no_jobs_limit_means_no_flag() {
        let (_repo, _game, mut plan) = fixture();
        plan.jobs = None;
        let steps = precompile_steps(&plan).expect("steps");
        assert!(!args_of(&steps[1].command).iter().any(|a| a == "--jobs"));
    }

    #[test]
    fn a_missing_game_exe_refuses_the_launch() {
        let (_repo, _game, mut plan) = fixture();
        plan.game_dir = PathBuf::from("Z:/no/such/game/folder");
        assert!(play_steps(&plan, true).is_err());
    }

    #[test]
    fn a_repository_without_victory_is_an_error() {
        let (_repo, _game, mut plan) = fixture();
        let empty = tempfile::tempdir().expect("tempdir");
        plan.repo = empty.path().to_path_buf();
        assert!(!has_victory(&plan.repo));
        assert!(play_steps(&plan, false).is_err());
    }
}
