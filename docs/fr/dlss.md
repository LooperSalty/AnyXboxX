# DLSS Super Resolution et Frame Generation 2×

Sous Windows, VIctory peut appliquer DLSS à la place du traitement FidelityFX du titre et générer une image supplémentaire par image rendue sur sa chaîne de présentation D3D12. Les deux options sont désactivées par défaut. La génération demande une carte RTX compatible, la planification GPU à accélération matérielle (HAGS) et des soumissions ordonnées (`VICTORY_THIN=1` ou `VICTORY_ASYNC_SUBMIT=1`).

## Installer et vérifier

Téléchargez vous-même les DLL de production signées de NVIDIA Streamline 2.14.16. AnyXboxX ne les distribue jamais. Gardez les fichiers x64 ensemble hors du dépôt, dont `sl.interposer.dll`, les plugins common, DLSS, DLSS-G, PCL et Reflex ainsi que `nvngx_dlss.dll` et `nvngx_dlssg.dll`. L’intégration Rust vient de VIctory-NVIDIA sous GPL. Les DLL conservent la licence de NVIDIA. [Versions officielles de NVIDIA Streamline](https://github.com/NVIDIA-RTX/Streamline/releases)

Depuis la racine du dépôt, après compilation de VIctory dans `third_party/victory/target-codex`, remplacez le chemin des DLL par le vôtre.

```powershell
$env:VICTORY_STREAMLINE_DIR = 'D:\NVIDIA\Streamline\bin\x64'
$victory = '.\third_party\victory\target-codex\release\victory.exe'
& $victory upscaler-check
& $victory upscaler-check --device
& $victory upscaler-check --present
```

Le dossier par défaut est `%LOCALAPPDATA%\victory\nvidia-sdk\Streamline\bin\x64`, utilisé par la commande `get-dlss` de VIctory-NVIDIA. VIctory vérifie hors ligne la signature Authenticode de l’interposeur et l’identité de son signataire, NVIDIA Corporation. Si le chargement échoue ou si la signature est refusée, le moteur conserve le rendu configuré pour le titre et journalise la raison.

La première commande indique les fonctions prises en charge. La deuxième réalise trois agrandissements sur des textures synthétiques. Exécutez la troisième sur un bureau Windows visible. Pour 180 présentations, cherchez 180 activations, jusqu’à 360 images affichées, 180 couleurs sans interface étiquetées et un statut `0x0`. Le marquage synthétique vérifie la présentation. La capture avant le dessin de l’interface du titre se vérifie en jeu.

Le 27 novembre 2026, la RTX 4070 SUPER a réalisé trois agrandissements synthétiques en mode Quality, de 1706×960 à 2560×1440, avec Streamline 2.14.16 et NGX 310.9.16. Dans la session de test hors ligne, `slShutdown` est ensuite resté en attente dans la télémétrie NGX. Le diagnostic se termine en erreur après 30 secondes si la fermeture bloque. Le bureau de la session était masqué. Le rapport entre images affichées et rendues reste à mesurer sur un bureau visible.

## Jouer et comparer

Pour ouvrir le lanceur avec le binaire VIctory que vous venez de vérifier, exécutez cette commande à la racine du dépôt après avoir compilé `anyxboxx-launcher` dans `target-codex`.

```powershell
.\target-codex\release\anyxboxx-launcher.exe --page play --victory-exe $victory
```

Dans **Play → NVIDIA Streamline**, sélectionnez le dossier des DLL et activez **DLSS Super Resolution** et **DLSS Frame Generation 2×**. Commencez en 1920×1080 avec le mode Quality. Le lanceur demande les soumissions ordonnées quand la génération est activée. Pour utiliser la CLI avec les mêmes réglages, remplacez le chemin du jeu par le vôtre.

```powershell
anyxboxx run --game-dir 'D:\Jeux\MaCopie' --victory-exe $victory --override "VICTORY_STREAMLINE_DIR=$env:VICTORY_STREAMLINE_DIR" --override VICTORY_OUTPUT=1920x1080 --override VICTORY_DLSS=1 --override VICTORY_FRAMEGEN=1 --override VICTORY_THIN=1
```

Dès que le périphérique est prêt, VIctory utilise le sélecteur de rendu 7 et l’échelle FidelityFX existants. `VICTORY_DLSS_MODE` accepte `dlaa`, `quality` (par défaut avec DLSS), `balanced`, `performance` et `ultraperformance`. Une valeur explicite de `VICTORY_FFX_SCALE` prime sur cette échelle. `VICTORY_DLSS_PRESET` sélectionne le modèle (default, E, F, J, K, L ou M). La génération seule utilise par défaut l’échelle DLAA et demande les dimensions natives du FidelityFX du titre, 1280×720 ou 1920×1080 en entrée et 1920×1080 en sortie.

Aux autres dimensions, DLSS utilise un contexte natif dimensionné pour les tables fixes du titre. Si DLSS refuse un traitement et que les dimensions conviennent, le FidelityFX du titre prend le relais. Si le contexte natif est trop petit, VIctory renvoie et journalise une erreur.

Cache chaud, comparez le même mouvement de caméra avec et sans génération. Relevez les compteurs de présentations et d’images effectivement affichées, observez la stabilité de l’interface et vérifiez la pause et la reprise. `VICTORY_FRAMEGEN_HUDLESS=0` désactive la copie avant interface sur Series X pour la comparaison. Les identifiants des shaders de ce dessin proviennent des observations de VIctory-NVIDIA sur le build 1.0.2.1. Sur Series S, les plans de la scène et de l’interface sont étiquetés séparément.
