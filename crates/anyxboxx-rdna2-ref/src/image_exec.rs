//! MIMG instructions of the reference interpreter: 2D image loads and stores, image_get_resinfo, and the
//! sample, sample_c and gather4 forms. Port of `execute` and `_write_masked` in
//! `emu/rdna2_exec_image.py`; the descriptors and what is refused are in `image_desc`.
//!
//! A load or a sample writes the channels of DMASK to consecutive VGPRs from VDATA; gather4 writes four.
//! image_load gives zeros outside the image or for a mip level other than 0; image_store writes nothing
//! there. image_get_resinfo gives (width, height, 1, 1) for mip 0, else (0, 0, 0, 1).

use crate::atomic;
use crate::buffer_format::format_name;
use crate::error::Error;
use crate::image_desc::{
    self, Fields, Image, Op, SamplerAddresses, address_registers, check_instruction,
};
use crate::image_sample::{Coordinates, gather4, sample_color, sample_compare, unpack_offset};
use crate::image_texel::{store_texel, swizzled, texel};
use crate::wave::Wave;

const VECTOR_REGISTERS: usize = 256;
const IMAGE_WORDS: usize = 8;
const SAMPLER_WORDS: usize = 4;

/// True when the interpreter runs the MIMG instruction `name`.
pub fn supports(name: &str) -> bool {
    image_desc::op(name).is_some()
}

fn vgpr(wave: &Wave, index: usize, lane: usize) -> Result<u32, Error> {
    if index >= VECTOR_REGISTERS {
        return Err(Error::Operand(format!(
            "registre vectoriel hors plage : v{index}"
        )));
    }
    Ok(wave.v[index][lane])
}

fn set_vgpr(wave: &mut Wave, index: usize, lane: usize, value: u32) -> Result<(), Error> {
    if index >= VECTOR_REGISTERS {
        return Err(Error::Operand(format!(
            "registre vectoriel hors plage : v{index}"
        )));
    }
    wave.v[index][lane] = value;
    Ok(())
}

/// `count` SGPRs from `first`, refused past the last one.
fn scalars(wave: &Wave, first: usize, count: usize) -> Result<Vec<u32>, Error> {
    wave.s
        .get(first..first + count)
        .map(<[u32]>::to_vec)
        .ok_or_else(|| Error::Operand(format!("descripteur hors des SGPR : s{first}")))
}

/// Writes the channels of DMASK of `result` to the VGPRs from `vdata`.
fn write_masked(
    wave: &mut Wave,
    vdata: usize,
    lane: usize,
    dmask: u32,
    result: &[u32; 4],
) -> Result<(), Error> {
    let mut written = 0;
    for (component, value) in result.iter().enumerate() {
        if dmask >> component & 1 == 1 {
            set_vgpr(wave, vdata + written, lane, *value)?;
            written += 1;
        }
    }
    Ok(())
}

fn load(
    wave: &mut Wave,
    name: &str,
    fields: &Fields,
    image: &Image,
    addrs: &[usize],
) -> Result<(), Error> {
    for lane in wave.active()? {
        let (x, y) = (vgpr(wave, addrs[0], lane)?, vgpr(wave, addrs[1], lane)?);
        let mip = if name == "image_load_mip" {
            vgpr(wave, addrs[2], lane)?
        } else {
            0
        };
        let result = if mip != 0 || x >= image.width || y >= image.height {
            [0; 4]
        } else {
            swizzled(
                &texel(&wave.memory, image, u64::from(x), u64::from(y)),
                &image.swizzle,
            )
        };
        write_masked(wave, fields.vdata, lane, fields.dmask, &result)?;
    }
    Ok(())
}

fn store(
    wave: &mut Wave,
    name: &str,
    fields: &Fields,
    image: &Image,
    addrs: &[usize],
) -> Result<(), Error> {
    let count = fields.dmask.count_ones() as usize;
    for lane in wave.active()? {
        let (x, y) = (vgpr(wave, addrs[0], lane)?, vgpr(wave, addrs[1], lane)?);
        let mip = if name == "image_store_mip" {
            vgpr(wave, addrs[2], lane)?
        } else {
            0
        };
        if mip == 0 {
            let values = (0..count)
                .map(|i| vgpr(wave, fields.vdata + i, lane))
                .collect::<Result<Vec<_>, _>>()?;
            store_texel(&mut wave.memory, image, (x, y), fields.dmask, &values);
        }
    }
    Ok(())
}

fn resinfo(wave: &mut Wave, fields: &Fields, image: &Image, addrs: &[usize]) -> Result<(), Error> {
    for lane in wave.active()? {
        let result = if vgpr(wave, addrs[0], lane)? == 0 {
            [image.width, image.height, 1, 1]
        } else {
            [0, 0, 0, 1]
        };
        write_masked(wave, fields.vdata, lane, fields.dmask, &result)?;
    }
    Ok(())
}

fn direct(
    wave: &mut Wave,
    name: &str,
    fields: &Fields,
    image: &Image,
    addrs: &[usize],
) -> Result<(), Error> {
    match name {
        "image_load" | "image_load_mip" => load(wave, name, fields, image, addrs),
        "image_store" | "image_store_mip" => store(wave, name, fields, image, addrs),
        _ => resinfo(wave, fields, image, addrs),
    }
}

/// The coordinates of one lane of a sample or gather.
fn coordinates(
    wave: &Wave,
    regs: &SamplerAddresses,
    unnormalized: bool,
    lane: usize,
) -> Result<Coordinates, Error> {
    let offset = match regs.offset {
        Some(reg) => unpack_offset(vgpr(wave, reg, lane)?),
        None => (0, 0),
    };
    let lod = regs.lod.map(|reg| vgpr(wave, reg, lane)).transpose()?;
    Ok(Coordinates {
        s: vgpr(wave, regs.s, lane)?,
        t: vgpr(wave, regs.t, lane)?,
        unnormalized,
        offset,
        lod,
    })
}

fn sampled(
    wave: &mut Wave,
    name: &str,
    op: Op,
    fields: &Fields,
    image: &Image,
    addrs: &[usize],
) -> Result<(), Error> {
    let sampler = image_desc::sampler(&scalars(wave, fields.ssamp, SAMPLER_WORDS)?)?;
    let unnormalized = fields.unorm || sampler.unnormalized;
    let mut regs = image_desc::sampler_addresses(name, op, addrs);
    if op == Op::Gather {
        regs.lod = None; // gather4 reads the bilinear footprint of level 0: its lod is not read
    }
    for lane in wave.active()? {
        let at = coordinates(wave, &regs, unnormalized, lane)?;
        match op {
            Op::Gather => {
                let result = gather4(&wave.memory, image, &sampler, fields.dmask, &at);
                for (i, value) in result.into_iter().enumerate() {
                    set_vgpr(wave, fields.vdata + i, lane, value)?;
                }
            }
            Op::Compare => {
                let z = vgpr(wave, regs.z.unwrap_or(regs.s), lane)?;
                let result = sample_compare(&wave.memory, image, &sampler, z, &at);
                write_masked(wave, fields.vdata, lane, fields.dmask, &result)?;
            }
            _ => {
                let result = sample_color(&wave.memory, image, &sampler, &at);
                write_masked(wave, fields.vdata, lane, fields.dmask, &result)?;
            }
        }
    }
    Ok(())
}

fn atomic_op(
    wave: &mut Wave,
    name: &str,
    fields: &Fields,
    image: &Image,
    addrs: &[usize],
) -> Result<(), Error> {
    let width = image_desc::atomic_width(name, fields.dmask)?;
    if !image_desc::atomic_format_ok(image.format, width) {
        return Err(Error::Untranslated(format!(
            "{name} ({width} bits) : format {} non pris en charge",
            format_name(image.format)
        )));
    }
    let kind = name.strip_prefix("image_atomic_").unwrap_or("");
    let cmpswap = image_desc::is_cmpswap(name);
    let words_per_val = (width / 32) as usize;
    let texel_bytes = (width / 8) as usize;
    for lane in wave.active()? {
        let (x, y) = (vgpr(wave, addrs[0], lane)?, vgpr(wave, addrs[1], lane)?);
        if x >= image.width || y >= image.height {
            if fields.glc {
                for i in 0..words_per_val {
                    set_vgpr(wave, fields.vdata + i, lane, 0)?;
                }
            }
            continue;
        }
        let address = image.address.wrapping_add(
            u64::from(y)
                .wrapping_mul(u64::from(image.width))
                .wrapping_add(u64::from(x))
                .wrapping_mul(texel_bytes as u64),
        );
        let raw = wave.memory.read(address, texel_bytes);
        let mut buf = [0u8; 8];
        buf[..texel_bytes].copy_from_slice(&raw);
        let old = u64::from_le_bytes(buf);
        let (data0, data1) = if width == 32 {
            let d0 = u64::from(vgpr(wave, fields.vdata, lane)?);
            let d1 = if cmpswap {
                u64::from(vgpr(wave, fields.vdata + 1, lane)?)
            } else {
                0
            };
            (d0, d1)
        } else {
            let d0 = u64::from(vgpr(wave, fields.vdata, lane)?)
                | u64::from(vgpr(wave, fields.vdata + 1, lane)?) << 32;
            let d1 = if cmpswap {
                u64::from(vgpr(wave, fields.vdata + 2, lane)?)
                    | u64::from(vgpr(wave, fields.vdata + 3, lane)?) << 32
            } else {
                0
            };
            (d0, d1)
        };
        let new_val = atomic::apply(kind, width, old, data0, data1)
            .ok_or_else(|| Error::Untranslated(format!("atomique non pris en charge : {name}")))?;
        wave.memory
            .write(address, &new_val.to_le_bytes()[..texel_bytes]);
        if fields.glc {
            set_vgpr(wave, fields.vdata, lane, old as u32)?;
            if width == 64 {
                set_vgpr(wave, fields.vdata + 1, lane, (old >> 32) as u32)?;
            }
        }
    }
    Ok(())
}

/// Executes a MIMG instruction; `words` holds its two dwords and its NSA words.
pub fn execute(wave: &mut Wave, name: &str, words: &[u32]) -> Result<(), Error> {
    let fields = image_desc::instruction_fields(words);
    let op = check_instruction(name, &fields)?;
    let image = image_desc::image(&scalars(wave, fields.srsrc, IMAGE_WORDS)?)?;
    let addrs = address_registers(name, op, &fields);
    if op == Op::Atomic {
        return atomic_op(wave, name, &fields, &image, &addrs);
    }
    if op == Op::Direct {
        return direct(wave, name, &fields, &image, &addrs);
    }
    sampled(wave, name, op, &fields, &image, &addrs)
}
