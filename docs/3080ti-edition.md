# AnyXboxX — 3080 Ti Edition

## Objetivo

Manter uma linha de testes independente para avaliar o AnyXboxX em:

- GPU: NVIDIA GeForce RTX 3080 Ti, 12 GB de VRAM.
- CPU: AMD Ryzen 7 7800X3D.
- RAM: 16 GB.
- Meta inicial: estabilidade e redução do uso de memória; desempenho será medido depois.

## Estado inicial

- Branch de trabalho: `3080Ti-Edition`.
- Base: branch `main` do fork `thsolucoes834-eng/AnyXboxX`.
- Nenhuma alteração no comportamento do emulador foi feita nesta edição até este registro.
- O repositório não inclui arquivos do jogo; os testes devem usar somente uma cópia legítima do usuário.

## Descobertas da análise inicial

1. O README da base relata aproximadamente 20–25 GB de RAM em alguns cenários e recomenda 32 GB.
2. O perfil NVIDIA rápido usa reservas de 1536 MB para janelas de draw-data e 1536 MB para cópias de buffers.
3. O launcher já oferece um preset para placas de 12 GB: reservas de 1024 MB, `VICTORY_TEXTURE_ROOM_PERCENT=90` e limite de textura de 2048 pixels. O próprio código informa que esse preset ainda não foi medido em uma placa de 12 GB.
4. A RTX 3080 Ti tem 12 GB de VRAM, então esse preset é um ponto de partida para testes de VRAM, não uma solução comprovada para o limite de 16 GB de RAM do sistema.
5. O README e os guias indicam Windows 10/11 x64, CPU com AVX2, Direct3D 12, Shader Model 6.6 e mesh shaders.

## Plano de testes

1. Registrar versão do Windows, driver NVIDIA, RAM disponível e espaço livre em disco.
2. Fazer uma execução de referência sem alterar configurações, anotando RAM, VRAM, FPS, travamentos e logs.
3. Repetir com o preset de 12 GB e comparar os mesmos indicadores.
4. Só então investigar ajustes adicionais, um por vez, sempre nesta branch e com possibilidade de reverter.
5. Não reduzir limites ou desativar recursos sem evidência nos logs ou medições.

## Regras desta edição

- Não fazer commits na branch `main` por engano.
- Não alterar o repositório original `LooperSalty/AnyXboxX`.
- Não incluir executáveis, chaves ou arquivos do jogo no Git.
- Registrar cada mudança e o resultado do teste correspondente.
