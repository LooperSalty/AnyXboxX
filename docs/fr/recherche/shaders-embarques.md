> English: [embedded-shaders.md](../../research/embedded-shaders.md)

# Shaders embarqués du paquet : mesure sans llvm-mc

Mesure du 22 novembre 2026, faite localement ; seuls des comptes figurent ici (aucun shader, désassemblage ni
SPIR-V, règle 1 d'AGENTS.md).

`XboxBvhBuilder.dll` (bibliothèque de Microsoft livrée dans le paquet) contient 52 conteneurs DXBC, chacun avec
un programme RDNA 2 natif (`XBCH` → `FSSM`), d'après la note d'un projet tiers (retiré depuis).
Ce sont les seuls vrais shaders console disponibles avant l'exécutable du jeu.

| Étape | Résultat |
|---|---|
| Désassemblage par AnyXboxX (sans llvm-mc) | 54 197 instructions ; 1 431 mots sans texte, des données entre les programmes d'un même blob (jusqu'à 5 programmes) |
| Traduction RDNA 2 → SPIR-V (traducteur tiers, retiré depuis, nourri par notre désassemblage) | **51 programmes sur 52** |
| Échec restant | `buffer_store_short`, règle absente du traducteur |

Deux corrections ont permis ce résultat (avant : 0, puis 41 sur 52) :

1. **Compute shaders** : le traducteur lit l'état de dispatch (`rsrc2`, taille du groupe) dans l'en-tête FSSM.
   `rdna2_spirv.translate` accepte maintenant le blob (`fssm=`) ou cet état (`dispatch=`) ; sans l'un ni
   l'autre, un compute shader est refusé avec un message clair.
2. **Données entre programmes** : avec `code_start`, `disassembly` saute un mot sans texte comme llvm-mc et le
   signale ; le traducteur se recale sur le blob FSSM.

Cette mesure ne peut plus être reproduite depuis ce dépôt : les outils d'extraction et le traducteur venaient de ce
projet tiers, retiré depuis, et les changements de code décrits plus haut ont été retirés avec lui.
