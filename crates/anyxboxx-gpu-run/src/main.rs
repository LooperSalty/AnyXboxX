// The C++ runner uses the Windows CRT in text mode, including CRLF on pipes.
macro_rules! println {
    ($($arg:tt)*) => { std::print!("{}\r\n", format_args!($($arg)*)) };
}
macro_rules! eprintln {
    ($($arg:tt)*) => { std::eprint!("{}\r\n", format_args!($($arg)*)) };
}

mod adapter;
mod gpu;

use std::{ffi::OsString, fs, path::Path};

fn fail(message: &str) -> i32 {
    eprintln!("[gpu] {message} (0x80004005)");
    1
}
fn number(value: &OsString) -> u32 {
    let text = value.to_string_lossy();
    let text = text.trim_start();
    let (negative, digits) = match text.as_bytes().first() {
        Some(b'-') => (true, &text[1..]),
        Some(b'+') => (false, &text[1..]),
        _ => (false, text),
    };
    let n = digits
        .bytes()
        .take_while(u8::is_ascii_digit)
        .fold(0u32, |n, c| {
            n.wrapping_mul(10).wrapping_add(u32::from(c - b'0'))
        });
    if negative { n.wrapping_neg() } else { n }
}
fn read(path: &OsString) -> std::io::Result<Vec<u8>> {
    fs::read(Path::new(path))
}

fn run(args: &[OsString], draw: bool) -> i32 {
    if draw && args.len() < 8 {
        return fail("--draw <vs.dxil> <ps.dxil|-> <mémoire.bin> <sommets> <largeur> <hauteur>");
    }
    let memory_index = if draw { 4 } else { 2 };
    let shader = read(&args[if draw { 2 } else { 1 }]);
    let pixel = draw && args[3] != "-";
    let ps = if pixel {
        read(&args[3])
    } else {
        Ok(Vec::new())
    };
    let memory = read(&args[memory_index]);
    let (Ok(shader), Ok(ps), Ok(mut memory)) = (shader, ps, memory) else {
        return fail("lecture des fichiers impossible");
    };
    let original = memory.len();
    memory.resize(original.div_ceil(4) * 4 + 4, 0);
    let spec = if draw {
        Some(gpu::Draw {
            vertices: number(&args[5]),
            width: number(&args[6]),
            height: number(&args[7]),
            pixel,
        })
    } else {
        None
    };
    if spec
        .as_ref()
        .is_some_and(|d| d.vertices == 0 || d.width == 0 || d.height == 0)
    {
        return fail("sommets, largeur et hauteur > 0");
    }
    let groups = if !draw && args.len() > 3 {
        number(&args[3])
    } else {
        1
    };
    let Ok((device, kind)) = adapter::create() else {
        return fail("aucun périphérique D3D12");
    };
    let pipeline = gpu::root(&device).and_then(|root| {
        let pipeline = if draw {
            gpu::graphics(&device, &root, &shader, &ps)
        } else {
            gpu::compute(&device, &root, &shader)
        }?;
        Ok((root, pipeline))
    });
    let Ok((root, pipeline)) = pipeline else {
        return fail(if draw {
            "VS/PS DXIL refusés (signature ou format)"
        } else {
            "shader DXIL refusé (signature ou format)"
        });
    };
    if gpu::execute(
        &device,
        &root,
        &pipeline,
        &mut memory,
        groups,
        spec.as_ref(),
    )
    .is_err()
    {
        return fail("exécution GPU impossible");
    }
    if fs::write(Path::new(&args[memory_index]), &memory[..original]).is_err() {
        return fail("écriture du résultat impossible");
    }
    if let Some(d) = spec {
        println!(
            "[gpu] {kind} : {} sommet(s) dessiné(s) sur {}x{}",
            d.vertices, d.width, d.height
        );
    } else {
        println!("[gpu] {kind} : {groups} groupe(s) exécuté(s)");
    }
    0
}
// The C++ usage message goes through fwprintf, which uses the Windows ANSI code page.
fn usage() {
    use std::io::Write;
    use windows::Win32::Globalization::{CP_ACP, WideCharToMultiByte};
    let message = "Usage : anyxboxx_gpu_run.exe <shader.dxil> <m\u{e9}moire.bin> [groupes]\r\n        anyxboxx_gpu_run.exe --draw <vs.dxil> <ps.dxil|-> <m\u{e9}moire.bin> <sommets> <largeur> <hauteur>\r\n        anyxboxx_gpu_run.exe --adaptateur\r\n";
    let wide: Vec<u16> = message.encode_utf16().collect();
    unsafe {
        let size = WideCharToMultiByte(CP_ACP, 0, &wide, None, None, None);
        let mut bytes = vec![0; size as usize];
        WideCharToMultiByte(CP_ACP, 0, &wide, Some(&mut bytes), None, None);
        let _ = std::io::stderr().write_all(&bytes);
    }
}

fn main() {
    let args: Vec<_> = std::env::args_os().collect();
    let status = if args.get(1).is_some_and(|s| s == "--adaptateur") {
        if adapter::print_adapter().is_ok() {
            0
        } else {
            1
        }
    } else if args.get(1).is_some_and(|s| s == "--draw") {
        run(&args, true)
    } else if args.len() < 3 {
        usage();
        2
    } else {
        run(&args, false)
    };
    std::process::exit(status);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_accept_the_same_prefix_as_wtoi() {
        for (input, expected) in [
            ("  +12xyz", 12),
            ("-2tail", u32::MAX - 1),
            ("no digits", 0),
            ("", 0),
        ] {
            assert_eq!(number(&input.into()), expected);
        }
    }
}
