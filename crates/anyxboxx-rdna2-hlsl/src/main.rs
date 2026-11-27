use std::io::{self, Read};

fn main() {
    let lanes = match std::env::args().skip(1).collect::<Vec<_>>().as_slice() {
        [] => 32,
        [flag] if flag == "--wave64" => 64,
        _ => {
            eprintln!("usage: anyxboxx-rdna2-hlsl [--wave64]");
            std::process::exit(2);
        }
    };
    let mut data = Vec::new();
    let result = io::stdin()
        .read_to_end(&mut data)
        .map_err(|error| error.to_string())
        .and_then(|_| anyxboxx_rdna2_hlsl::translate_wave(&data, lanes));
    match result {
        Ok(hlsl) => print!("{hlsl}"),
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        }
    }
}
