#[cfg(windows)]
mod windows;

#[cfg(windows)]
fn main() {
    std::process::exit(windows::run());
}

#[cfg(not(windows))]
fn main() {
    eprintln!("[anyxboxx] le lanceur natif nécessite Windows x86-64");
    std::process::exit(1);
}
