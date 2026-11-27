use std::path::PathBuf;

#[derive(clap::Args)]
pub struct Args {
    /// Trusted executable, or GDK package folder.
    pub target: PathBuf,
    /// Additional folders searched for imported DLLs, in order.
    pub dll_dirs: Vec<PathBuf>,
    /// Show frames per second in the window title.
    #[arg(long)]
    pub fps: bool,
}

#[cfg(not(windows))]
pub fn run(_: &Args) -> Result<i32, Box<dyn std::error::Error>> {
    Err("L'exécution native d'AnyXboxX demande Windows x64.".into())
}

#[cfg(windows)]
pub fn run(args: &Args) -> Result<i32, Box<dyn std::error::Error>> {
    use anyxboxx::{emulated::folders, run::find_repo};
    use anyxboxx_native_launcher::{
        execution::{LaunchOptions, launch_with},
        libraries,
    };
    use anyxboxx_report::{dll_name, is_system, resolve_target};
    use std::{
        env,
        path::{Path, absolute},
    };

    eprintln!(
        "ATTENTION : en mode --natif, le programme s'exécute directement sur ce PC, sans isolation. N'utilisez ce mode qu'avec des programmes de confiance."
    );
    let exe = absolute(resolve_target(&args.target)?)?;
    let bytes = std::fs::read(&exe)?;
    validate_image(&bytes)?;
    let folder = exe.parent().ok_or("dossier de l’exécutable introuvable")?;
    let repo = find_repo(None).unwrap_or(env::current_dir()?);
    let tombox = absolute(folders::tombox(&repo))?;
    let runtime = absolute(folders::runtime(&repo))?;
    let override_d3d = env::var_os("ANYXBOXX_D3D12_XS")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from);
    let paths = libraries::runtime_paths(&runtime, &tombox, override_d3d.as_deref(), Path::is_file);
    let search = std::iter::once(Ok(folder.to_owned()))
        .chain(args.dll_dirs.iter().map(absolute))
        .collect::<Result<Vec<_>, _>>()?;
    // Native execution owns this CLI process. Set up the environment before loading the runtime.
    unsafe {
        if args.fps {
            env::set_var("ANYXBOXX_FPS", "1");
        }
        env::set_var("ANYXBOXX_GAME_DIR", folder);
    }
    let measuring = ["ANYXBOXX_FPS", "ANYXBOXX_FPS_CSV"]
        .iter()
        .any(|k| env::var_os(k).is_some_and(|v| !v.is_empty()));
    let modules = libraries::load_runtime(&paths).map_err(|(p, e)| {
        if !p.is_file() {
            format!("Runtime C++ d'AnyXboxX introuvable ({}). Compilez-le : cmake -S runtime -B runtime/build -A x64 && cmake --build runtime/build --config Release", p.display())
        } else {
            format!("Chargement de {} impossible (erreur {}).", p.display(), e.raw_os_error().unwrap_or(0))
        }
    })?;
    let absent: Vec<_> = ["d3d12_xs.dll", "xmem.dll", "xcrdapi.dll"]
        .into_iter()
        .filter(|name| !tombox.join(name).is_file())
        .collect();
    if !absent.is_empty() {
        eprintln!(
            "DLL de TomBox absentes de {} ({}) : AnyXboxX sert ses propres DLL. Compilez-les : python tools/build_tombox.py",
            tombox.display(),
            absent.join(", ")
        );
    }
    env::set_current_dir(folder)?;
    let local_dll = |name: &std::ffi::OsStr| {
        let name = dll_name(&name.to_string_lossy());
        if is_system(&name) || Path::new(&name).file_name() != Some(name.as_ref()) {
            return None;
        }
        search
            .iter()
            .map(|dir| dir.join(&name))
            .find(|p| p.is_file())
    };
    let before_entry = |base, adopted: bool| {
        println!(
            "[natif] {} chargé à {base:#x} : exécution directe sur le processeur.",
            exe.file_name().unwrap_or_default().to_string_lossy()
        );
        if !adopted {
            println!("[natif] ligne de commande du jeu non appliquée : il verra celle du lanceur.");
        }
    };
    // SAFETY: the user requested native execution of a trusted image and its DLLs. This
    // single-threaded entry point initializes hooks once and exits with the guest's code.
    Ok(unsafe {
        launch_with(
            &bytes,
            &exe,
            &tombox,
            modules,
            measuring,
            LaunchOptions {
                local_dll: &local_dll,
                before_entry: &before_entry,
            },
        )
    }?)
}

#[cfg(windows)]
fn validate_image(bytes: &[u8]) -> Result<(), String> {
    use anyxboxx_pe::{Headers, LoadError};
    const CLR_DIRECTORY: usize = 14;
    const MAX_NATIVE_IMAGE: u32 = 512 * 1024 * 1024;
    let headers = Headers::read(bytes).map_err(|error| match error {
        LoadError::NotPe32Plus => {
            "Exécution native : seul le format PE32+ x86-64 est accepté.".into()
        }
        other => other.to_string(),
    })?;
    if !(1..=MAX_NATIVE_IMAGE).contains(&headers.size_of_image) {
        return Err("Exécution native : taille d’image invalide.".into());
    }
    if headers.has_directory(CLR_DIRECTORY) {
        return Err("Exécution native : .NET pas encore pris en charge.".into());
    }
    Ok(())
}
