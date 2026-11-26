# État de la branche `rust/full`

Version anglaise : [../rust-full-status.md](../rust-full-status.md). Plan : [architecture-rust.md](architecture-rust.md).

## Base : main (VIctory Neptune)

Cette branche contient désormais `origin/main` (fusion de la PR #277 et de tout ce qui la précède) : `third_party/victory` est l'arbre Neptune de main, avec deux de nos correctifs réappliqués par-dessus (VICTORY_KEYBOARD et VICTORY_WINDOW_MODE=borderless, commit b27dbae4), puis les changements de rust/full sur xmem (EraFields) et xasync. `anyxboxx run` donne à VIctory le jeu de réglages du lanceur de main : build release, jeu Neptune (39 variables, mesuré le 25 novembre 2026), `-Quality` (`--quality`), `--override`, `--auto-a`, `--frametime-dump`, `--warm`, `--precompile`, `--diag`/`--dred`. La copie de référence du script de main est `tests/fixtures/lancer_victory_neptune.ps1` ; `tests/test_anyxboxx_run.py` et `tests/test_lanceur_victory.py` comparent les réglages Rust à elle. Le lanceur graphique démarre le jeu détaché (journal sous `%LOCALAPPDATA%\AnyXboxX\logs`), se ferme dès que le jeu tourne sauf si « Garder le lanceur ouvert » est coché, et réchauffe le cache des shaders automatiquement quand le marqueur de ce build et de cet environnement manque ou est périmé (`warm-state.txt`).

À régler après la fusion : les tests de parité `anyxboxx-rdna2-ref` (`tests/test_rdna2_ref_parity.py`, 5 tests) échouent : le modèle Python de main exécute désormais des formes que l'interpréteur Rust refuse encore (VOP3P, SDWA/DPP de VOPC, accès typés aux tampons). Elles doivent être portées dans `crates/anyxboxx-rdna2-ref` avant la fusion de cette branche.

## La tester en local (Windows)

```powershell
git fetch origin
git checkout rust/full
git submodule update --init
cargo build --release --workspace

# Ligne de commande (a remplacé tools\lancer_victory.ps1 : mêmes options, même environnement) :
.\target\release\anyxboxx.exe run --game-dir "C:\Users\ANAKIN\Downloads\GTA6.Xbox.v1.0.2.1-CYBERLEEK" --xg-dll "C:\Users\ANAKIN\Downloads\GTA6.Xbox.v1.0.2.1-CYBERLEEK\dll\xg_xs.dll" --precompile

# Lanceur graphique (double-clic sur target\release\anyxboxx-launcher.exe, ou) :
.\target\release\anyxboxx-launcher.exe
```

- `anyxboxx run` compile d'abord VIctory (`cargo build --profile diag -p victory --features diagnostics`), comme le script : le premier lancement prend plusieurs minutes. `--precompile` remplit le cache de shaders avant le jeu (environ 4,5 minutes la première fois, mesuré le 25/11/2026). Le premier lancement à froid peut planter pendant la compilation des pipelines ; relancer une fois le cache rempli.
- Les noms d'options PowerShell sont acceptés aussi (`--GameDir`, `--XgDll`, `--XgMode`, `--Precompile`, `--Dred`, `--NoTunedSettings`, `--SkipPipelines`, `--BatchCardRehash`). `anyxboxx run --help` les liste.
- Un seul jeu à la fois : fermer d'abord un jeu déjà lancé. Le lanceur graphique le vérifie lui-même.

### Linux (Proton)

La compilation native sous Linux n'est pas prise en charge : VIctory, que l'espace racine utilise via `victory-hle`, ne compile que pour Windows (`victory-gpu` utilise sans condition ses modules `#[cfg(windows)]`). Compiler les deux espaces pour Windows depuis Linux, avec un CRT et un SDK MSVC sous Linux (par exemple [xwin](https://github.com/Jake-Shadle/xwin)) et `lld-link` :

```bash
cargo build --release --workspace --target x86_64-pc-windows-msvc \
    --exclude anyxboxx-kernelx --exclude anyxboxx-d3d12-xs
(cd third_party/victory && cargo build --profile diag -p victory --features diagnostics --target x86_64-pc-windows-msvc)
```

- `kernelx` et `d3d12_xs` ont besoin du `lib.exe` de MSVC pour produire le `.exp` de leur table de redirections (`anyxboxx-forwarders::link`) ; `llvm-lib /DEF` n'écrit que le `.lib`. Pas bloquant : le jeu passe par VIctory, qui ne charge pas ces DLL.
- Il n'y a pas de cargo dans Proton : `anyxboxx.exe run` prend le VIctory compilé depuis Linux avec `--victory-exe` et saute la compilation (le dépôt n'est alors pas nécessaire). Le lancer avec le `wine` de Proton et le préfixe Wine de son choix, chemins écrits en `Z:\…` :

```bash
VKD3D_CONFIG=descriptor_heap "$PROTON/files/bin/wine" target/x86_64-pc-windows-msvc/release/anyxboxx.exe run \
    --victory-exe 'Z:\…\third_party\victory\target\x86_64-pc-windows-msvc\diag\victory.exe' \
    --game-dir 'Z:\…\votre dossier du jeu'
```

- `anyxboxx run` applique le jeu de réglages Windows (`TUNED`), pas le profil `linux-amd`, que seul le lanceur graphique lit. Wine transmet l'environnement Linux : exporter `VKD3D_CONFIG=descriptor_heap` (le tas de vues du titre dépasse le plafond par défaut de vkd3d-proton) et, pour les valeurs de `linux-amd`, ajouter `--no-tuned-settings` et les exporter soi-même.
- Le lanceur graphique n'a pas encore de `--victory-exe` : ses pages Jouer et Shaders commencent par la compilation cargo, qui échoue sous Proton.

### Lanceur graphique (`crates/anyxboxx-launcher`)

Installation et utilisation : [lanceur.md](lanceur.md). Exécutable précompilé : [releases du lanceur](https://github.com/LooperSalty/AnyXboxX/releases?q=launcher&expanded=true).

Il est fait avec egui via eframe 0.34 : du Rust pur, sans liaison C++ ni installeur. La fenêtre compte quatre pages avec des barres de progression, un journal et quelques boîtes de dialogue, qu'egui (mode immédiat) dessine en quelques centaines de lignes. Le Win32 brut aurait demandé des boucles de messages et une mise en page écrites à la main pour les mêmes pages. La logique vit dans des modules simples que la fenêtre ne fait qu'appeler : les 190 tests n'ouvrent jamais de fenêtre.

Style : un dégradé de crépuscule et des cartes translucides (`theme.rs`, calcul des couleurs testé pour le niveau AA du WCAG, y compris le texte sur le voile de l'image mélangé en lumière linéaire), une seule échelle d'espacement sur une grille de 8 points, une échelle typographique et un jeu de rayons (`tokens.rs`), et des contrôles dessinés à la main avec survol, appui et anneau de focus clavier (`widgets.rs`). Les images de l'écran d'accueil ne sont jamais versionnées : elles sont lues dans `ANYXBOXX_LAUNCHER_SPLASH` (un fichier ou un dossier), sinon dans les PNG et JPEG de `%LOCALAPPDATA%\AnyXboxX\launcher\splash\` (plusieurs images défilent en diaporama avec un fondu), sinon `%LOCALAPPDATA%\AnyXboxX\launcher\splash.png` ; sans image, le dégradé et le titre sont dessinés. La page Accueil l'affiche en grand avec un zoom lent et une parallaxe qui suit le pointeur ; quand PLAY compile VIctory ou réchauffe les shaders, un écran de lancement plein fenêtre (`ui_launch.rs`) montre les étapes (Building VIctory, Preloading shaders, Starting the game), une barre alimentée par les lignes de progression de VIctory, le temps écoulé et le temps restant (`launch_progress.rs`, testé), et Cancel, qui tue l'étape en cours (`job.rs`). Les lignes « À propos » donnent la version et le commit git (intégré par `build.rs`). `--page <home|setup|folder|shaders|play>` ouvre une page ; les builds debug acceptent aussi `--preview-launch` pour montrer l'écran de lancement sans rien exécuter.

| Page | Ce qu'elle fait | Limites |
| --- | --- | --- |
| Installation | Vérifie la chaîne Rust, `d3d12.dll`, le runtime VC++ (présent, version non vérifiée), `dxc` du SDK Windows et le GPU via DXGI. Liens vers les pages officielles seulement ; rien n'est téléchargé. | La version du pilote affiche « inconnue » : le dépôt ne fixe encore aucun minimum. |
| Fichiers | Dossier du jeu (chemin saisi) : présence, fichiers vides, bornes des sections PE (exe ou DLL tronqué), bornes de la table d'en-tête RPF7, et un manifeste SHA-256 des fichiers requis et des `.rpf` de premier niveau, calculé en local. Rien n'est copié ni envoyé. | Il n'y a pas de hachés de référence : le manifeste est un relevé, pas une preuve. Une archive coupée après des tables intactes n'est pas détectée. La disposition RPF7 n'a pas été vérifiée sur un vrai fichier. |
| Shaders | `victory translate --cache --pipelines --used-pipelines … --jobs N` sur `shaders.rpf`, l'exe et ses deux DLL seulement, avec l'environnement du jeu (la clé du cache en dépend). La progression est lue dans les lignes « N of M containers (x%) » de VIctory. Refusé pendant que `victory.exe` ou le jeu tourne. | |
| Jouer | Profil choisi selon le système et le GPU (`profiles.toml`), surcharges dans le panneau avancé, `xg_xs.dll` facultative (real ou compare), compilation puis lancement par les mêmes étapes qu'`anyxboxx run`, emplacement des sauvegardes, nettoyage du cache de shaders (demande confirmation ; seul un dossier nommé `cache` peut être supprimé). | VIctory n'écrit les i/s que dans le titre de sa fenêtre : pas de ligne d'i/s à afficher. Le chemin des sauvegardes affiché est le dossier de données de VIctory, pas le sous-dossier du titre. |

Profils (`crates/anyxboxx-launcher/profiles.toml`, un test chacun) :

- **Windows + NVIDIA :** ton jeu de réglages (les six valeurs `TUNED` d'`anyxboxx run`, sans DRED, sans relais). Mesuré : 2-3 i/s sur `main` après la PR #199, 5-7 i/s sur le fork à `80d58f8`. Mettre les interrupteurs de la PR #199 à `0` n'a pas changé le chiffre de `main` : le profil les laisse aux valeurs par défaut de VIctory.
- **Linux (Proton) + AMD :** les valeurs de l'essai de la PR #199 (RX 9070 XT, RADV, GE-Proton11-12) :
  - `SHADER_LOOP_LIMIT=256`, `LATE_WAIT=0`, `WIDE_BACK=0`, `PIPELINE_WAIT_MS=50`
  - `TEXTURE_BUDGET_MB=8192` (pour une carte de 16 Go), `TEXTURE_NO_WAIT=1`
  - `CARD_BUFFERS_MIN_MB=1`, `CARD_BUFFERS_MB=1024`
  - `VKD3D_CONFIG=descriptor_heap`
- **Windows + AMD, Intel, autres :** valeurs par défaut de VIctory, marquées « non mesuré ».

## Composant par composant

| Composant (avant) | Maintenant sur `rust/full` | Vérification de parité | Ancien code |
| --- | --- | --- | --- |
| `anyxboxx run` (PowerShell) | `anyxboxx run` (`crates/anyxboxx`) | mêmes appels, même environnement `VICTORY_*` et mêmes codes de sortie que le script, avec un faux cargo et un faux victory.exe | **supprimé** ; `tests/test_anyxboxx_run.py` garde les attentes |
| `emu/loader.py` (chargeur PE) | `crates/anyxboxx-pe` (sur `victory-pe`) | `tests/test_rust_pe_parity.py` : 13 images, même JSON et mêmes protections de pages | gardé (le bac à sable Python s'en sert encore) |
| `--rapport` (`emu/report.py`, `config.py`) | `anyxboxx rapport` (`crates/anyxboxx-report`) | `tests/test_rust_rapport_parity.py` : même texte sur 7 cibles, mêmes refus sur 5 | gardé (utilisé par la CLI Python et les outils) |
| `--traduction` | `anyxboxx traduction` | `tests/test_rust_traduction_parity.py` : même tableau, SVG identique à l'octet sur 6 configurations | gardé |
| `--carte` | `anyxboxx carte` | `tests/test_rust_carte_parity.py` : SVG et badges identiques à l'octet sur 5 cas, plus 3 tables GDK synthétiques | gardé |
| `tools/jour_j.py` | `anyxboxx jour-j` | `tests/test_rust_jour_j_parity.py` : partie statique de `rapport.md` identique à l'octet sur 5 cas | gardé : les sections de lancement Unicorn et natif sont déclarées indisponibles en Rust (D2, D4), le portage est partiel |
| `--banc` | `anyxboxx banc` | `tests/test_rust_banc_parity.py` : colonnes Module, Imports, Résolus et pied de tableau | gardé : les colonnes chargement/initialisation demandent le bac à sable Unicorn (D2) et affichent `n/d` |
| `rust/` (textures, son, passerelle RDNA 2) | `crates/` (un seul espace) | tests existants inchangés | `rust/Cargo.toml` supprimé |
| `runtime/xcrdapi` | `crates/anyxboxx-xcrdapi` | `test_xcrdapi.py`, table d'exports identique | **supprimé** |
| `runtime/kernelx` | `crates/anyxboxx-kernelx` (2 661 redirections) | `test_kernelx.py`, table d'exports identique | **supprimé** (la table de redirections est passée dans la crate) |
| `runtime/d3d12_xs` | `crates/anyxboxx-d3d12-xs` | `test_d3d12_virtual_memory.py`, `test_runtime_cpp.py`, table d'exports identique | **supprimé** |
| `runtime/tls` (`anyxboxx_tls.dll`) | `crates/anyxboxx-tls`, cdylib `no_std` | `test_native_tls.py`, `test_native_tls_callbacks.py`, table d'exports identique | **supprimé** |
| `runtime/xmem` | `crates/anyxboxx-xmem` (attributs ERA partagés avec le module xmem de VIctory) | `test_xmem_era.py`, `test_xmem_virtual.py`, `test_runtime_cpp.py`, table d'exports identique | **supprimé** |
| `runtime/xgameruntime` | `crates/anyxboxx-xgameruntime` (file de tâches = `xasync` de VIctory) | Tests du runtime et comparaison des exports avec `xgameruntime_cpp.dll` réussis (Windows, 26-11-2026) | La compilation du runtime copie la DLL Rust sous le nom `xgameruntime.dll` et conserve la DLL C++ sous `xgameruntime_cpp.dll`. La désinstallation de chunks ou UWP renvoie `E_NOTIMPL` journalisé ; celle d’un paquet journalise et renvoie `false`. La parité des autres méthodes GDK reste à vérifier. |
| `runtime/launcher` | `crates/anyxboxx-native-launcher` | `tests/test_rust_native_launcher_parity.py` lance la suite native synthétique contre les deux implémentations | gardé sous `anyxboxx_cpp.exe` comme oracle ; les cas TomBox demandent ses DLL |
| Lanceur graphique | `crates/anyxboxx-launcher` (nouveau) | 190 tests Rust | |

Comment les DLL Rust et le lanceur natif arrivent dans le dossier du runtime : `cargo build --release` les compile, puis `runtime/CMakeLists.txt` les copie de `target/release` vers `runtime/build/bin` (`rust_dlls.cmake` ; s'il en manque un, la compilation s'arrête en donnant la commande à lancer). Le lanceur natif est `runtime/build/bin/anyxboxx.exe` ; `target/release/anyxboxx.exe` est la CLI VIctory distincte. `cargo build --release` passe donc avant `cmake` (CI, AGENTS.md, README).

Constat sur la porteuse TLS : en cdylib `no_std`, son bloc TLS contient exactement la zone porteuse (262 208 octets, sans variables de thread de la std Rust). Le modèle du jeu la partage donc comme avec la DLL C++. `no_std` exige `panic = "abort"`, que Cargo n'accepte que pour tout le profil release : le profil release de l'espace racine interrompt donc le programme sur panique.

**Encore en C++ :**

- `runtime/xgameruntime` : conservé comme oracle de parité C++. XPackage est porté avec les refus de désinstallation ci-dessus ; la DLL C++ reste compilée.
- `runtime/launcher` : gardé sous `anyxboxx_cpp.exe` comme oracle de parité.
- `runtime/gpu_runner` : gardé sous `anyxboxx_gpu_run_cpp.exe` comme oracle de parité ; le GPU runner lui-même est `crates/anyxboxx-gpu-run` (`anyxboxx_gpu_run.exe`).
- `runtime/third_party/libhttpclient-task` : part avec xgameruntime.

**Encore en Python :**

- Le bac à sable Unicorn et ses fonctions HLE (abandonnés dans l'état final, D2).
- `emu/modules.py`, `emu/loader.py`, `report.py`, `status_map.py`, `translation.py`, `bench.py`, `tools/jour_j.py` : ils ont un portage Rust avec parité, mais d'autres parties Python (la CLI, la fenêtre, le bac à sable) les importent encore. Ils partiront avec le bac à sable.
- RDNA 2 (`emu/rdna2*.py`, phase 4). L'interpréteur de référence est en Rust, crate `anyxboxx-rdna2-ref` (décodeur, table de mnémoniques générée par `tools/rdna2_ref_tables.py`, état du wave, instructions scalaires, branchements, VOP1, VOP2, VOPC, et VOP3 avec sa table native, les encodages plus anciens, retenues, lignes, multiplication 64 bits, SAD, division, comparaisons 64 bits, opérations f64, movrel ; mémoire scalaire (SMEM : chargements, écritures, atomiques) sur une mémoire globale éparse, avec la directive `mem` et la sortie des pages ; SDWA et DPP sur VOP1 et VOP2 ; produits scalaires VOP3P). Aussi en Rust depuis : la LDS (lectures et écritures DS, atomiques, moitiés `_d16`, échanges entre lignes), FLAT, GLOBAL et SCRATCH, les accès MUBUF non typés, les formats typés des tampons MUBUF et MTBUF (`buffer_*_format`, `tbuffer_*_format`, avec la table des formats `format_table.rs` générée par le même outil depuis `emu/data/gfx10_formats.json`), les instructions MIMG 2D (image_load, image_store, image_get_resinfo, image_sample, image_sample_c et image_gather4 avec leurs formes _lz, _l et _o, en refusant ce que le Python refuse), les atomiques flottantes (fcmpswap, fmin, fmax et leurs formes `_x2` sur FLAT et GLOBAL ; fadd, fmin, fmax et fcmpst en f32 et f64 sur la LDS ; `atomic_float.rs`), et SDWA sur les comparaisons VOPC (`ext_compare.rs`). Deux règles des atomiques flottantes sont des hypothèses non vérifiées reprises du Python : fmin et fmax gardent l'autre valeur quand une seule est un NaN (tiré de la description de V_MIN/V_MAX, non mesuré), et le NaN que produit fadd est celui que donne l'arithmétique x86-64 du Python (le premier opérande NaN, l'ancienne valeur d'abord, rendu calme ; le NaN par défaut, signe à 1, pour deux infinis de signes opposés), une propriété de la machine du modèle plutôt qu'un résultat RDNA 2 documenté. Le modèle Python refuse lui-même, et le Rust refuse de même, avec un cas de parité pour chacun : les opérations VOP3P autres que les produits scalaires entiers (et les produits scalaires avec op_sel, neg ou clamp), les formes op_sel et clamp de la seule opération VOP3 à résultat 16 bits (`v_sat_pk_u8_i16` ; op_sel n'est pas lu sur les résultats 32 bits), DPP16 et DPP8 sur VOPC, et les formes `_d16` des accès typés aux tampons et des images. Aussi en Rust depuis : les instructions graphiques, les exports EXP (`export.rs`) et l'interpolation VINTRP (`interp.rs` ; la ligne de commande prend `primitive N` et `primitives L:P ...` et écrit des lignes `exp CIBLE LIGNE X Y Z W`), les instructions du compteur de programme (s_getpc_b64, s_setpc_b64, s_swappc_b64, s_call_b64 ; `pc.rs`) et les contrôles de cache MUBUF (buffer_gl0_inv, buffer_gl1_inv). Aucune famille d'instructions RDNA 2 de l'interpréteur n'est plus seulement en Python : un balayage de chaque encodage que le Python exécute (1 060 des 1 490 encodages de LLVM plus les cinq propres à l'ISA, opérandes à zéro, wave32 et wave64, lancé le 2026-11-25) n'en trouve aucun que le Python exécute et que le Rust refuse. Le Rust refuse aussi ce que le Python y refuse : EXP avec COMPR (exports 16 bits), les attributs VINTRP au-delà de param7 et l'opcode VINTRP 3, et les formes VOP3 de `v_interp_*`. Les NaN que donnent v_interp_p1_f32 et v_interp_p2_f32 sont, comme ceux de fadd, ceux de l'arithmétique x86-64 du Python (l'opérande NaN de gauche d'abord, rendu calme ; le NaN par défaut, signe à 1, pour ∞ × 0 ou ∞ − ∞) : une propriété de la machine du modèle, gardée comme hypothèse non vérifiée. Les règles des formats typés (arrondi UNORM et SNORM, traitement de DST_SEL, FORMAT INVALID du V# qui désactive MTBUF, restrictions en écriture) viennent des mesures de Leonida sur gfx1035 et gfx1036 (et sur une Radeon RX 6700 pour l'arrondi) ; l'auteur de Leonida les a retirées après une notification DMCA, elles attendent donc une citation de la documentation ISA d'AMD ou des tests de LLVM, ou notre propre mesure. La division utilise `num-bigint` (Rust pur, crates.io) pour l'arrondi exact. Parité avec le modèle Python : `tests/test_rdna2_ref_parity.py` (programmes fixes et 300 programmes aléatoires à graine fixe, comparés ligne à ligne). D'ici là, `anyxboxx traduction` lit la couverture RDNA 2 dans `crates/anyxboxx-report/data/rdna2-coverage.txt` ; un test échoue quand cette table s'écarte de la mesure Python, et `python tests/test_rust_traduction_parity.py` la régénère.
- `tools/*.py` et la suite pytest (l'oracle).

## Vérifications sur cette branche (25/11/2026, Windows 11, Rust 1.98.1, Python 3.12)

```
python -m pytest                       → 1251 passed, 11 skipped
python -m ruff check --select W605,F,E9 PrototypeScarlett tests tools → All checks passed!
cargo test --release --workspace       → 198 passed, 0 failed
cargo clippy --release --workspace --all-targets → 0 warnings
cargo fmt --check                      → propre
cargo test --release -p anyxboxx-xgameruntime → 65 réussis (dont la régression de course XAsync entre nettoyage et état)
cargo test -p anyxboxx-rdna2-ref      → 122 passed (décodeur, wave, mémoire, atomiques entières et flottantes, scalaires, vectoriel, SDWA/DPP, SDWA sur VOPC, comparaisons, VOP3, SAD, division, f64, movrel, produits scalaires, LDS, mémoire vectorielle, formats typés, images, exports, interpolation, compteur de programme, contrôles de cache, boucle d'exécution)
python -m pytest tests/test_rdna2_ref_parity.py → 28 passed sur 28 (programmes fixes, EXP (120 programmes à graine fixe), VINTRP (150 programmes à graine fixe), les instructions du compteur de programme, les contrôles de cache MUBUF, 300 aléatoires, tests dédiés à la division et aux f64, mémoire scalaire, LDS, FLAT/GLOBAL/SCRATCH, MUBUF, formats typés et images, atomiques flottantes, SDWA et DPP sur VOPC (200 programmes à graine fixe), et les refus communs aux deux modèles : `_d16` typés et images, VOP3P au-delà des produits scalaires, op_sel 16 bits de VOP3 ; modèle Python contre binaire Rust). `test_wide_operations_match_the_python_model` passe depuis `ea9fee97` (NaN canonique pour sqrt et rsq f64 d'entrées négatives).
```

Le 26-11-2026, `cargo build --release --workspace`, `cargo test --release --workspace`, Clippy sur la crate avec `-D warnings`, `cargo fmt --check` et Ruff ont réussi. CMake a compilé `runtime/build-ninja` avec Ninja. Les 89 tests pytest utilisent la DLL xgameruntime Rust dans un runtime mixte ; `tests/test_rust_dlls.py` compare aussi ses exports à la DLL C++. Aucun jeu n’a été lancé.

Sur la branche de livraison de xgameruntime, la compilation et les tests du workspace en release ont réussi, ainsi que la compilation CMake avec Ninja et 1 781 tests pytest avec la DLL Rust. `tests/test_anyxboxx_run.py` a été exclu car il exécute un faux `victory.exe`, interdit pour cette exécution ; ses trois tests qui vérifient uniquement les paramètres ont réussi séparément. Le runtime a été compilé directement dans `runtime/build/bin` avec Ninja et l’environnement de développement Visual Studio ; les DLL TomBox des tests proviennent d’une compilation locale existante. Le générateur CMake Visual Studio par défaut n’a pas trouvé de compilateur C++ dans cet environnement.

Le refus de désinstallation de chunks par l’oracle C++ est testé séparément. L’interface de file de tâches implémente désormais `XTaskQueueRegisterWaiter` et `XTaskQueueUnregisterWaiter` (positions 20 et 21) : un thread par inscription attend une copie du handle Windows et se réarme après la fin du rappel. La désinscription, l’arrêt et la destruction de la file annulent l’attente ; des tests unitaires couvrent les deux modes de réinitialisation et l’annulation, et le même test d’interface réussit avec Rust et C++. Son `QueryInterface` répond aussi à IUnknown et à l’IID de la file. Des tests Rust comparent les positions des méthodes C++ aux tables d’interfaces Rust, y compris la table source de la file de tâches, avec des exceptions explicites pour les deux refus de désinstallation ; ils vérifient la couverture des positions, pas l’équivalence des comportements. Le test de la file échoue bien lorsque la position d’attente est remplacée par la méthode non implémentée. `XThreadAssertNotTimeSensitive` interrompt seulement un thread sensible au temps lorsqu’un débogueur est attaché, conformément à [Microsoft Learn](https://learn.microsoft.com/en-us/gaming/gdk/docs/reference/system/xthread/functions/xthreadassertnottimesensitive) ; la décision d’interruption est couverte par un test unitaire Rust. Après ces changements, la compilation et les tests du workspace en release ont réussi, ainsi que CMake et 1 781 tests pytest, avec la même exclusion des tests du lanceur que ci-dessus.

Une différence mesurée avec l’oracle C++ : lorsqu’un rappel d’attente est déjà dans la file avant la désinscription, C++ l’exécute encore lors du dispatch, tandis que Rust le supprime. Pour chaque DLL, un moniteur de file a confirmé la mise en file du rappel avant la désinscription ; le comportement Rust suit le [contrat de désinscription de Microsoft Learn](https://learn.microsoft.com/en-us/gaming/gdk/docs/reference/system/xtaskqueue/functions/xtaskqueueunregisterwaiter), et `test_unregister_suppresses_a_queued_waiter` le vérifie par l’interface.

Les chiffres de `rdna2-ref`, 1251 tests pytest et 198 tests du workspace ci-dessus datent de la mesure précédente ; les contrôles du workspace et de xgameruntime ont été relancés comme indiqué.

Le runtime a été compilé depuis cette branche (cargo, puis CMake). Avec `ANYXBOXX_REQUIRE_RUNTIME=1`, les tests des DLL de TomBox demandent aussi `python tools/build_tombox.py`.

## Constats en chemin

- À ce commit, VIctory ne compile pas sans sa fonctionnalité `diagnostics` (`victory_gpu::queue::presented` en dépend mais sert hors d'elle). L'espace racine dépend de `victory-hle` avec cette fonctionnalité, comme VIctory est toujours compilé.
- VIctory n'écrit les i/s que dans le titre de sa fenêtre. Une ligne d'i/s sur stdout dans VIctory permettrait aux deux lanceurs de l'afficher.
- Le test `a_read_in_place_never_meets_the_gap_of_a_remapping` de VIctory (victory-hle, xmem) est instable : il échoue environ 3 fois sur 8 sur le fichier non modifié.
- Le profil Windows+NVIDIA a été corrigé après ton essai : les interrupteurs de la PR #199 à `0` n'ont rien changé (2-3 i/s sur `main`, 5-7 i/s sur le fork à `80d58f8`).
- CI sur `rust/full` (première exécution via les déclencheurs de la PR #228) : le travail pytest échouait à la copie des DLL Rust, car `file(COPY_FILE)` ne crée pas `runtime/build/bin` sur une compilation propre. Corrigé dans `7bc532ba` (`file(MAKE_DIRECTORY)` d'abord). Une exécution `tests.yml` lancée à la main sur `rust/full` passe ensuite dans les trois travaux.
- Le test xmem `graphics_blocks_get_their_own_region_flagged_by_the_query` échouait de temps en temps : les tests partagent un seul espace d'adressage, et une région libérée par un test était reprise par un autre qui tournait en même temps. Les tests xmem passent maintenant à tour de rôle sur un même verrou (`7b69e6fb`) ; 15 exécutions de suite passent.
- Le générateur de parité RDNA 2 envoyait parfois les branchements dans des mots de données, décodés en instructions mémoire (hors du modèle). Les branchements tombent désormais sur des débuts d'instruction.
- Bogue du modèle Python (non corrigé ici, à signaler dans une issue) : les formes VOP3 de `v_madmk_f32`, `v_madak_f32`, `v_fmamk_f32` et `v_fmaak_f32` (opcodes VOP3 0x120, 0x121, 0x12C, 0x12D) sans opérande littéral lèvent `TypeError` dans `emu/rdna2_exec_vop3.py` (la constante K absente arrive à l'opération comme `None`) au lieu d'être refusées ou exécutées. Le Rust lit K comme 0. Aucun des deux résultats n'est un refus, les tests de parité laissent donc ces formes de côté ; trouvé par le balayage des encodages ci-dessus.
- Bogue du modèle Python (non corrigé ici, à signaler dans une issue) : `emu/rdna2_exec_wide.py`, `_fma`, calcule `float(Fraction(a) * Fraction(b) + Fraction(c))` pour des opérandes finis, et `float()` lève `OverflowError` quand le résultat exact dépasse l'intervalle des doubles, au lieu de rendre ±infini comme le fma IEEE. Le portage Rust rend ±infini ; le test aléatoire général laisse donc `v_fma_f64` de côté et un test dédié le couvre avec des opérandes qui ne peuvent pas déborder.
