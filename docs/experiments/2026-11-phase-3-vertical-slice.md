# Fase 3 — primeira fatia vertical (device hub + sessão + console)

> **Status:** parcial — o app GPUI tem o caminho completo para listar,
> selecionar, configurar, espelhar e parar um dispositivo no Linux, sem Tauri.
> O smoke com hardware ainda não foi executado (sem `adb` neste host).
> **Data:** 2026-11-23
> **Crate:** `crates/open-scrcpy-gpui`
> **Plano:** [docs/GPUI-MIGRATION.md](../GPUI-MIGRATION.md)

## 1. O que funciona agora

O casco deixou de ser estático: ele fala com `open-scrcpy-core` e com o
`scrcpy` real.

- **Device hub:** polling de `adb devices -l` a cada 3 s pelo core; lista
  disponíveis e indisponíveis (`unauthorized`/`offline`) em vez de sumir com
  elas; seleção por botão; auto-seleção do primeiro online; se o dispositivo
  some, a seleção é limpa.
- **Sessão de espelhamento:** `Start mirroring` monta o `ScrcpyConfig` a partir
  das preferências persistidas e chama `core::monitor::run_session`; `Stop
  session` chama `core::session::stop_session`. Os botões ficam desabilitados
  quando não há dispositivo/seleção ou quando a sessão já está rodando.
- **Painel Mirror:** bitrate (slider 1–50 Mbps), frame rate (30/60/90),
  resolução máxima (Original/1920/1280/720), codec (H.264/H.265/AV1), áudio
  (fonte de captura, duplicação no aparelho e exigir áudio), VSync, manter
  acordado, desligar a tela, reconexão automática, HID
  teclado/mouse, OTG (pure HID), always-on-top, borderless, fullscreen,
  gravação em vídeo, rotação e renderer (lista de drivers lida de
  `scrcpy --help` com fallback para Automático). Cada mudança é gravada no
  `settings.json`; o evento `scrcpy-window-pos` do core salva a posição da
  janela do espelho, que volta em `--window-x/--window-y` no próximo start; o
  resumo mostra a configuração efetiva antes de iniciar.
- **Console:** o `EventBus` do core é consumido por uma task GPUI; logs do
  `scrcpy`, status de sessão e erros aparecem no painel, com teto de 100 linhas.
  A toolbar tem **Clear**, **Copy** (clipboard do sistema) e **Report** (JSON de
  diagnóstico em Downloads com versão, locale, tema, modo, dispositivos e logs).
  Um campo de terminal executa `adb`/`scrcpy` (parse e allowlist no core,
  output limitado a 256 KiB) e despeja stdout/stderr no console. O painel
  rola para o fim a cada linha nova e coloriza `[ERROR]`/`[WARN]` com os
  tokens de perigo/aviso.
- **Notificações nativas:** fim inesperado de sessão (erro ou saída não
  solicitada) dispara `SystemNotification` com o dispositivo; parada manual não
  notifica.
- **Teardown:** fechar a janela mata os filhos e descarrega o sink virtual
  (`core::session::teardown_sessions`), o mesmo código que o casco Tauri passou
  a usar.
- **Câmera:** facing (traseira/frontal/externa), **lente específica**
  (descoberta via `scrcpy --list-cameras`, com botão Refresh e fallback Auto),
  aspecto, zoom (slider 1–10×), torch, high-speed, virtual mic e tamanho
  máximo; o start usa `ScrcpyConfig::camera` e o resumo reflete as escolhas.
- **Microfone:** ponte sem janela (`ScrcpyConfig::mic`) com fonte (mic cru /
  denoised), codec (Opus/AAC), bitrate, RNNoise e "definir como microfone
  padrão" (Linux). O painel mostra o destino do stream (`mic-route`) e um
  medidor de nível ao vivo (`mic-level`, dBFS → barra), consumidos do
  `EventBus`. O start usa o mesmo session bar e o teardown do core descarrega
  o sink.
- **Desktop (virtual display):** presets de tamanho (1080p/1440p/4K/ultra-wide),
  sliders de **largura/altura/DPI livres**, rotação, tamanho máximo de vídeo e
  flex display; o resumo mostra a configuração e o start usa
  `ScrcpyConfig::desktop`. As preferências de performance/áudio são
  compartilhadas com o Mirror.
- **Wireless:** a sidebar tem `IP Connect`, descoberta **mDNS** (polling de
  `adb mdns services` a cada 5 s; clicar preenche o endereço de conexão ou de
  pairing), **pares recentes** persistidos (máx. 5, atualizados após um connect
  bem-sucedido), e `Start Pairing` **guiado** (usa o serviço de pairing do mDNS
  quando o campo está vazio, tenta até 20 vezes com backoff de 1–5 s e mostra
  `attempt/N`), ligados a
  `core::adb::connect`/`pair`; os passos e erros do adb vão para o console e o
  dispositivo conectado aparece no hub no próximo polling.
- **Arquivos:** "Flash / Push Files" abre o seletor nativo (portal) e envia
  cada arquivo escolhido com `core::adb::push_file`; arquivos `.apk` vão por
  `install_apk`. Resultado e erros entram no console. O botão fica desabilitado
  sem dispositivo; soltar arquivos externos na janela usa o mesmo caminho de
  transferência (`.apk` instala, o resto faz push).
- **Onboarding:** se `scrcpy --version` falha no start, o app mostra o painel de
  inicialização do core (em vez das abas) com Download Core Binaries, progresso
  vindo do `EventBus`, link para os releases e Continue quando o core fica
  pronto; a conclusão é persistida.
- **Atalhos:** card informativo na sidebar com os 8 atalhos de device do 2.x
  (`Alt+F/H/B/S/P/R/V/O`), nos 8 locales. No 2.x esse card também é apenas
  informativo (não há handler global de `Alt` no frontend atual).
- **Preferências:** diálogo "Application" com *notificações nativas* e
  *lembrar posição da janela*; ambos persistem, controlam as notificações de
  fim inesperado e a restauração de `--window-x/--window-y`. O texto do modo
  background explica por que ele está indisponível.
- **Header:** diálogo de idioma com os 8 locales (troca em runtime e fecha ao
  escolher), diálogo **Sobre** com versão, descrição, repositório/releases
  (abertos no navegador) e licença, além do check de update com badge.
- **Serviços de plataforma:** instância única por sessão de usuário e atalho
  global `Ctrl+Alt+Shift+C` para recentralizar a janela do espelho ativo. A
  falha de registro é não-fatal e vai para o stderr.
- **Tema e locale** continuam funcionando junto com o resto.

## 2. Arquitetura

```
AppShell (Entity<AppShell>)
  ├─ AppServices
  │    ├─ Arc<Runtime> (tokio: processo, rede, timers do core)
  │    ├─ Arc<ScrcpyState> (registro de sessões, gerações, mic)
  │    ├─ EventBus (core → GPUI)
  │    └─ Arc<dyn SessionHost> (KWin/xdotool/Win32 + force_kill)
  ├─ task de eventos (WeakEntity: EventBus → logs/status)
  └─ task de polling (adb a cada 3 s → device hub)
```

- O core usa `tokio::process` e timers; o GPUI tem o próprio executor. Por
  isso `AppServices` mantém um runtime Tokio dedicado e entrega os resultados
  ao shell por `WeakEntity::update`.
- `SessionHost` é implementado por `GpuiSessionHost` reusando
  `open-scrcpy-platform` (`kwin`, `window`), sem nenhum tipo Tauri.
- O `AppServices` é `Clone` (Arcs + canal) e vive no `AppShell`; o runtime cai
  junto com a janela.

## 3. Decisões

1. **Uma janela, um runtime.** Sem `AppHandle`/`invoke`; a sessão é chamada
   como função de biblioteca.
2. **Logs em inglês, UI em i18n.** As linhas do console mantêm o formato do
   app 2.x (`[SYSTEM] ...`), como no Tauri; os controles visíveis usam os oito
   catálogos.
3. **As quatro abas estão implementadas.** A chave temporária
   `controlPanel.modeUnavailable` foi removida dos 8 locales quando a última
   aba (Camera) passou a montar sua própria sessão.
4. **Teardown no core.** O casco Tauri deixou de duplicar a limpeza de
   sessões; o comportamento dos dois cascos é o mesmo por construção.
5. **`ScrcpyConfig::mirror`** e **`ScrcpyState::new`** são o baseline do core,
   para o shell não repetir 45 campos nem o registro de estado.
6. **`MirrorSettings`** é a preferência da UI (validada e persistida); o DTO do
   core continua sendo o contrato de linha de comando, montado por
   `MirrorSettings::to_config`. O core revalida tudo antes de spawn.
7. **Controles do Kit, não `div`s clicáveis:** `Slider` para bitrate,
   `RadioGroup`/`Radio` para conjuntos pequenos e `Switch` para preferências
   booleanas. Nome acessível explícito no slider; os valores técnicos
   (`H.264`, `Mbps`, resoluções) não são traduzidos.

## 4. Gates

| Gate | Resultado |
| --- | --- |
| `cargo fmt --check` (gpui/core/tauri) | PASS |
| `cargo clippy --all-targets -- -D warnings` (gpui) | PASS |
| Smoke de abertura (`timeout 10 openscrcpy`) | PASS (ficou rodando, sem panic) |
| `cargo test` (gpui) | **39 passed** (35 unit + 1 a11y + 2 paridade + 1 UI) |
| `cargo test` (core) | 79 passed, 1 ignored |
| `npx tsc --noEmit` | PASS |
| `npm test` | 166 passed (33 arquivos) |
| Smoke com dispositivo real | **não executado** — sem `adb` neste host |
| `cargo build --release` | não executado |

### 4.1 Bloqueio novo: tray/background sem API de esconder

O GPUI 0.3.12 expõe `Window::activate_window`, mas **não** expõe `hide()` nem
`platform_window()`; `hide` existe só na trait de plataforma interna. Sem isso,
o modo background do 2.x (fechar a janela → continuar no tray) não tem como
esconder a janela: minimizar não é equivalente e deixa a janela na barra de
tarefas.

Consequências:

- o tray (`ksni`) pode ser registrado e oferecer Show/Quit, mas "fechar para o
  tray" não é implementável hoje;
- o item 3.9 (background) e parte de 3.11 (tray) ficam bloqueados até existir
  uma API pública de esconder/mostrar (upstream ou patch do `gpui-pre`);
- o atalho global e a instância única **não** dependem disso e foram feitos.

Nota do smoke: com o OpenScrcpy 2.0 aberto, o registro de
`Ctrl+Alt+Shift+C` falha com "HotKey already registered" e o app segue — o
comportamento esperado durante a convivência dos dois cascos.

### 4.2 Compatibilidade com scrcpy 5.0.1

O relato de uso com scrcpy 5.0.1 expôs três pontos, corrigidos nesta rodada:

- o help do 5.x imprime `--new-display[=[<width>x<height>][/<dpi>]]`; o parser
  tratava o `[` como parte do nome e concluía que o binário não tinha
  `--new-display`, recusando o modo Desktop. O nome agora para no primeiro
  caractere que não pertence a uma flag, com teste usando a linha real do 5.x;
- o `--flex-display` foi removido no scrcpy 5.0; o toggle só aparece quando o
  binário anuncia a flag, e o valor salvo é limpo caso contrário;
- o log de início dizia "mirror session" em qualquer modo; agora usa o modo
  ativo (mirror/desktop/camera/microphone).

O check de update passou a tratar HTTP 404 do manifesto como "sem canal de
release" em vez de erro de rede — o `latest-gpui.json` só existirá quando a
release nativa for publicada.

### 4.3 Componentes do Kit aplicados

Depois do protótipo (`artifacts/prototypes/ui-before-after.html`), o casco
recebeu:

- **Button variants**: `primary` no Start e no Connect (commit da tarefa),
  `danger` no Stop, `ghost` no header e na toolbar do console (com `compact`),
  `outline` em ações secundárias (Push, Pair, idioma). A hierarquia segue os
  Design Guides: primary escasso, danger para o commit destrutivo, ghost para
  ações silenciosas.
- **Avatar + Badge**: cada device mostra a inicial do modelo em Avatar, estado
  como ponto + texto (não só cor), transporte (USB/Wi-Fi) e **uma** Badge com a
  versão do Android (`getprop ro.build.version.release`, consultada uma vez por
  serial e cacheada). Falha na consulta apenas omite a badge.
- **AlertDialog**: Stop abre confirmação com o objeto no título
  ("Stop the session on SM-A055M?"), botão `Stop session` em danger e
  `Cancel`; o foco volta ao gatilho pelo próprio componente.
- **Progress**: o download do core no onboarding agora tem barra determinada,
  além do texto de porcentagem.

Ficaram de fora, de propósito: `TabBar` (os botões de modo têm IDs estáveis
usados pelos testes de UI; a troca exige repensar a navegação), `Combobox`
(útil quando a lista de lentes crescer), `Kbd` (cosmético) e `Empty` (o texto
atual cobre o estado vazio). `Bubble` e `Checkbox` seguem descartados por não
haver conversa nem multi-seleção real.

## 5. O que falta nesta fase

| Fatia | Situação |
| --- | --- |
| 3.1 Header (menu de idioma real, update badge) | **parcial**: menu de idioma, Sobre, check/badge de update prontos; botões de janela dependem do chrome (3.12) |
| 3.2 Device hub | **feito** (USB; wireless/pairing ainda não) |
| 3.3 Wireless (IP/pairing/histórico) | **feito**: IP, pairing manual e guiado (mDNS + retries), pares recentes (smoke com hardware pendente) |
| 3.4 Mirror (bitrate/fps/res/codec/renderer/OTG/áudio) | **feito**: performance, áudio, HID/OTG, janela, gravação, rotação e renderer; smoke com hardware pendente |
| 3.5 Camera | **feito**: facing, lente (`--list-cameras`), aspecto, zoom, torch, high-speed, virtual mic e tamanho (smoke com hardware pendente) |
| 3.6 Desktop | **feito**: presets, largura/altura/DPI livres, rotação, max size e flex (falta o ratio lock opcional) |
| 3.7 Mic (source, meter, default source) | **feito** (fonte, codec, bitrate, RNNoise, source padrão, destino e medidor em dBFS); smoke com hardware pendente |
| 3.8 Sessão + logs | **feito** (iniciar/parar, status, console 100 linhas, clear/copy/report, terminal adb/scrcpy) |
| 3.9 Behavior (reconnect/background/notificações) | **parcial**: reconnect, notificações (com opt-out) e posição da janela prontos; background/tray **bloqueados** pela ausência de `Window::hide` no GPUI 0.3.12 (ver 4.1) |
| 3.10 Modais (onboarding/atalhos/sobre/update) | **parcial**: onboarding, Sobre e card de atalhos prontos; modal de update e confirmações pendentes |
| 3.11 Integrações (tray/single-instance/hotkeys/drag-drop) | **parcial**: instância única, recenter global e drag-drop de arquivos prontos; tray/background bloqueados (4.1) |
| 3.12 Chrome/titlebar | **decidido**: decoração `Server` explícita + título da janela, conforme P8; os botões/resize do webview do 2.x não foram portados |

## 6. Próximos passos

1. Smoke de pairing com aparelho real (o fluxo guiado já existe).
3. Camera/Desktop/Mic com os campos que já existem no DTO.
4. Camera (3.5), que já tem os campos no DTO e as chaves de i18n.
5. Camera: descobrir lentes via `scrcpy` e selecionar `camera_id`.
6. mDNS/histórico do wireless e arquivos/terminal/relatório.
7. Tray/background: depende de API pública de hide/show (bloqueio 4.1).
8. Drag-and-drop externo, terminal no console e updater.
5. Substituir o botão de ciclo de idioma pelo menu e remover a chave
   `modeUnavailable` quando as quatro abas estiverem implementadas.
