# Auditoria dos modais do LiveMix

Referência local examinada no Windows. A comparação combina a árvore de controles observada em Settings → Decoders, a estrutura de um arquivo exportado pela aplicação de referência, os recursos de idioma da instalação, referências de IL catalogadas em `app/mechanics.json` e o código Rust. A inspeção visual ao vivo cobriu Settings → Decoders. Os demais itens abaixo são auditoria estática de controles e fluxo. Ainda precisam de uma passagem visual e funcional lado a lado em 100%, 125% e 150% de escala, em inglês e português.

## Seleção e edição de inputs

| Tela / seção | LiveMix | Diferença a verificar ou implementar |
| --- | --- | --- |
| Add Input / lista | 21 entradas de referência na ordem do código, com opções próprias depois. 18 selecionáveis. | DVD, Video Call e Zoom estão indisponíveis. PowerPoint estático, Web Browser e Virtual Set estão disponíveis. Conferir seleção por teclado, foco e retorno do seletor de arquivos. |
| Video, List, Image Sequence / Stinger | Arquivo e histórico, playlist com adicionar/remover, pasta para sequência, reprodução e seek. | Testar relink de cada caminho ao reabrir preset, seleção de canais de áudio e opções da aba Video para sequência, especialmente frame rate. |
| Camera / Blackmagic / Audio Input | Enumera dispositivos e modos conforme backend. A entrada Blackmagic também tem um seletor separado. | Comparar formato de captura, porta, áudio embutido, baixa latência e diferenças por driver. |
| NDI / OMT / Desktop / Stream / SRT | Descoberta e miniaturas, captura da área de trabalho, endereço e opções SRT. | Comparar abas, parâmetros de descoberta e lista de protocolos aceitos, e validar reconexão. OMT é uma opção adicional no LiveMix. |
| Image / Photos / Colour / Audio / Title | Arquivos e histórico conforme tipo, transições de fotos, cor/transparência/barras, GT e XAML com cobertura parcial. | Comparar opções por tipo e seguir a matriz GT por template. |
| Replay / Video Delay / Telestrator | Entrada de replay multicâmera, buffer de atraso e edição de anotações. | Comparar capacidades de buffer, roteamento A/B e destino do Telestrator no navegador. |
| Input Settings → General | Nome, metadados, aspecto, opções por fonte e link de áudio. | Conferir troca de fonte mantendo ajustes, Flatten Layers, Create Virtual Input e opções de transição por fonte. |
| Colour Adjust / Colour Key / Colour Correction / Effects | Correções de imagem, presets de chave e seção de efeitos disponíveis. | Conferir parâmetros, reset, máscaras, formato de arquivo e o resultado em Program e saídas, com e sem GPU. |
| Position / Layers | Posição, crop, copiar/colar posição, camadas com fonte e transformação. | Comparar importação/exportação de layout, bordas, navegação entre camadas, ordem e defaults. |
| Audio / Triggers / Tally Lights / PTZ / Advanced | Áudio próprio, script ao entrar no Program, controles de tally e VISCA, controles avançados. | Triggers do original têm uma lista editável de eventos. Auditar os eventos restantes, sinais de tally, provedores PTZ e Copy From. |

## Settings e janelas de operação

| Tela | Controles e fluxo encontrados | Lacuna ou teste pendente |
| --- | --- | --- |
| Settings → Display | Tema, cores, saída, FPS e Multiview, dois Fullscreens. | Comparar os dois monitores, presets de layout e cor após reiniciar. |
| Outputs / NDI / OMT / SRT | Quatro saídas, rotas, SRT por canal, saídas de rede. | Verificar áudio por rota, autoativação e confirmação de dispositivo ausente. |
| Options / Performance | Interface, arranque, transições, captura, miniaturas, preview skip, adaptador e diagnóstico. | Conferir Advanced, impacto real de cada opção, Cancel e aplicação após reinício. |
| Decoders | Extensões FFmpeg, BWDIF/YADIF e caminho DirectShow opcional no Windows. | No original há desentrelaçamento de câmera e vídeo separados, MPEG-2, MPEG Audio, H.264, Properties e bloqueio de filtros. LiveMix usa FFmpeg para áudio. Páginas de propriedades do filtro e modos nativos de desentrelaçamento seguem em aberto. |
| Recording / Recording Setup | Pasta, nome, formato, fonte, bitrate, split e segunda gravação. | Comparar parâmetros por codec, memória de gravação, WAV separado e comportamento ao alterar codec entre sessões. |
| MultiCorder | Formato, split e seleção de origens de captura/saídas. | Conferir seleção de pasta por origem, WAV separado, NDI MOV sem recompressão e exclusão mútua com Replay. |
| Streaming | Cinco destinos, fontes, resoluções, codecs, áudio e diagnósticos. | Conferir provedores e perfis específicos, modos de latência e opções de qualidade avançadas. |
| External Output / engrenagem | Seções de saída externa e roteamento DeckLink. | Conferir modos, porta, dispositivo, atraso de áudio e a relação entre menu rápido e Settings. |
| Audio / Audio Outputs / Audio Settings / Audio Engine | Opções gerais, Master/A/B/fones, solo, medidores, delay, EQ, compressão e gate. | Conferir matriz de canais, VST, dinâmica e todos os caminhos de áudio com dispositivos reais. |
| Web Controller / Tally / Shortcuts / Activators / Scripting | Web em loopback, MIDI, editor de atalhos, retorno de controlador e executor de comandos. | Tally global está desabilitado. Conferir LAN, autenticação, eventos de retorno, templates e funções ausentes. |
| Alerts / About | Alertas de gravação e Fade to Black, dados do aplicativo. | Verificar alertas com falha real de destino e escala da interface. |
| Instant Replay Setup / Instant Replay | Fontes, pasta, monitores, eventos, velocidade, faixa compacta, A/B e configuração avançada. | Comparar todos os botões com sessão ativa, primeira reprodução após Out, navegação no buffer, áudio e retomada de gravação. |
| Overlay Settings / PlayList / New Preset | Configuração dos oito overlays, playlist e formato de novo preset. | Comparar atalhos, confirmação, repetição e comportamento após reabrir o projeto. |
| Settings → Import / Export / Default | JSON do LiveMix e XML `.config` para preferências mapeadas. Default restaura padrões e guarda cópia das preferências. | Validar a importação do arquivo gerado em uma instalação isolada da aplicação de origem. Comparar os campos ignorados e a semântica de cancelamento dos dois programas. |

## Configuração de referência

O arquivo de referência foi exportado como XML UTF-8, com uma seção de preferências do usuário. Campos simples usam `setting serializeAs="String"`. As listas `FFMpegExtensions` e `BlockedFilters` usam `serializeAs="Xml"` e `ArrayOfString`. O valor observado de `FrameRate` foi `333667` com `Interlaced=True`, correspondente a 29,97 quadros/s e 59,94 campos/s.

O LiveMix troca um subconjunto de preferências de formato, cores, visualização, medidores e decoders. O importador informa quantos campos aplicou e quantos ignorou. Na exportação de referência, aplicou 27 e ignorou 164. O teste DirectShow entregou o primeiro quadro vermelho com a reprodução pausada, buscou o quadro azul a 1 s e rejeitou o Color Space Converter quando bloqueado. O arquivo XML exportado não inclui dispositivos, credenciais, perfis de streaming, atalhos, presets ou estado de saídas. O JSON segue como formato de backup do LiveMix.
