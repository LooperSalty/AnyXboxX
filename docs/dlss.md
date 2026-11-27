# DLSS Super Resolution and Frame Generation 2×

On Windows, VIctory can apply DLSS at the title's FidelityFX dispatch and generate one additional image per rendered frame on its D3D12 swap chain. Both options default to off. Frame Generation requires a compatible RTX GPU, hardware-accelerated GPU scheduling and ordered submission (`VICTORY_THIN=1` or `VICTORY_ASYNC_SUBMIT=1`).

## Install and check

Download the signed Streamline 2.14.16 production runtime from NVIDIA yourself. AnyXboxX never ships these DLLs. Keep the x64 files together outside the repository, including `sl.interposer.dll`, the common, DLSS, DLSS-G, PCL and Reflex plugins, `nvngx_dlss.dll` and `nvngx_dlssg.dll`. The Rust integration comes from VIctory-NVIDIA under the GPL. NVIDIA's binaries retain their own licence. [NVIDIA Streamline releases](https://github.com/NVIDIA-RTX/Streamline/releases)

From the repository root, after building VIctory in `third_party/victory/target-codex`, substitute your runtime folder below.

```powershell
$env:VICTORY_STREAMLINE_DIR = 'D:\NVIDIA\Streamline\bin\x64'
$victory = '.\third_party\victory\target-codex\release\victory.exe'
& $victory upscaler-check
& $victory upscaler-check --device
& $victory upscaler-check --present
```

The default folder is `%LOCALAPPDATA%\victory\nvidia-sdk\Streamline\bin\x64`, where VIctory-NVIDIA's `get-dlss` installs the runtime. VIctory verifies the interposer's Authenticode signature and its signer's NVIDIA Corporation identity offline. A missing or rejected runtime leaves the title's configured renderer active and writes the reason to the log.

The first command reports feature support. The device check runs three upscales on synthetic textures. Run the presentation check on a visible Windows desktop. For 180 presents, look for 180 with generation on, up to 360 frames shown, 180 HUD-free tags and status `0x0`. The synthetic colour tests presentation. Capturing the right draw before the title's HUD needs an in-game check.

On 27 November 2026 the RTX 4070 SUPER completed three synthetic Quality upscales from 1706×960 to 2560×1440 with Streamline 2.14.16 and NGX 310.9.16. Under the offline test session, `slShutdown` then waited in NGX telemetry. The diagnostic exits with an error after 30 seconds if shutdown stalls. The session's desktop was hidden, so the displayed-frame ratio still needs a visible desktop measurement.

## Play and compare

To open the launcher with the same VIctory executable you checked, run this from the repository root after building `anyxboxx-launcher` in `target-codex`.

```powershell
.\target-codex\release\anyxboxx-launcher.exe --page play --victory-exe $victory
```

In **Play → NVIDIA Streamline**, select the DLL folder and enable **DLSS Super Resolution** and **DLSS Frame Generation 2×**. Start at 1920×1080 in Quality mode. The launcher requests ordered submission when Frame Generation is enabled. To use the CLI with the same runtime and output, substitute your game folder below.

```powershell
anyxboxx run --game-dir 'D:\Games\MyCopy' --victory-exe $victory --override "VICTORY_STREAMLINE_DIR=$env:VICTORY_STREAMLINE_DIR" --override VICTORY_OUTPUT=1920x1080 --override VICTORY_DLSS=1 --override VICTORY_FRAMEGEN=1 --override VICTORY_THIN=1
```

Once the device is ready, VIctory uses its existing renderer-7 selector and FidelityFX scale. `VICTORY_DLSS_MODE` accepts `dlaa`, `quality` (default with DLSS), `balanced`, `performance` and `ultraperformance`. An explicit `VICTORY_FFX_SCALE` takes precedence. `VICTORY_DLSS_PRESET` selects the model (default, E, F, J, K, L or M). Frame Generation alone defaults to DLAA scale and needs the title's native FidelityFX dimensions, 1280×720 or 1920×1080 in and 1920×1080 out.

At other sizes DLSS uses a native context sized for the title's fixed FidelityFX tables. A declined dispatch uses native FidelityFX when the context matches. A size mismatch returns an error and is logged.

Compare the same camera movement with generation off and on after warming the cache. Read the per-second counts of rendered presents and frames actually shown, check for a steady HUD while panning, and test pause and resume. `VICTORY_FRAMEGEN_HUDLESS=0` disables the Series X pre-HUD copy for comparison. The shader identifiers for this draw come from VIctory-NVIDIA's observations of build 1.0.2.1. Series S tags the scene and interface planes separately.
