//! Counts of a module's instructions by kind (a diagnostic): what a
//! translated program costs before the driver's compiler sees it.
//!
//! `VICTORY_SHADER_CENSUS` (any value) turns the counting on; the counts of
//! the module a thread wrote last are read with [`take`]. The `translate`
//! command writes them as a table (see `apps/victory/src/translate.rs`).
//!
//! Past the instruction kinds, what the translation knows costs on the host
//! and the console program did not have: the group barriers it adds where
//! the two host waves of a console wave64 program meet (`added_barrier`),
//! the selects of M0-indexed register moves (`indexed_select`), 64-bit
//! results (`wide64`), loops (`loops`: branches back to a block on the
//! depth-first path from the entry, whatever the block numbers), and the program's
//! shape (`lds_bytes`, `threads`, the `wave_size` it requires, 0 for none).
//! Which form of a wave64 program was emitted is [`form`]. Counts only: no
//! code.

use super::{Cast, Module, Op, Tag, Term, TypeDef, V};
use std::cell::{Cell, RefCell};

/// The kinds, in the order of [`Census::counts`].
pub const KINDS: [&str; 33] = [
    "total",
    "arith",
    "math_call",
    "bitcast",
    "cast",
    "select",
    "guard_select",
    "phi",
    "extract",
    "buffer_load",
    "cbuffer_load",
    "texture_read",
    "lds_load",
    "buffer_store",
    "lds_store",
    "atomic",
    "handle",
    "annotate",
    "dimensions",
    "barrier",
    "wave_op",
    "io_other",
    "gep",
    "clamp",
    "loop_counter",
    "blocks",
    "added_barrier",
    "indexed_select",
    "wide64",
    "loops",
    "lds_bytes",
    "threads",
    "wave_size",
];

/// Console instructions the lowering spells out in several host operations, by class, for
/// the census line (`c_` + the class): float minimum, maximum and median (`v_max_f32`,
/// `v_min3_f32`, `v_med3_f32`...; `minmax_k` those of them with an inline or literal constant
/// operand), round to even, sine and cosine, byte permutes, packed f16 arithmetic and the
/// legacy multiplies. Measured on an RTX 4060 Laptop in loops of 8 dependent chains of one
/// of them, each took 2-6 times the GPU time of the closest HLSL.
pub const SPELLED: [&str; 7] = ["minmax", "minmax_k", "round", "trig", "perm", "pk16", "legacy"];

/// The [`SPELLED`] classes console instruction `name` counts in; `constant` when one of its
/// operands is an inline constant or a literal.
pub fn spelled(name: &str, constant: bool) -> Vec<usize> {
    let class = |s: &str| SPELLED.iter().position(|k| *k == s).expect("class");
    let base = name.strip_suffix("_e64").or(name.strip_suffix("_e32")).unwrap_or(name);
    match base {
        "v_max_f32" | "v_min_f32" | "v_max3_f32" | "v_min3_f32" | "v_med3_f32" => {
            let mut v = vec![class("minmax")];
            if constant {
                v.push(class("minmax_k"));
            }
            v
        }
        "v_rndne_f32" => vec![class("round")],
        "v_sin_f32" | "v_cos_f32" => vec![class("trig")],
        "v_perm_b32" => vec![class("perm")],
        "v_mul_legacy_f32" | "v_fma_legacy_f32" | "v_mad_legacy_f32" => vec![class("legacy")],
        n if n.starts_with("v_pk_") && n.ends_with("_f16") => vec![class("pk16")],
        _ => Vec::new(),
    }
}

/// The shape of a written program that its instructions do not show: its
/// thread group and the wave size it requires.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Shape {
    pub threads: Option<[u32; 3]>,
    pub wave_size: Option<u32>,
}

/// Instruction counts of one written module, by [`KINDS`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Census {
    pub counts: [u32; KINDS.len()],
}

impl Default for Census {
    fn default() -> Self {
        Census { counts: [0; KINDS.len()] }
    }
}

thread_local! {
    /// The modules this thread wrote since the last [`take`]: the length and
    /// a hash of each one's bitcode, with its counts (a program may be
    /// emitted in several forms before one is kept).
    static WRITTEN: RefCell<Vec<(usize, u64, Census)>> = const { RefCell::new(Vec::new()) };
    /// Counting on for this thread whatever the environment says ([`with`]).
    static FORCED: Cell<bool> = const { Cell::new(false) };
    /// The form of the program `emit` returned last on this thread
    /// ([`form`]).
    static FORM: Cell<&'static str> = const { Cell::new("") };
}

/// Whether this thread counts: [`enabled`], or inside [`with`].
pub(crate) fn counting() -> bool {
    FORCED.with(Cell::get) || enabled()
}

/// Runs `f` with counting on for this thread (tests, and tools that ask for
/// one program's counts).
pub fn with<R>(f: impl FnOnce() -> R) -> R {
    let was = FORCED.with(|c| c.replace(true));
    let r = f();
    FORCED.with(|c| c.set(was));
    r
}

/// Notes the form of the program being emitted (`emit`'s retries).
pub(crate) fn set_form(form: &'static str) {
    FORM.with(|c| c.set(form));
}

/// The form the last program this thread emitted was taken in: `as is`, or
/// the retry a console wave64 program on a host of 32-lane waves needed
/// (`without early-outs`, `local waves (APPROXIMATE)`, `loose LDS`...).
pub fn form() -> &'static str {
    FORM.with(Cell::get)
}

fn fnv(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, &b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// Whether modules are counted (`VICTORY_SHADER_CENSUS` set).
pub fn enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_SHADER_CENSUS").is_some())
}

/// The counts of the module whose bitcode the container `dxil` holds, if
/// this thread wrote it since the last call (which forgets the others).
pub fn take(dxil: &[u8]) -> Option<Census> {
    let written = WRITTEN.with(|l| std::mem::take(&mut *l.borrow_mut()));
    let at = dxil.windows(4).position(|w| w == b"BC\xC0\xDE")?;
    written
        .into_iter()
        .rev()
        .find(|(len, hash, _)| dxil.get(at..at + len).is_some_and(|b| fnv(b) == *hash))
        .map(|w| w.2)
}

/// Counts `m`, whose written form is `bitcode`, and keeps the result for
/// [`take`].
pub(crate) fn note(m: &Module, bitcode: &[u8], shape: Shape) {
    if !counting() {
        return;
    }
    let mut c = m.census();
    let k = |s: &str| KINDS.iter().position(|k| *k == s).expect("kind");
    c.counts[k("threads")] = shape.threads.map_or(0, |t| t.iter().product());
    c.counts[k("wave_size")] = shape.wave_size.unwrap_or(0);
    WRITTEN.with(|l| l.borrow_mut().push((bitcode.len(), fnv(bitcode), c)));
}

fn kind(name: &str) -> usize {
    let n = name.strip_prefix("dx.op.").unwrap_or(name);
    let n = n.split('.').next().unwrap_or(n);
    let k = |s: &str| KINDS.iter().position(|k| *k == s).expect("kind");
    match n {
        "unary" | "binary" | "tertiary" | "unaryBits" | "legacyF16ToF32" | "legacyF32ToF16"
        | "isSpecialFloat" | "dot2" | "dot3" | "dot4" | "quaternary" => k("math_call"),
        "bufferLoad" | "rawBufferLoad" => k("buffer_load"),
        "cbufferLoadLegacy" | "cbufferLoad" => k("cbuffer_load"),
        "bufferStore" | "rawBufferStore" | "textureStore" => k("buffer_store"),
        "atomicBinOp" | "atomicCompareExchange" => k("atomic"),
        "createHandle" | "createHandleFromBinding" | "createHandleFromHeap" => k("handle"),
        "annotateHandle" => k("annotate"),
        "getDimensions" => k("dimensions"),
        "barrier" => k("barrier"),
        _ if n.starts_with("wave") || n.starts_with("quad") => k("wave_op"),
        _ if n.starts_with("textureLoad")
            || n.starts_with("sample")
            || n.starts_with("textureGather")
            || n.starts_with("calculateLOD") =>
        {
            k("texture_read")
        }
        _ => k("io_other"),
    }
}

impl Module {
    /// The instructions [`Module::write`] writes, by kind.
    pub(crate) fn census(&self) -> Census {
        let k = |s: &str| KINDS.iter().position(|k| *k == s).expect("kind");
        let call_kind: Vec<usize> = self.funcs.iter().map(|f| kind(&f.name)).collect();
        let live = self.live();
        // The blocks `write` lays out: those reached from the entry.
        let mut seen = vec![false; self.blocks.len()];
        let mut stack = vec![0usize];
        if !self.blocks.is_empty() {
            seen[0] = true;
        }
        while let Some(b) = stack.pop() {
            for &s in super::successors(self.blocks[b].term).iter() {
                if !seen[s] {
                    seen[s] = true;
                    stack.push(s);
                }
            }
        }
        let mut c = Census::default();
        for (b, blk) in self.blocks.iter().enumerate() {
            if !seen[b] {
                continue;
            }
            c.counts[k("blocks")] += 1;
            let _: Option<Term> = blk.term;
            for &i in blk
                .phis
                .iter()
                .filter(|&&i| live[i as usize])
                .chain(&blk.body)
            {
                c.counts[0] += 1;
                let at = match &self.insts[i as usize].op {
                    Op::Bin(..) | Op::Cmp(..) => k("arith"),
                    Op::Cast(Cast::Bitcast, _) => k("bitcast"),
                    Op::Cast(..) => k("cast"),
                    Op::Select(..) => k("select"),
                    Op::Extract(..) => k("extract"),
                    Op::Phi(_) => k("phi"),
                    Op::Gep(..) => k("gep"),
                    Op::Load(_) => k("lds_load"),
                    Op::Store(..) => k("lds_store"),
                    Op::AtomicRmw(..) | Op::CmpXchg(..) => k("atomic"),
                    Op::Call(f, _) => call_kind[*f as usize],
                };
                c.counts[at] += 1;
                if matches!(self.types[self.insts[i as usize].ty.0 as usize], TypeDef::Int(64) | TypeDef::Double) {
                    c.counts[k("wide64")] += 1;
                }
                match self.tags.get(&i) {
                    Some(Tag::Guard) => c.counts[k("guard_select")] += 1,
                    Some(Tag::Clamp) => c.counts[k("clamp")] += 1,
                    Some(Tag::Counter) => c.counts[k("loop_counter")] += 1,
                    Some(Tag::Meeting) => c.counts[k("added_barrier")] += 1,
                    Some(Tag::Indexed) => c.counts[k("indexed_select")] += 1,
                    None => {}
                }
                let _ = V::Inst(i);
            }
        }
        // Branches back to a block still on the depth-first path from the entry. Not
        // block numbers: the lowering appends the blocks it makes (a critical edge's, a
        // guarded store's join), so their branches into the next console block go to a
        // lower number while going forward.
        c.counts[k("loops")] = self.back_edges().iter().map(|(_, from)| from.len() as u32).sum();
        c.counts[k("lds_bytes")] = self
            .globals
            .iter()
            .filter(|g| g.space == 3)
            .map(|g| self.bytes(g.ty))
            .sum();
        c
    }

    /// The size in memory of a value of type `t` (scalars and aggregates).
    fn bytes(&self, t: super::Ty) -> u32 {
        match &self.types[t.0 as usize] {
            TypeDef::Int(bits) => bits.div_ceil(8),
            TypeDef::Float => 4,
            TypeDef::Double => 8,
            TypeDef::Array(n, of) => n * self.bytes(*of),
            TypeDef::Struct(_, fields) => fields.iter().map(|f| self.bytes(*f)).sum(),
            _ => 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::{Module, Term, pred};
    use super::KINDS;

    fn loops(m: &Module) -> u32 {
        m.census().counts[KINDS.iter().position(|k| *k == "loops").expect("kind")]
    }

    /// The lowering appends the blocks it makes as it goes (a critical edge's block, the
    /// join of a guarded store): a branch from one of them into a console block made
    /// earlier goes to a lower block number but forward in the program. Only an edge
    /// to a block still on the depth-first path from the entry is a loop's.
    #[test]
    fn spelled_out_instructions_by_class() {
        let names = |n: &str, k: bool| super::spelled(n, k).into_iter().map(|c| super::SPELLED[c]).collect::<Vec<_>>();
        assert_eq!(names("v_max_f32", false), ["minmax"]);
        assert_eq!(names("v_med3_f32", true), ["minmax", "minmax_k"]);
        assert_eq!(names("v_rndne_f32", false), ["round"]);
        assert_eq!(names("v_cos_f32", false), ["trig"]);
        assert_eq!(names("v_perm_b32", true), ["perm"]);
        assert_eq!(names("v_pk_fma_f16", false), ["pk16"]);
        assert_eq!(names("v_mul_legacy_f32", false), ["legacy"]);
        assert!(names("v_pk_add_u16", false).is_empty());
        assert!(names("v_fma_f32", true).is_empty());
    }

    #[test]
    fn census_counts_loops_not_forward_edges_into_earlier_blocks() {
        // 0 -> {2, 1}, 2 -> 1, 1 -> ret: a diamond whose edge block (2) was made last.
        let mut m = Module::default();
        let (b0, b1, b2) = (m.new_block(), m.new_block(), m.new_block());
        let c = m.bool(true);
        m.terminate(b0, Term::CondBr(c, b2, b1));
        m.terminate(b2, Term::Br(b1));
        m.terminate(b1, Term::Ret);
        assert_eq!(loops(&m), 0, "a forward edge into an earlier block is not a loop");

        // 0 -> 1, 1 -> {3, 2}, 3 -> 1 (the latch, made last), 2 -> ret: one loop.
        let mut m = Module::default();
        let (b0, b1, b2, b3) = (m.new_block(), m.new_block(), m.new_block(), m.new_block());
        let (x, y) = (m.i32(1), m.i32(2));
        let c = m.cmp(b1, pred::EQ, x, y);
        m.terminate(b0, Term::Br(b1));
        m.terminate(b1, Term::CondBr(c, b3, b2));
        m.terminate(b3, Term::Br(b1));
        m.terminate(b2, Term::Ret);
        assert_eq!(loops(&m), 1, "one branch back");

        // A loop whose header comes after its latch in block order: 0 -> 2, 2 -> {1, 3},
        // 1 -> 2, 3 -> ret.
        let mut m = Module::default();
        let (b0, b1, b2, b3) = (m.new_block(), m.new_block(), m.new_block(), m.new_block());
        let (x, y) = (m.i32(1), m.i32(2));
        let c = m.cmp(b2, pred::EQ, x, y);
        m.terminate(b0, Term::Br(b2));
        m.terminate(b2, Term::CondBr(c, b1, b3));
        m.terminate(b1, Term::Br(b2));
        m.terminate(b3, Term::Ret);
        assert_eq!(loops(&m), 1, "a back edge to a later-numbered header");
    }
}
