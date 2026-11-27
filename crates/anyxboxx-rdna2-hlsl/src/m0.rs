//! Propagate M0 through immediate writes and joins. Unmodelled writers invalidate the value.
use crate::control_flow::Operation;
use anyxboxx_rdna2_ref::{movrel, wave::M0};
use std::collections::BTreeMap;

pub fn known(name: &str, value: Option<u32>) -> Result<u32, String> {
    value.ok_or_else(|| format!("untranslated instruction: {name} with unknown M0"))
}

pub fn incoming(ops: &[Operation<'_>], initial: u32) -> BTreeMap<usize, Option<u32>> {
    let mut values = BTreeMap::new();
    if !ops
        .iter()
        .any(|op| movrel::is_vector(op.2) || movrel::is_scalar(op.2))
    {
        return values;
    }
    values.insert(0, Some(initial));
    let mut pending = vec![0];
    while let Some(index) = pending.pop() {
        let (inst, bytes, name, branch) = ops[index];
        let value = after(values[&inst.offset], inst.family, name, bytes);
        let next = (!matches!(name, "s_branch" | "s_endpgm") && index + 1 < ops.len())
            .then_some(index + 1);
        let target = branch.and_then(|(_, pc)| {
            ops.binary_search_by_key(&(pc as usize), |op| op.0.offset)
                .ok()
        });
        for edge in [next, target].into_iter().flatten() {
            let pc = ops[edge].0.offset;
            let joined = match values.get(&pc) {
                Some(&previous) if previous.is_none() || previous == value => continue,
                Some(_) => None,
                None => value,
            };
            values.insert(pc, joined);
            pending.push(edge);
        }
    }
    values
}

fn after(value: Option<u32>, family: &str, name: &str, bytes: &[u8]) -> Option<u32> {
    let word = u32::from_le_bytes(bytes[..4].try_into().unwrap());
    let dest = (word >> 16) & 127;
    match family {
        "SOP1" if name == "s_mov_b32" && dest == M0 => {
            let literal = bytes
                .get(4..8)
                .map(|b| u32::from_le_bytes(b.try_into().unwrap()));
            crate::unary_constant::immediate(word & 255, literal)
        }
        "SOPK" if name == "s_movk_i32" && dest == M0 => Some(word as i16 as i32 as u32),
        "SOP1" | "SOP2" | "SOPK" if dest == M0 || dest + 1 == M0 => None,
        "VOP1" if name == "v_readfirstlane_b32" && (word >> 17) & 255 == M0 => None,
        "VOPC" if word & 511 == 0xf9 => None, // SDWA can select its own scalar destination.
        "VOP1" | "VOP2" | "VOPC" | "VOP3P" | "SOP1" | "SOP2" | "SOPK" | "SOPP" | "SOPC" | "DS"
        | "MUBUF" | "MTBUF" | "MIMG" | "GLOBAL" | "FLAT" | "SCRATCH" | "EXP" | "VINTRP" => value,
        "VOP3" if movrel::is_vector(name) => value,
        "VOP3"
            if ![word & 255, (word >> 8) & 127]
                .iter()
                .any(|d| *d == M0 || d + 1 == M0) =>
        {
            value
        }
        "SMEM" if !((word >> 6 & 127)..(word >> 6 & 127) + 16).contains(&M0) => value,
        _ => None,
    }
}
