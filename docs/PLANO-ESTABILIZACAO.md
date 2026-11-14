# Plano de estabilização do LiveMix

Este plano cobre a cópia de trabalho atual. Cada etapa termina com um resultado verificável. Os tempos são estimativas de trabalho para uma pessoa com acesso às máquinas de teste. Dependências de hardware, drivers e sessões reais podem aumentar o calendário.

## Prioridade e sequência

| Etapa | Trabalho | Estimativa |
| --- | --- | --- |
| 0 | Inventário dos modais, projeto de teste e medições iniciais | 2–4 dias |
| 1 | Diagnóstico e correção de áudio e sincronismo | 5–8 dias |
| 2 | Importar, Exportar, Padrão e correções prioritárias dos modais | 4–7 dias |
| 3 | Decoders com diagnóstico, fallback e testes por plataforma | 4–7 dias |
| 4 | Otimizações CPU/GPU guiadas pelas medições | 6–12 dias |
| 5 | Compatibilidade GT e Web Controller em incrementos | 8–15 dias |

A ordem coloca estabilidade de áudio antes de alterações profundas no pipeline. As medições da etapa 0 guiam a etapa 4. A etapa 2 estabelece o formato de configurações usado pelas preferências de decoder e pelo Web Controller. Estimativa total de **6–11 semanas úteis**. A paridade completa, um backend DirectShow completo e a prévia remota ficam fora dessa estimativa.

## 0. Inventário e teste de referência

- Inventariar **Add Input**, **Input Settings**, **Settings**, Recording, MultiCorder, Streaming, External, Replay e Audio Settings. Para cada controle, registrar tela, ordem, valor padrão, dependências, handler, persistência, plataforma e estado no LiveMix. Citar método ou recurso no código descompilado quando identificado. Marcar diferenças visuais e funcionais em campos separados da matriz.
- Conferir o caminho inteiro de **Add Input**, do seletor ao preset reaberto. O menu já exibe seis tipos indisponíveis (DVD, PowerPoint, Virtual Set, Web Browser, Video Call e Zoom). Classificar cada um como prioridade, substituição existente ou item de roadmap. O fato de uma página de Input Settings estar habilitada não valida todos os seus controles.
- Criar um projeto sintético reproduzível com vídeo com flash e clique, duas fontes, título com alpha, transições, MultiView e saídas opcionais. Guardar duração, resolução, FPS, GPU, driver, dispositivo de áudio, frequência, configuração, comando e resultado de cada execução.
- Capturar os modais de referência e do LiveMix em 100%, 125% e 150% de escala, quando disponível, com inglês e pt-BR. Conferir rolagem, foco, Tab, teclado, fechamento, labels cortados e espaçamento de 8 px.

**Aceite** — matriz de controles com estado comprovado e uma cena de referência reproduzível em Windows e macOS. Corrigir primeiro controles que causam configuração incorreta ou impedem uma operação. A revisão visual vem após a funcional.

## 1. Áudio e sincronismo

- Separar três sintomas: estalos somente nos fones, estalos presentes nos arquivos/streams e desvio A/V que cresce com o tempo. O mixer já tem correção de deriva, resync e suavização de relógio. Reproduzir o defeito antes de alterar buffer ou DSP.
- Medir underruns, overruns, resyncs, profundidade de fila, picos no callback e atraso A/V. Registrar por entrada e saída. Testar 44,1 e 48 kHz, câmera com áudio, arquivo com áudio, USB e DeckLink quando disponíveis, e as saídas fones, MP4, RTMP e SRT.
- Testar primeiro 30 minutos em 1080p59,94, com troca de entradas, gravação e transmissão em loopback. Repetir com a mesma cena após a correção. Exercitar reinício e troca do dispositivo de áudio.
- Corrigir no caminho onde aparecer a falha. Separar correção de relógio e reamostragem, faltas de amostras e atraso fixo de cada saída. Acrescentar regressão sintética para a causa observada.

**Aceite inicial** — sem estalos detectados no teste de 30 minutos, sem crescimento da diferença A/V e variação do sincronismo de até um quadro do projeto em relação à medição inicial. Registrar o atraso fixo e os resultados por caminho. Validar com hardware real antes de considerar o item concluído para produção.

## 2. Modais e configurações portáveis

- Colocar **Importar**, **Exportar** e **Padrão** no rodapé de Settings. No LiveMix, a leitura e a escrita atuais usam `settings.txt`. Reusar a mesma representação de configurações com uma versão de esquema e aplicar a importação somente após validar o arquivo inteiro.
- Exportar as preferências de forma portátil. O arquivo padrão omite chaves de stream e segredos. Na importação, listar dispositivos, monitores e caminhos indisponíveis e manter uma cópia recuperável das preferências anteriores. **Padrão** aplica os defaults do LiveMix. Mudanças que exigem reinício ou reabertura de fontes ficam visíveis.
- Validar separadamente a importação de um arquivo de configuração exportado pelo aplicativo de origem, com amostra fornecida pelo operador. Mapear campos comprovados e listar os ignorados. Compatibilidade com presets existentes não garante compatibilidade com o formato de exportação de configurações.
- Aplicar as correções da matriz, começando por Add Input, Input Settings, Recording/MultiCorder e External. Preservar nomes e ordem reconhecíveis, agrupar controles relacionados e dar suporte à rolagem nas janelas compactas.

**Aceite** — exportar, alterar, importar e confirmar recuperação das preferências; arquivo inválido não deixa estado parcial; **Padrão** retorna aos valores de instalação; presets, projetos e gravações continuam acessíveis. Cada controle corrigido tem teste funcional, além da captura visual quando aplicável.

## 3. Decoders

- A página já roteia arquivos por extensão ou por seleção global para FFmpeg no Windows. Media Foundation atende os demais arquivos. No macOS, arquivos passam por FFmpeg. BWDIF e YADIF já existem no caminho FFmpeg. Conferir que os controles persistem e afetam realmente arquivo e playlist.
- Identificar formato e backend efetivamente inicializado, mostrar fallback e motivo do erro. Distinguir escolha de demuxer, decoder, desentrelaçamento e aceleração por hardware. A indicação de hardware deve vir do decoder inicializado.
- Testar MP4/MOV, MXF, MPEG-2 TS/PS e arquivos entrelaçados, com e sem FFmpeg disponível. Cobrir áudio, primeiro frame, seek, loop, troca de item na playlist e extensão em maiúsculas.
- O original inspecionado tem `MpegVideoDecoder`, `MpegAudioDecoder`, `H264VideoDecoder`, `BlockedFilters`, `VideoDeinterlacing` e `FFMpegExtensions`. Avaliar DirectShow após estabilizar Media Foundation/FFmpeg. Prioridades e bloqueio de filtros só fazem sentido se houver um backend DirectShow que de fato os use.

**Aceite** — a configuração sobrevive ao reinício, o diagnóstico identifica o caminho em uso e erros ou fallback podem ser reproduzidos. A compatibilidade inicial cobre os backends já existentes nas duas plataformas.

## 4. CPU/GPU

- Medir primeiro três cenários: projeto sem saídas, projeto com gravação e projeto com gravação mais rede. Usar 1080p59,94 como base. Medir 4K60 separadamente. Registrar CPU, GPU por engine, tempo de render p50/p95/p99, quedas, uso de VRAM, filas e bytes de readback por segundo.
- Priorizar a eliminação de cópias. O caminho D3D11/OpenGL para H.264 já foi validado em casos específicos. Medir conversão de cor e scaling, chroma key, scopes, uploads de miniaturas e readback ao misturar saídas. Separar CPU de captura/decodificação do custo do compositor e dos encoders.
- Implementar um gargalo por vez. Considerar shader para operações por pixel, cache ou menor cadência para miniaturas e scopes, compartilhamento de textura e encode na mesma GPU quando o backend aceitar. Validar fallback em iGPU e em adaptadores divergentes.
- Fazer uma matriz com NVIDIA dedicada, notebook híbrido, Intel/AMD conforme disponibilidade e Apple Silicon. Metal, VideoToolbox nativo e GPU entre plataformas são projetos próprios se as medições justificarem.

**Aceite** — relatório antes/depois na mesma cena, com melhora mensurável de CPU, cópias ou estabilidade de frames e sem regressão observada em áudio, qualidade ou latência. A meta de CPU fica vinculada à medição inicial de cada máquina.

## 5A. GT Titles

- Ampliar a suíte existente com Spinner 180, Stinger 2, Scoreboard 3, Scoreboard 13 e Social Kimono, usando os arquivos instalados quando fornecidos pelo operador. Guardar no repositório fixtures sintéticos que isolem os recursos. Conferir frames em tempos específicos, alpha, ordem de composição, relógios e data-change.
- Gerar a lista de recursos ausentes por template. O parser hoje sinaliza casos de crop feather, efeitos, geometria, animação contínua e storyboards. Priorizar pelo que quebra títulos usados em produção. Separar arquivos GT de XAML legado.
- Tornar o diagnóstico visível no editor de título e no relatório de importação, com o nome do recurso e o objeto afetado quando o parser conseguir identificá-lo.

**Aceite** — cada caso prioritário abre, renderiza e mantém campos editáveis. Os frames de referência e eventos de animação passam nos testes. O diagnóstico informa limitações que ainda afetam o template.

## 5B. Web Controller

- Hoje há comandos e Telestrator por loopback. Implementar primeiro leitura de estado, tally e um switcher com Preview/Program/overlays, reutilizando o executor de ações. Depois adicionar editor de campos GT e roteamento por input no Telestrator.
- Para uso em LAN, adicionar configuração explícita de interface/porta, autenticação, permissões por função, proteção das rotas de mutação e testes de acesso. Manter loopback como padrão. Descrever a API de compatibilidade por endpoint e comando, com respostas claras para funções ainda indisponíveis.
- Avaliar a prévia pela rede só depois de estabilizar o controle. Preview de vídeo e áudio via navegador precisa de transporte, limites de CPU e autenticação próprios.

**Aceite** — teclado/MIDI/web acionam o mesmo comportamento; dois clientes recebem mudanças de tally e estado; comandos inválidos ou sem permissão não alteram a produção; teste em uma segunda máquina antes de liberar LAN.

## Progresso do primeiro incremento

| Área | Resultado observado | Verificação em aberto |
| --- | --- | --- |
| Configurações | Rodapé com Importar, Exportar e Padrão. JSON versionado, validação antes de aplicar e backup das preferências. | Importação do formato original, verificação de dispositivos e teste visual nas escalas do plano. |
| Modais | Espaçamento de 8 px no conteúdo de Add Input, e grades de General e External. Recording, MultiCorder e Streaming já usavam intervalos de 8 px. | Inventário de controles, estados e comparação visual com o original. |
| Áudio | O callback de monitor reutiliza seus buffers. Os testes de relógio, seek, decodificação de áudio e Replay passaram. | Medir estalos e deriva no monitor, na gravação e na rede durante uma sessão de 30 minutos. |
| Decoders | Diagnóstico em Input Settings → General. Windows tenta FFmpeg se o Media Foundation falhar antes de entregar o primeiro quadro. O fallback abriu um MKV FFV1 sintético em 64×48 a 25 fps. | Matriz de formatos e drivers, incluindo seek e troca de item. Seleção e bloqueio de filtros DirectShow seguem em aberto. |
| CPU/GPU | O diagnóstico de Program conta quadros e bytes submetidos a readback. Os testes D3D11 confirmaram o gerenciador compartilhado e a gravação a partir de superfícies de GPU. A máquina tem Intel Arc e RTX 4060 Laptop. O teste isolado usou o Microsoft AVC DX12 Encoder HMFT com 12 superfícies sintéticas de 320×180, timestamps espaçados em 33 ms e 8 descartes. O arquivo reabriu como textura GPU. | Medir o compositor com e sem saídas na mesma cena e identificar o adaptador usado em cada execução. |
| GT | Os cinco templates instalados abriram sem recursos marcados como incompatíveis. Campos identificados: Spinner 180 (0), Stinger Blue (0), Lineup 12 (27), The Extreme (7), Kimono (3). O editor exibe o diagnóstico quando houver limitações. | Comparar frames, alpha, animações e relógios com o original. |
| Web Controller | Painel local lê Program, Preview, REC, streaming e overlays e usa o executor de ações do switcher para os comandos de produção. | Validar dois navegadores, edição GT, Telestrator por input e autenticação para LAN. |

A [auditoria de modais e configurações](AUDITORIA-MODAIS.md) detalha a comparação estática, os testes de troca de configurações XML e o caminho opcional DirectShow.

## Próximo incremento

Entregar o inventário de **Add Input**, **Audio Settings** e **Decoders**, uma cena de teste reproduzível e a primeira captura de 30 minutos. O resultado deve dizer se os estalos ocorrem no monitor, no arquivo ou em ambos, e em que ponto o relógio A/V começa a divergir. Isso define a primeira correção e evita otimizar um trecho que não é o gargalo.
