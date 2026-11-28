use std::io::{self, Read};

fn main() {
    use anyxboxx_rdna2_hlsl::{graphics, translate_wave};
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    let mut data = Vec::new();
    let result = io::stdin()
        .read_to_end(&mut data)
        .map_err(|error| error.to_string())
        .and_then(|_| match args.as_slice() {
            [] => translate_wave(&data, 32),
            [flag] if flag == "--wave64" => translate_wave(&data, 64),
            [flag] if flag == "--vertex" => {
                graphics::translate(&data, graphics::Stage::Vertex, &Default::default())
            }
            [flag, width] if flag == "--pixel" => {
                let width = width
                    .parse()
                    .map_err(|_| format!("invalid pixel width: {width}"))?;
                graphics::translate(
                    &data,
                    graphics::Stage::Pixel {
                        width,
                        multi_lane: None,
                    },
                    &Default::default(),
                )
            }
            _ => Err("usage: anyxboxx-rdna2-hlsl [--wave64 | --vertex | --pixel WIDTH]".into()),
        });
    match result {
        Ok(hlsl) => print!("{hlsl}"),
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        }
    }
}
