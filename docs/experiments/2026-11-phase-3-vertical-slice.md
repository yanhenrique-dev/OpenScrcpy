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
  resolução máxima (Original/1920/1280/720), codec (H.264/H.265/AV1), áudio,
  VSync, manter acordado, desligar a tela e reconexão automática. Cada mudança
  é gravada no `settings.json`; o resumo mostra a configuração efetiva antes de
  iniciar.
- **Console:** o `EventBus` do core é consumido por uma task GPUI; logs do
  `scrcpy`, status de sessão e erros aparecem no painel, com teto de 100 linhas.
- **Teardown:** fechar a janela mata os filhos e descarrega o sink virtual
  (`core::session::teardown_sessions`), o mesmo código que o casco Tauri passou
  a usar.
- **Câmera:** facing (traseira/frontal/externa), aspecto, zoom (slider 1–10×),
  torch, high-speed, virtual mic e tamanho máximo; o start usa
  `ScrcpyConfig::camera` e o resumo reflete as escolhas.
- **Microfone:** ponte sem janela (`ScrcpyConfig::mic`) com fonte (mic cru /
  denoised), codec (Opus/AAC), bitrate, RNNoise e "definir como microfone
  padrão" (Linux). O start usa o mesmo session bar e o teardown do core
  descarrega o sink.
- **Desktop (virtual display):** presets de tamanho (1080p/1440p/4K/ultra-wide),
  DPI, rotação, tamanho máximo de vídeo e flex display; o resumo mostra a
  configuração e o start usa `ScrcpyConfig::desktop`. As preferências de
  performance/áudio são compartilhadas com o Mirror.
- **Wireless:** a sidebar tem `IP Connect` e `Start Pairing` ligados a
  `core::adb::connect`/`pair`; os passos e erros do adb vão para o console e o
  dispositivo conectado aparece no hub no próximo polling.
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
| `cargo test` (gpui) | **23 passed** (20 unit + 2 paridade + 1 UI) |
| `cargo test` (core) | 76 passed, 1 ignored |
| `npx tsc --noEmit` | PASS |
| `npm test` | 166 passed (33 arquivos) |
| Smoke com dispositivo real | **não executado** — sem `adb` neste host |
| `cargo build --release` | não executado |

## 5. O que falta nesta fase

| Fatia | Situação |
| --- | --- |
| 3.1 Header (menu de idioma real, update badge) | parcial: locale por botão de ciclo |
| 3.2 Device hub | **feito** (USB; wireless/pairing ainda não) |
| 3.3 Wireless (IP/pairing/histórico) | parcial: IP/pairing manual prontos; descoberta mDNS, histórico e o fluxo guiado de pairing pendentes |
| 3.4 Mirror (bitrate/fps/res/codec/renderer/OTG/áudio) | parcial: bitrate/fps/res/codec/áudio/vsync/reconnect prontos; faltam renderer, rotação, HID, recording, always-on-top/borderless |
| 3.5 Camera | **parcial**: facing, aspecto, zoom, torch, high-speed, virtual mic e tamanho; descoberta/seleção de lente específica pendente |
| 3.6 Desktop | **parcial**: presets de tamanho, DPI, rotação, max size e flex; largura/altura/DPI livres e presets de ratio pendentes |
| 3.7 Mic (source, meter, default source) | **parcial**: fonte, codec, bitrate, RNNoise e source padrão prontos; medidor de nível (`parec`) e o painel de destino pendentes |
| 3.8 Sessão + logs | **feito** (iniciar/parar, status, console 100 linhas) |
| 3.9 Behavior (reconnect/background/notificações) | pendente |
| 3.10 Modais (onboarding/atalhos/sobre/update) | pendente |
| 3.11 Integrações (tray/single-instance/hotkeys/drag-drop) | pendente |
| 3.12 Chrome/titlebar | pendente (decoração nativa) |

## 6. Próximos passos

1. Completar o Mirror: renderer, rotação, HID, recording,
   always-on-top/borderless e o "pure HID".
2. Wireless: descoberta mDNS (`adb mdns services`), lista de pares recentes e
   o fluxo guiado de pairing (o IP/código manual já funciona).
3. Camera/Desktop/Mic com os campos que já existem no DTO.
4. Camera (3.5), que já tem os campos no DTO e as chaves de i18n.
5. Medidor de nível do Mic (eventos `mic-level` do core) e painel de destino.
6. Camera: descobrir lentes via `scrcpy` e selecionar `camera_id`.
7. Tray/single-instance/hotkeys (F2.8 `Platform` + F3.11).
5. Substituir o botão de ciclo de idioma pelo menu e remover a chave
   `modeUnavailable` quando as quatro abas estiverem implementadas.
