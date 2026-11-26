> Français : [voisins.md](../fr/recherche/voisins.md)

# Neighbouring projects: where they are ahead and how to catch up

State on 21 November 2026. AnyXboxX is GPL-3.0 or later: the code of the two projects can
be reused with credit in [THIRD_PARTY.md](../../THIRD_PARTY.md). Neither of them
starts a commercial Series X game yet (TomBox reaches the Fortnite lobby, according to their notes).
Details per project: [tombox.md](tombox.md), [xbox2windows.md](xbox2windows.md).

| Project | Licence | Size of the work | Their best lead |
| --- | --- | --- | --- |
| [omonkeyz/xbox-emulator](https://github.com/omonkeyz/xbox-emulator) (TomBox) | GPL-3.0 or later (MIT or Apache 2.0 before 22 November) | `d3d12_xs` of 32,400 lines (Rust) | D3D12.X (194/200 methods), RDNA 2 → HLSL translation, console libraries |
| [Xbox2Windows](https://github.com/R0Y00/Xbox2Windows) | MIT | about 100 ctest tests | ERA memory and GPU aliasing, CPU recompilation, DXIL retarget, GPU bench |

## What to take, in order of impact

| # | Area | Source | What we have | Issue | Effort |
| --- | --- | --- | --- | --- | --- |
| 1 | Textures: T#/S# descriptors, GFX10 formats, `image_sample` | LLVM; Mesa | BC7 and SW_4K_S in Rust | #21 | 15-25 h |
| 2 | Full D3D12.X (device, command lists, ESRAM) | TomBox `compat/d3d12_xs`, `d3d12_x` (DLL link, already in place for `d3d12_xs`) | root signature serialisation | #18, #22 | 8-15 h |
| 3 | ERA memory, CPU/GPU aliasing, title address table, `XMemVirtualQuery` | Xbox2Windows `src/era_*.cpp` | `XMemAllocDefault` without attributes | #13, #22 | 12-18 h |
| 4 | Second comparison translator | TomBox `tombox-xshader` (existing Rust bridge) | decoder bridge | #21 | 10-20 h |
| 5 | Retarget console DXIL to PC DXIL, PSO inventory | Xbox2Windows `tools/dxil_retarget.py`, `pso_readiness.py` | none | #69 | 10-16 h |
| 6 | Additional console libraries and audio (`xaudio2_9`, `xapu`, `opus`) | TomBox | `xg_xs`, `xmem`, `xma` linked | #26 | 9-18 h |
| 7 | Shared GPU bench (BVH, DXR, wave64, per-card report) | Xbox2Windows `tools/gpu-check` | `--banc`, `anyxboxx_gpu_run.exe` | #70 | 14-20 h |
| 8 | Static recompilation of CPU code (AOT) | Xbox2Windows `tools/cpu_*_aot.py` | native execution and Unicorn | #71 | 25-40 h |

Item 8 is only useful if native execution fails, since the console's processor has the
same instruction set as the PC.

## TomBox on 22 November 2026 (commit `51f5319`)

The `third_party/tombox` submodule moves from `838f43f` to `51f5319` (15 commits). AnyXboxX's Rust
workspace and the 7 DLLs served by `ANYXBOXX_TOMBOX_DIR` (`tools/build_tombox.py`) build
without changes.

| Area | Their progress | For AnyXboxX |
| --- | --- | --- |
| Licence | moved from MIT or Apache 2.0 to GPL-3.0 or later; `xma` stays LGPL-2.1 or later | compatible with AnyXboxX's GPL-3.0; notices updated |
| `xg_x`, `xg_xs` | texture layout rewritten from the console model (0 mismatches on 3 builds), `XGEncodeDCC` exact on 1,000 cases | picked up by the built DLLs (`xg_xs`) |
| GDK libraries | 37 defects fixed (`XTaskQueue` shutdown, `XVersion` order, GDK error codes, D3D12 device wrapper, `xmem`); 34 fixed return values identical to the console | picked up for `xmem`, `d3d12_xs`, `d3d12_x` |
| `d3d12_xs` | ABI per console OS version | picked up by the built DLL |
| Sandbox | fix for a drive-letter escape in `fs::resolve` (50,000 fuzz cases) | not used: AnyXboxX has its own launcher |
| Console debugging tools (`xbtp`, `xtf*`) | sessions and clients reimplemented | out of scope for now |

## Xbox2Windows on 22 November 2026 (commit `0f8e7fa`)

Our copy of `third_party/xbox2windows/` moves from `bc24dfd7eb` to `0f8e7fa` (338 commits on their side,
5 on the retarget). The retarget now re-signs the rewritten DXIL container and refuses VBR operands
wider than 64 bits; three generic modules are added (`call_evidence.py`, `probe_roots.py`,
`pe_inventory.py`) with `tests/shader_probe_check.py`.

| Area | Their progress | For AnyXboxX |
| --- | --- | --- |
| Pixel shaders (AOT to DXIL) | 851 of the game's 12,101 pixel shaders compiled and identical on RTX 4060 and WARP; DPP16/DPP8, SDWA, conversions, `v_sin`/`v_cos` | compare our HLSL translations with their cases (#21) |
| Compute and mesh | 12 exact compute shaders; one indexed mesh shader rendered like the CPU reference | to take into `--banc` (#70) |
| RDNA 2 decoding | VOP3P decoded, fixes for EXP, VINTRP, SMEM, MIMG and literals | to check against our disassembler (LLVM oracle, rule 4) |
| CPU recompilation | FMA3, AES-NI, PCLMULQDQ, SSE4.1, AVX2, x87 lowered to C++ | only useful if native execution fails (#71) |

## Update of the evening of 22 November

- **TomBox rewrote its history**: `51f5319`, pinned in the morning, no longer has a common ancestor with
  their `main` and could disappear from GitHub. The submodule moves to `99f48e3`, which takes in the
  contributor PRs (`d3d12_xs`: no more false cached "Xbox One" answer before
  `d3d12_x.dll` loads, Direct queues recorded; gfx7 MTBUF formats translated to unified RDNA 2 formats; credits gathered in `THIRD_PARTY/`). The Rust workspace, the 7 DLLs and the
  test suite pass without changes.
- **Xbox2Windows** (17 commits): CPU recompilation (SSE2 to SSE4.2, x87), DPP and SDWA in their
  pixel shader recompiler; nothing new to copy for AnyXboxX.

## Update of 22 November, 6 pm

- **TomBox** `99f48e3` → `dbe89c8` (61 commits, history intact): `d3d12_xs` (DRED diagnostics,
  hang capture, closing the window stops the game, optional protected texture reads),
  `tombox-xshader` (image atomics with `glc`, bit-exact f16 transcendentals, viewport
  index). Rust, the 7 DLLs and the test suite pass without changes.
- **Xbox2Windows** `0f8e7fa` → `a97ba9f`: the DXIL retarget reports `xbox.*` intrinsics.

## Update of 22 November, 8 pm

- **TomBox** `dbe89c8` → `a541bbb` (history intact): mesh shaders with 256 threads and per-primitive outputs,
  root signature constants on user SGPRs (Scarlett), offline network view.
- **Xbox2Windows** `a97ba9f` → `ff7f68a`: DXIL retarget and its dependencies copied.
- Check: 51 of the 52 shaders of `XboxBvhBuilder.dll` translated, as before (measured with a third-party translator and a script of ours, both since removed).

## Update of 22 November, 11 pm, and automatic tracking

- **Automatic tracking**: the `.github/workflows/voisins.yml` workflow runs `tools/sync_voisins.py` every
  hour. It advances the TomBox submodule, copies without modification the files of
  `third_party/xbox2windows` (and the modules they start importing), changes the commit numbers, then
  force-pushes only the `auto/voisins` branch, which CI tests. It does not open a pull request (the setting
  that would allow Actions to create and approve PRs stays disabled): a maintainer reviews the branch, completes
  it if needed on a `chore/voisins-<date>` branch and opens the PR. It merges nothing and stops
  if a licence changes or if a copied file disappears. Commits that touch hand-ported code
  (BC7, tiles, `era_heap`, GFX10 descriptors) are only flagged in the report of the run.
- **TomBox** `a541bbb` → `30a1b53`: Scarlett PSO stream (sub-objects 23 to 30 renumbered), protection against
  a removed device, PSO warm-up, race fixed in the `xmem` sweep, XVM audio service.
- **Xbox2Windows** `ff7f68a` → `6f48648`: the DXIL retarget refuses unresolved `xbox.*` calls before a PSO;
  new copied dependency, `tools/xbox_intrinsic_gate.py`.

## What is not taken

Everything specific to one game: archive and resource formats (RPF, AWC, TTPF, XBCH,
FSSM), file names, hashes, shader inventories, online modules of specific games,
Bink and FFX cases of Xbox2Windows. AnyXboxX stays a generic platform (rule 1 of AGENTS.md).

## First launches of the 1.0.2.1 executable (23 November)

First launches of the 1.0.2.1 executable under a third-party runtime (since taken down) (stops measured in order and the resulting work list for AnyXboxX): [first-boot-1021.md](first-boot-1021.md).

## Licences changed upstream (23 November, afternoon)

The workflow had refused two updates because a licence file changed. Reviewed by hand, they are compatible with GPL 3.0 or later:

- **TomBox** `30a1b53` → `f89e410`: `THIRD_PARTY/LICENSE-TuxBox-APACHE-2.0.txt` (Apache 2.0, fixes by brcly/TuxBox taken with credit) and `crates/tombox-xshader/src/trans32/LICENSE-rocjitsu.txt` (MIT, AMD, models of `v_log/exp/sin/cos_f32`). Apache 2.0 and MIT are compatible with GPL 3.0; TomBox's own text stays GPL-3.0 or later. The submodule advances without modification.
- **Xbox2Windows** `6f48648` → `708f4ab`: `tools/_bcpack_rans/LICENSE-MIT` and `LICENSE-APACHE` (bcpack-rs, MIT or Apache 2.0, plus Microsoft's silicon-rans, MIT). This folder is not copied on our side and no copied file imports it; the repository licence (`LICENSE`, MIT) is unchanged. The five copied files that changed (`dxil_retarget.py`, `pe_inventory.py`, `probe_roots.py`, `xbox_intrinsic_gate.py`, `dxil_retarget_check.py`) are copied without modification.

## New neighbours since 24 November

VIctory, xcl and five-star-recomp, with our measurements of VIctory on 2026-11-24 (menus at 30 fps, 351 untranslated programs in the world, 119 compute shaders refused for wave64): [victory-xcl-fivestar.md](victory-xcl-fivestar.md).
