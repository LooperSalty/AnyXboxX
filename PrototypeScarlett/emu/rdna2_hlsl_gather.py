"""HLSL for image_gather4 with depth compare, LOD bias or LOD clamp: the computation of emu/rdna2_exec_gather.py.

The gathers without compare call ImageGather4 (emu/rdna2_hlsl_image.py) as image_gather4 does: on a
one-level view the bias, clamp and lod dwords do not change the result. The compare forms fetch the same
2x2 footprint and compare z_ref with each texel's R on integers (denormals flushed, NaN handled apart),
so no float compare runs on the GPU. Instruction fields are checked at translation; descriptors are only
known at run time, where the interpreter refuses the cases it does not model.
"""
from emu import rdna2_exec_gather as gathers
from emu import rdna2_image as images
from emu.rdna2_hlsl_ops import Untranslatable

_COMP = ("x", "y", "z", "w")

HELPERS = f"""
bool GatherIsNan(uint bits) {{ return (bits & 0x7F800000u) == 0x7F800000u && (bits & 0x007FFFFFu) != 0u; }}

int GatherKey(uint bits) {{
  uint magnitude = (bits & 0x7F800000u) == 0u ? 0u : (bits & 0x7FFFFFFFu);
  return (bits & 0x80000000u) != 0u ? -int(magnitude) : int(magnitude);
}}

uint GatherTest(uint z_bits, uint t_bits, uint func) {{
  if (func == {images.COMPARE_NEVER}u) return 0u;
  if (func == {images.COMPARE_ALWAYS}u) return {images.ONE_BITS:#x}u;
  bool p = false;
  if (GatherIsNan(z_bits) || GatherIsNan(t_bits)) {{
    p = func == {images.COMPARE_NOTEQUAL}u;
  }} else {{
    int a = GatherKey(z_bits);
    int b = GatherKey(t_bits);
    if (func == {images.COMPARE_LESS}u) p = a < b;
    else if (func == {images.COMPARE_EQUAL}u) p = a == b;
    else if (func == {images.COMPARE_LEQUAL}u) p = a <= b;
    else if (func == {images.COMPARE_GREATER}u) p = a > b;
    else if (func == {images.COMPARE_NOTEQUAL}u) p = a != b;
    else p = a >= b;
  }}
  return p ? {images.ONE_BITS:#x}u : 0u;
}}

uint4 GatherCompare(uint4 t, uint4 q, uint z_bits, uint s_bits, uint t_bits, uint unorm_bit, int2 off) {{
  uint un = unorm_bit | ((q.x >> 15) & 1u);
  uint cx = q.x & 7u;
  uint cy = (q.x >> 3) & 7u;
  uint func = (q.x >> 12) & 7u;
  uint border = (q.w >> 30) & 3u;
  uint w = ImageWidth(t);
  uint h = ImageHeight(t);
  float fx = 0.0f;
  float fy = 0.0f;
  int2 xb = ImageBilinearAxis(s_bits, w, un, off.x, fx);
  int2 yb = ImageBilinearAxis(t_bits, h, un, off.y, fy);
  int x0 = ImageClamp(xb.x, w, cx);
  int x1 = ImageClamp(xb.y, w, cx);
  int y0 = ImageClamp(yb.x, h, cy);
  int y1 = ImageClamp(yb.y, h, cy);
  return uint4(GatherTest(z_bits, ImageFetch(t, x0, y1, border).x, func),
               GatherTest(z_bits, ImageFetch(t, x1, y1, border).x, func),
               GatherTest(z_bits, ImageFetch(t, x1, y0, border).x, func),
               GatherTest(z_bits, ImageFetch(t, x0, y0, border).x, func));
}}
"""


def supports(family, name):
    return gathers.supports(family, name)


def statements(name, word, active):
    """HLSL of one gather instruction; `word`: the whole instruction (NSA dword included)."""
    fields = images.instruction_fields(word)
    try:
        gathers.check_instruction(name, fields)
    except gathers.GatherError as error:
        raise Untranslatable(str(error)) from None
    r, q = fields["srsrc"], fields["ssamp"]
    addrs = gathers.address_registers(name, fields)
    slots = gathers.layout(name)
    off_expr = f"ImageOffset(v[{addrs[slots['offset']]}])" if slots["offset"] is not None else "int2(0, 0)"
    s_expr, t_expr = f"v[{addrs[slots['s']]}]", f"v[{addrs[slots['t']]}]"
    lines = [f"uint4 t = uint4(s[{r}], s[{r + 1}], s[{r + 2}], s[{r + 3}]);",
             f"uint4 q = uint4(s[{q}], s[{q + 1}], s[{q + 2}], s[{q + 3}]);"]
    if slots["z"] is not None:
        lines.append(f"uint4 res = GatherCompare(t, q, v[{addrs[slots['z']]}], {s_expr}, {t_expr}, "
                     f"{fields['unorm']}u, {off_expr});")
    else:
        channel = images.gather_channel(fields["dmask"])
        lines.append(f"uint4 res = ImageGather4(t, q, {s_expr}, {t_expr}, {fields['unorm']}u, {channel}u, {off_expr});")
    lines += [f"v[{fields['vdata'] + i}] = res.{_COMP[i]};" for i in range(4)]
    return [f"if ({active}) {{ " + " ".join(lines) + " }"]
