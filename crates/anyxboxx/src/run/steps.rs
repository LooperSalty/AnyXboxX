//! The steps of a launch as reusable pieces: the CLI (`anyxboxx run`) chains them like main's
//! script did, the graphical launcher (`crates/anyxboxx-launcher`) drives them one by one.

use std::ffi::OsString;
use std::io;
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::Arc;

use super::identity::{BuildProfile, IdentityError, MANIFEST_PIN_ENV, VerifiedPrebuilt};
use super::{RunArgs, RunError, settings};

pub const GAME_EXE: &str = "game_scarlett_master.exe";
/// The game's two DLLs warmed with its shaders by `--precompile`.
pub const PRECOMPILED_DLLS: [&str; 2] = ["XboxBvhBuilder.dll", "amd_fidelityfx_upscaler_xbox.dll"];
pub const SHADERS: &str = "shaders.rpf";

/// Environment variables applied to the VIctory child process in order.
/// An empty value removes a VICTORY_* variable from that child.
pub type Env = Vec<(String, OsString)>;

/// Where VIctory's sources and its built `victory.exe` are. `manifest` is `None` for a
/// `victory.exe` built beforehand (`--victory-exe`): there is nothing to build.
#[derive(Debug, Clone)]
pub struct Victory {
    manifest: Option<PathBuf>,
    exe: PathBuf,
    /// Built with the diag profile and the diagnostics feature (`--diag`, `--dred`).
    diag: bool,
    prebuilt: Option<Arc<VerifiedPrebuilt>>,
}

impl Victory {
    /// VIctory in `repo`, release build by default; its target folder is
    /// `third_party\victory\target` unless `CARGO_TARGET_DIR` is set (VIctory is its own workspace).
    pub fn locate(repo: &Path, diag: bool) -> Result<Self, RunError> {
        let victory = repo.join("third_party").join("victory");
        let manifest = victory.join("Cargo.toml");
        if !manifest.is_file() {
            return Err(RunError::NoVictory);
        }
        let target = std::env::var_os("CARGO_TARGET_DIR")
            .map_or_else(|| victory.join("target"), PathBuf::from);
        let profile = if diag { "diag" } else { "release" };
        Ok(Victory {
            manifest: Some(manifest),
            exe: target.join(profile).join("victory.exe"),
            diag,
            prebuilt: None,
        })
    }

    /// Select a prebuilt using the manifest pin supplied by the launcher's caller.
    pub fn prebuilt(exe: &Path) -> Result<Self, RunError> {
        let pin = std::env::var(MANIFEST_PIN_ENV).map_err(|error| match error {
            std::env::VarError::NotPresent => IdentityError::MissingPin,
            std::env::VarError::NotUnicode(_) => IdentityError::InvalidPin,
        })?;
        Self::pinned_prebuilt(exe, &pin)
    }

    /// The pin comes from a trusted build provider independently of the executable and sidecar.
    pub fn pinned_prebuilt(exe: &Path, trusted_manifest_sha256: &str) -> Result<Self, RunError> {
        let prebuilt = VerifiedPrebuilt::open(exe, trusted_manifest_sha256)?;
        Ok(Victory {
            manifest: None,
            exe: prebuilt.path().to_path_buf(),
            diag: prebuilt.profile() == BuildProfile::Diag,
            prebuilt: Some(Arc::new(prebuilt)),
        })
    }

    pub fn exe(&self) -> &Path {
        &self.exe
    }

    /// Stable identity for cache and benchmark records of a verified prebuilt.
    pub fn manifest_sha256(&self) -> Option<&str> {
        self.prebuilt
            .as_ref()
            .map(|verified| verified.identity.manifest_sha256())
    }

    pub(super) fn check_diagnostics(&self, requested: bool) -> Result<(), RunError> {
        if requested && !self.diag {
            return Err(IdentityError::Diagnostics.into());
        }
        Ok(())
    }

    pub(super) fn record_build(&mut self, command: &mut Command) -> Result<(), RunError> {
        self.exe = build_executable(command)?;
        Ok(())
    }

    /// `cargo build` of the release profile (or the diag profile with diagnostics): does nothing
    /// when victory.exe is current. `None` for a prebuilt `victory.exe`.
    pub fn build_command(&self) -> Option<Command> {
        let manifest = self.manifest.as_ref()?;
        let mut command = Command::new("cargo");
        if self.diag {
            command.args(["build", "--profile", "diag", "-p", "victory"]);
            command.args(["--features", "diagnostics"]);
        } else {
            command.args(["build", "--release", "-p", "victory"]);
        }
        command.arg("--manifest-path").arg(manifest);
        Some(command)
    }

    /// `victory warm` with the game's environment (the cache key depends on it). On the Vice-Tay
    /// base `warm` takes no archives: it makes the host pipelines of every pipeline earlier runs
    /// noted, so the driver's cache has them before the game asks. `archives` only enter the warm
    /// fingerprint (`warm_state`), never the command line, which Vice-Tay refuses.
    pub fn warm_command(&self, archives: &[PathBuf], env: &Env) -> RuntimeCommand {
        let _ = archives;
        let mut command = self.runtime_command();
        command.arg("warm");
        apply_env(&mut command.command, env);
        command
    }

    /// The warm before a first launch (`--precompile`): checks that shaders.rpf, the game and its
    /// two DLLs are there, then runs `victory warm` with the game's environment.
    pub fn precompile_command(
        &self,
        game_dir: &Path,
        env: &Env,
    ) -> Result<RuntimeCommand, RunError> {
        validate_precompile_inputs(game_dir)?;
        // Vice-Tay's `warm` reads no files: the inputs are checked so a broken game folder still
        // fails here, as before, and the command is the plain warm.
        Ok(self.warm_command(&[], env))
    }

    /// `victory run <game> --execute` with the game's environment.
    pub fn game_command(&self, game_dir: &Path, env: &Env) -> RuntimeCommand {
        let mut command = self.runtime_command();
        command
            .arg("run")
            .arg(game_dir.join(GAME_EXE))
            .arg("--execute");
        apply_env(&mut command.command, env);
        command
    }

    fn runtime_command(&self) -> RuntimeCommand {
        RuntimeCommand {
            command: Command::new(self.exe()),
            prebuilt: self.prebuilt.clone(),
        }
    }
}

/// A process command keeps a verified image and its path protected until the child is spawned.
pub struct RuntimeCommand {
    command: Command,
    prebuilt: Option<Arc<VerifiedPrebuilt>>,
}

impl RuntimeCommand {
    pub fn arg(&mut self, arg: impl AsRef<std::ffi::OsStr>) -> &mut Self {
        self.command.arg(arg);
        self
    }

    pub fn stdio(&mut self, stdin: Stdio, stdout: Stdio, stderr: Stdio) {
        self.command.stdin(stdin).stdout(stdout).stderr(stderr);
    }

    #[cfg(windows)]
    pub fn creation_flags(&mut self, flags: u32) {
        use std::os::windows::process::CommandExt;

        self.command.creation_flags(flags);
    }

    pub fn identity_line(&self) -> Option<String> {
        self.prebuilt
            .as_ref()
            .map(|verified| verified.identity.log_line())
    }

    pub fn spawn(&mut self) -> io::Result<Child> {
        if let Some(verified) = &self.prebuilt {
            verified.recheck()?;
        }
        self.command.spawn()
    }

    fn status(&mut self) -> io::Result<ExitStatus> {
        self.spawn()?.wait()
    }
}

impl From<Command> for RuntimeCommand {
    fn from(command: Command) -> Self {
        Self {
            command,
            prebuilt: None,
        }
    }
}

impl Deref for RuntimeCommand {
    type Target = Command;

    fn deref(&self) -> &Command {
        &self.command
    }
}

/// Check the extra inputs required for a first launch.
pub(super) fn validate_precompile_inputs(game_dir: &Path) -> Result<(), RunError> {
    for name in std::iter::once(SHADERS).chain(PRECOMPILED_DLLS) {
        let needed = game_dir.join(name);
        if !needed.is_file() {
            return Err(RunError::PrecompileInput(needed.display().to_string()));
        }
    }
    Ok(())
}

/// The `.rpf` archives directly in `game_dir`, sorted by name (never the folder's other trees).
pub fn warm_archives(game_dir: &Path) -> Result<Vec<PathBuf>, RunError> {
    let entries = std::fs::read_dir(game_dir)
        .map_err(|e| RunError::Spawn(game_dir.display().to_string(), e))?;
    let mut archives: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file()
                && path
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("rpf"))
        })
        .collect();
    if archives.is_empty() {
        return Err(RunError::NoArchive(game_dir.display().to_string()));
    }
    archives.sort_by(|a, b| a.file_name().cmp(&b.file_name()));
    Ok(archives)
}

/// The game executable of `game_dir`, which must exist.
pub fn game_exe(game_dir: &Path) -> Result<PathBuf, RunError> {
    let exe = game_dir.join(GAME_EXE);
    if exe.is_file() {
        Ok(exe)
    } else {
        Err(RunError::NoGame(game_dir.display().to_string()))
    }
}

/// Validate XG and apply explicit overrides after profile values and dedicated switches.
pub fn environment(args: &RunArgs) -> Result<Env, RunError> {
    let mut env = settings::base(args);
    if let Some(dll) = args.xg_dll.as_deref().filter(|p| !p.as_os_str().is_empty()) {
        if !dll.is_file() {
            return Err(RunError::NoXgDll(dll.display().to_string()));
        }
        let absolute =
            std::path::absolute(dll).map_err(|e| RunError::Spawn(dll.display().to_string(), e))?;
        env.extend(settings::xg(&absolute, args.xg_mode.as_str()));
    }
    env.extend(
        args.overrides
            .iter()
            .map(|(name, value)| (name.clone(), OsString::from(value))),
    );
    Ok(env)
}

fn apply_env(command: &mut Command, env: &Env) {
    for (key, value) in env {
        if key.starts_with("VICTORY_") && value.is_empty() {
            command.env_remove(key);
        } else {
            command.env(key, value);
        }
    }
}

/// Ask Cargo for the executable path so custom targets and target directories work.
pub(super) fn build_executable(command: &mut Command) -> Result<PathBuf, RunError> {
    let output = command
        .arg("--message-format=json-render-diagnostics")
        .stderr(Stdio::inherit())
        .output()
        .map_err(|e| RunError::Spawn("cargo".into(), e))?;
    if !output.status.success() {
        return Err(RunError::BuildFailed(output.status.code().unwrap_or(1)));
    }
    let executables: Vec<_> = output
        .stdout
        .split(|&byte| byte == b'\n')
        .filter_map(|line| serde_json::from_slice::<serde_json::Value>(line).ok())
        .filter_map(|message| {
            let target = &message["target"];
            let is_binary = target["kind"]
                .as_array()
                .is_some_and(|kind| kind.iter().any(|k| k == "bin"));
            if message["reason"] != "compiler-artifact" || target["name"] != "victory" || !is_binary
            {
                return None;
            }
            message["executable"]
                .as_str()
                .filter(|path| !path.is_empty())
                .map(PathBuf::from)
        })
        .collect();
    let [executable] = executables.as_slice() else {
        return Err(RunError::BuildArtifactCount(executables.len()));
    };
    if !executable.is_file() {
        return Err(RunError::NoVictoryExe(executable.display().to_string()));
    }
    Ok(executable.clone())
}

/// Waits for the command; a process killed without exit code counts as 1, like PowerShell.
pub fn status(command: &mut RuntimeCommand, name: &str) -> Result<i32, RunError> {
    if let Some(identity) = command.identity_line() {
        println!("{identity}");
    }
    let status = command
        .status()
        .map_err(|e| RunError::Spawn(name.to_string(), e))?;
    Ok(status.code().unwrap_or(1))
}
