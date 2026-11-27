"""image_gather4 with depth compare, LOD bias or LOD clamp in the RDNA 2 reference interpreter.

Covered (18 MIMG encodings): image_gather4_c, image_gather4_c_lz, image_gather4_c_l and their _o forms
(depth-compare gather, D3D GatherCmp), plus the bias and clamp forms image_gather4_cl, image_gather4_b,
image_gather4_b_cl, image_gather4_c_cl, image_gather4_c_b, image_gather4_c_b_cl and their _o forms.

Semantics:
- Addresses, in consecutive VGPRs or in the NSA word: {offset} {bias} {z-compare} {s, t} {lod or clamp}.
- The image is the supported case of emu/rdna2_image.py: a linear 2D view with a single mip level
  (BASE_LEVEL = LAST_LEVEL = 0). The LOD only picks the mip level, and every LOD of a one-level view picks
  level 0, so bias, clamp and lod are read but do not change the result (as for image_gather4_l already).
- The 2x2 footprint, the addressing modes, the border colours and the result order (x0, y1), (x1, y1),
  (x1, y0), (x0, y0) are those of image_gather4 (emu/rdna2_exec_image.py), reused as is.
- Without compare: the dmask component (one bit) of each of the 4 texels after DST_SEL, as image_gather4.
- With compare: each of the 4 results is 1.0 when `z_ref <func> texel.R` holds, else 0.0 (no filtering);
  func is the S# DEPTH_COMPARE_FUNC. Denormals of z_ref and of the texel are flushed to zero before the
  compare; a NaN compares false except with NOT_EQUAL (true), ALWAYS (true) and NEVER (false). The
  compare is done on integers, so the HLSL needs no float compare.

Refused by name (rule 3): a dmask other than 0x1 with compare (the compared component is R), a T#
DST_SEL_X other than R with compare, a T# BC_SWIZZLE other than 0 and an S# TRUNC_COORD of 1 (not
modelled), plus everything emu/rdna2_image.py refuses (dimension other than 2D, r128, tfe, lwe, a16, d16,
mip levels, formats, anisotropic filtering, other clamp modes and border colours).

Sources (labelled as in docs/research/rdna2-sources.md; printed pages of the November 2020 PDF):
- AMD, "RDNA 2" Instruction Set Architecture: section 12.16, MIMG opcodes 64-95 (p. 220-221) for the names
  and "gather 4 single component elements (2x2) with user bias / LOD clamp / PCF"; section 8.2.4, table 43
  and note 4 (p. 72-73: gather4 bodies are x, y; gather4_cl adds clamp) and table 44 (p. 73: _B adds a
  bias dword, _CL a clamp dword, _C a z-comp dword, _O an offset dword); section 8.2.5 (p. 73: address
  order { offset } { bias } { z-compare } { derivative } { body }, offsets X=[5:0], Y=[13:8]); table 45
  (p. 74-75: T# base level, last level, dst_sel, BC swizzle at bits 123:121); table 46 (p. 76: S# depth
  compare func 0-7 at bits 14:12, trunc coord at bit 27, min/max LOD "in resource view space (0.0 =
  T#.base_level)").
- Microsoft Learn, gather4_c (sm5 - asm): "returns each comparison result, rather than filtering them",
  "srcReferenceValue {ComparisonFunction} texel.R", "Denorms are flushed to zero going into the comparison".
- LLVM (cross-check), MIMGInstructions.td / AMDGPU::getAddrSizeMIMGOp: one address dword each for c, b, o
  and for cl/l, as ported in emu/rdna2_operands_mimg.py.
"""
from emu import rdna2_image as images
from emu.rdna2_exec_image import bilinear_axis, clamp_index, fetch_texel, swizzled

FAMILY = "MIMG"
GATHER = "image_gather4"
COMPARE, BIAS, OFFSET = "c", "b", "o"
LOD_WORDS = frozenset({"l", "cl"})
OPERATIONS = frozenset({
    "image_gather4_c", "image_gather4_c_lz", "image_gather4_c_l",
    "image_gather4_c_o", "image_gather4_c_lz_o", "image_gather4_c_l_o",
    "image_gather4_cl", "image_gather4_b", "image_gather4_b_cl",
    "image_gather4_cl_o", "image_gather4_b_o", "image_gather4_b_cl_o",
    "image_gather4_c_cl", "image_gather4_c_b", "image_gather4_c_b_cl",
    "image_gather4_c_cl_o", "image_gather4_c_b_o", "image_gather4_c_b_cl_o",
})
UNSUPPORTED_FLAGS = ("r128", "tfe", "lwe", "a16", "d16")
RED_ONLY = 0x1
SIGN = 0x80000000
EXPONENT = 0x7F800000
MANTISSA = 0x007FFFFF
TRUNC_COORD_BIT = 27
BC_SWIZZLE_SHIFT, BC_SWIZZLE_MASK = 25, 0x7
GATHER_ORDER = ((0, 1), (1, 1), (1, 0), (0, 0))           # (x index, y index) of results 0..3


class GatherError(ValueError):
    """Gather instruction or descriptor outside the supported case."""


def supports(family, name):
    return family == FAMILY and name in OPERATIONS


def suffixes(name):
    return frozenset(name[len(GATHER) + 1:].split("_")) if name != GATHER else frozenset()


def is_compare(name):
    return COMPARE in suffixes(name)


def layout(name):
    """Address slots: {"offset", "bias", "z", "s", "t", "lod"} → index in the address list (None if absent)."""
    words = suffixes(name)
    slots, index = {}, 0
    for key, present in (("offset", OFFSET in words), ("bias", BIAS in words), ("z", COMPARE in words),
                         ("s", True), ("t", True), ("lod", bool(words & LOD_WORDS))):
        slots[key] = index if present else None
        index += int(present)
    slots["count"] = index
    return slots


def address_registers(name, fields):
    count = layout(name)["count"]
    if fields["nsa"]:
        return tuple(fields["vaddr"][:count])
    return tuple(fields["vaddr"][0] + i for i in range(count))


def check_instruction(name, fields):
    if name not in OPERATIONS:
        raise GatherError(f"gather instruction not supported: {name}")
    if fields["dim"] != images.DIM_2D:
        raise GatherError(f"{name}: dimension {fields['dim']} not supported (2D only)")
    for flag in UNSUPPORTED_FLAGS:
        if fields[flag]:
            raise GatherError(f"{name}: option {flag} not supported")
    if bin(fields["dmask"]).count("1") != 1:
        raise GatherError(f"{name}: dmask 0x{fields['dmask']:x} not supported (exactly one component)")
    if is_compare(name) and fields["dmask"] != RED_ONLY:
        raise GatherError(f"{name}: dmask 0x{fields['dmask']:x} not supported with compare (R only, 0x1)")


def check_descriptors(name, t_words, s_words):
    """(image, sampler) dictionaries; GatherError for the descriptor cases this module does not model."""
    try:
        image = images.image(t_words)
        sampler = images.sampler(s_words)
    except images.ImageError as error:
        raise GatherError(f"{name}: {error}") from None
    if (t_words[3] >> BC_SWIZZLE_SHIFT) & BC_SWIZZLE_MASK:
        raise GatherError(f"{name}: T# BC_SWIZZLE {(t_words[3] >> BC_SWIZZLE_SHIFT) & BC_SWIZZLE_MASK} not supported")
    if s_words[0] >> TRUNC_COORD_BIT & 1:
        raise GatherError(f"{name}: S# TRUNC_COORD not supported")
    if is_compare(name) and image["swizzle"][0] != images.SELECT_X:
        raise GatherError(f"{name}: T# DST_SEL_X {image['swizzle'][0]} not supported with compare (R only)")
    return image, sampler


def compare_key(bits):
    """Integer that orders float32 `bits` as numbers once denormals are flushed (-0 = +0); None for NaN."""
    exponent = bits & EXPONENT
    if exponent == EXPONENT and bits & MANTISSA:
        return None
    magnitude = 0 if exponent == 0 else bits & ~SIGN & 0xFFFFFFFF
    return -magnitude if bits & SIGN else magnitude


def depth_test(z_bits, texel_bits, func):
    """1.0 bits if `z <func> texel` holds, else 0, compared on integers."""
    if func in (images.COMPARE_NEVER, images.COMPARE_ALWAYS):
        return images.ONE_BITS if func == images.COMPARE_ALWAYS else 0
    z, texel = compare_key(z_bits), compare_key(texel_bits)
    if z is None or texel is None:
        passed = func == images.COMPARE_NOTEQUAL
    else:
        passed = {images.COMPARE_LESS: z < texel, images.COMPARE_EQUAL: z == texel,
                  images.COMPARE_LEQUAL: z <= texel, images.COMPARE_GREATER: z > texel,
                  images.COMPARE_NOTEQUAL: z != texel, images.COMPARE_GEQUAL: z >= texel}[func]
    return images.ONE_BITS if passed else 0


def _offsets(word):
    off_x = (word & 0x3F) - (64 if word & 0x20 else 0)
    off_y = ((word >> 8) & 0x3F) - (64 if word & 0x2000 else 0)
    return off_x, off_y


def footprint(memory, image, sampler, s_bits, t_bits, unnormalized, off_x, off_y):
    """The 4 raw texels (before DST_SEL) in gather order; border texels are the border colour."""
    x0, x1, _ = bilinear_axis(s_bits, image["width"], unnormalized, off_x)
    y0, y1, _ = bilinear_axis(t_bits, image["height"], unnormalized, off_y)
    xs = (clamp_index(x0, image["width"], sampler["clamp_x"]), clamp_index(x1, image["width"], sampler["clamp_x"]))
    ys = (clamp_index(y0, image["height"], sampler["clamp_y"]), clamp_index(y1, image["height"], sampler["clamp_y"]))
    border = sampler["border_color"]
    return [fetch_texel(memory, image, xs[i], ys[j], border) for i, j in GATHER_ORDER]


def gather(name, memory, image, sampler, unnormalized, dmask, lane_words):
    """The 4 result dwords for one lane; `lane_words`: the lane's address dwords by layout slot."""
    slots = layout(name)
    off_x, off_y = _offsets(lane_words[slots["offset"]]) if slots["offset"] is not None else (0, 0)
    texels = footprint(memory, image, sampler, lane_words[slots["s"]], lane_words[slots["t"]],
                       unnormalized, off_x, off_y)
    if slots["z"] is not None:
        z_bits = lane_words[slots["z"]]
        return [depth_test(z_bits, texel[0], sampler["depth_compare"]) for texel in texels]
    channel = images.gather_channel(dmask)
    return [swizzled(texel, image["swizzle"])[channel] for texel in texels]


def execute(wave, name, word):
    """Run one gather instruction; `word`: the whole instruction (NSA dword included)."""
    from emu.rdna2_exec import Untranslated

    fields = images.instruction_fields(word)
    try:
        check_instruction(name, fields)
        image, sampler = check_descriptors(name, [wave.s[fields["srsrc"] + i] for i in range(8)],
                                           [wave.s[fields["ssamp"] + i] for i in range(4)])
    except GatherError as error:
        raise Untranslated(str(error)) from None
    addrs = address_registers(name, fields)
    unnormalized = fields["unorm"] or sampler["unnormalized"]
    vdata = fields["vdata"]
    results = {}
    for lane in wave.active():
        lane_words = [wave.v[register][lane] for register in addrs]
        results[lane] = gather(name, wave.memory, image, sampler, unnormalized, fields["dmask"], lane_words)
    for lane, values in results.items():
        for index, value in enumerate(values):
            wave.v[vdata + index][lane] = value

