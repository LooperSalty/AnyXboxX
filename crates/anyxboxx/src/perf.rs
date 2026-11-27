//! Native frame counter capture and the 60 fps verdict of emu/perf.py.
use anyxboxx_report::text::write_text;
use std::{
    error::Error,
    path::PathBuf,
    process::{Command, Stdio},
    time::Duration,
};

#[derive(clap::Args)]
pub struct Args {
    /// Trusted native executable to measure.
    target: PathBuf,
    /// Capture duration in seconds.
    #[arg(default_value_t = 10.0, allow_negative_numbers = true)]
    seconds: f64,
    /// Optional SVG frame-rate chart.
    chart: Option<PathBuf>,
}

struct Second {
    seconds: f64,
    fps: f64,
    p99_ms: f64,
}
const TARGET: f64 = 60.0;
const TOLERANCE: f64 = 0.5;

fn parse(csv: &str) -> Result<Vec<Second>, Box<dyn Error>> {
    // The runtime writes an unquoted numeric CSV. Address columns by name, like DictReader.
    let mut lines = csv.lines().filter(|s| !s.trim().is_empty());
    let Some(header) = lines.next() else {
        return Ok(Vec::new());
    };
    let headers: Vec<_> = header.split(',').collect();
    let index = |name: &str| {
        headers
            .iter()
            .position(|&h| h == name)
            .ok_or_else(|| format!("Colonne CSV absente : {name}"))
    };
    let (sec, frames, fps, avg, p99) = (
        index("secondes")?,
        index("images")?,
        index("fps")?,
        index("ms_moyen")?,
        index("ms_p99")?,
    );
    lines
        .map(|line| {
            let cells: Vec<_> = line.split(',').map(str::trim).collect();
            let get = |i: usize| cells.get(i).copied().ok_or("Ligne CSV incomplète");
            get(frames)?.parse::<u64>()?;
            get(avg)?.parse::<f64>()?;
            Ok(Second {
                seconds: get(sec)?.parse()?,
                fps: get(fps)?.parse()?,
                p99_ms: get(p99)?.parse()?,
            })
        })
        .collect()
}

fn svg(rows: &[Second], title: &str, constant: bool) -> Result<String, Box<dyn Error>> {
    let top = rows.iter().map(|r| r.fps).fold(TARGET * 1.25, f64::max);
    let span = rows
        .iter()
        .map(|r| r.seconds)
        .reduce(f64::max)
        .unwrap_or(1.0);
    if span == 0.0 {
        return Err("Durée CSV nulle".into());
    }
    let y = |fps: f64| 252.0 - 204.0 * fps / top;
    let points = rows
        .iter()
        .map(|r| format!("{:.1},{:.1}", 48.0 + 664.0 * r.seconds / span, y(r.fps)))
        .collect::<Vec<_>>()
        .join(" ");
    let color = if constant { "#2ea043" } else { "#e8890c" };
    let title = title
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    Ok(format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="760" height="300" viewBox="0 0 760 300" font-family="Segoe UI, Arial, sans-serif" font-size="12">
<rect width="760" height="300" fill="#f7f8fa"/>
<text x="48" y="28" font-size="16" font-weight="600" fill="#1d2230">{title}: frames per second</text>
<line x1="48" y1="{target:.1}" x2="712" y2="{target:.1}" stroke="#d0d7de" stroke-width="2" stroke-dasharray="6 4"/>
<text x="712" y="{label:.1}" text-anchor="end" fill="#1d2230">60 fps target</text>
<line x1="48" y1="252" x2="712" y2="252" stroke="#1d2230"/>
<text x="48" y="270" fill="#1d2230">0 s</text>
<text x="712" y="270" text-anchor="end" fill="#1d2230">{span:.0} s</text>
<polyline points="{points}" fill="none" stroke="{color}" stroke-width="3"/>
</svg>"##,
        target = y(TARGET),
        label = y(TARGET) - 6.0
    ))
}

pub fn run(args: &Args) -> Result<bool, Box<dyn Error>> {
    let duration =
        Duration::try_from_secs_f64(args.seconds).map_err(|_| "Durée de mesure invalide.")?;
    let dir = tempfile::tempdir()?;
    let csv = dir.path().join("fps.csv");
    let mut child = Command::new(std::env::current_exe()?)
        .arg("natif")
        .arg("--")
        .arg(&args.target)
        .env("ANYXBOXX_FPS_CSV", &csv)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    std::thread::sleep(duration);
    if child.try_wait()?.is_none() {
        child.kill()?;
    }
    child.wait()?;
    if !csv.is_file() {
        return Err("Aucune mesure : le jeu n'a créé aucune swap chain DXGI.".into());
    }
    let rows = parse(&std::fs::read_to_string(csv)?)?;
    let measured = rows
        .get(1..)
        .filter(|r| !r.is_empty())
        .ok_or("Mesure trop courte : aucune seconde après la chauffe, pas de verdict.")?;
    let count = measured.len();
    let average = average_fps(measured);
    let min = measured.iter().map(|r| r.fps).fold(f64::INFINITY, f64::min);
    let p99 = measured
        .iter()
        .map(|r| r.p99_ms)
        .fold(f64::NEG_INFINITY, f64::max);
    if p99 <= 0.0 {
        return Err("99e centile de durée d’image invalide.".into());
    }
    let constant = measured.iter().all(|r| r.fps >= TARGET - TOLERANCE);
    println!(
        "Mesure sur {count} s : moyenne {average:.1} i/s · minimum {min:.1} i/s · 1 % bas {:.1} i/s · 60 i/s constants : {}",
        1000.0 / p99,
        if constant { "oui" } else { "non" }
    );
    if let Some(chart) = &args.chart {
        write_text(
            chart,
            &svg(
                &rows,
                &args
                    .target
                    .file_stem()
                    .unwrap_or_default()
                    .to_string_lossy(),
                constant,
            )?,
        )?;
    }
    Ok(constant)
}

// Python 3.12's sum uses compensated addition. With values near a decimal tie,
// losing the low part changes the one-decimal verdict printed by the oracle.
fn average_fps(rows: &[Second]) -> f64 {
    let mut total: f64 = 0.0;
    let mut correction = 0.0;
    for row in rows {
        let next = total + row.fps;
        correction += if total.abs() >= row.fps.abs() {
            (total - next) + row.fps
        } else {
            (row.fps - next) + total
        };
        total = next;
    }
    (total + correction) / rows.len() as f64
}
