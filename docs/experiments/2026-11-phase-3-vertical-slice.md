# Fase 3 — primeira fatia vertical (device hub + sessão + console)

> **Status:** parcial — o app GPUI tem o caminho completo para listar,
> selecionar, espelhar e parar um dispositivo no Linux, sem Tauri. O smoke com
> hardware ainda não foi executado (sem `adb` neste host).
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
- **Sessão de espelhamento:** `Start mirroring` chama
  `core::monitor::run_session` com `ScrcpyConfig::mirror(serial)`; `Stop
  session` chama `core::session::stop_session`. Os botões ficam desabilitados
  quando não há dispositivo/seleção ou quando a sessão já está rodando.
- **Console:** o `EventBus` do core é consumido por uma task GPUI; logs do
  `scrcpy`, status de sessão e erros aparecem no painel, com teto de 100 linhas.
- **Teardown:** fechar a janela mata os filhos e descarrega o sink virtual
  (`core::session::teardown_sessions`), o mesmo código que o casco Tauri passou
  a usar.
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
3. **Nova chave `controlPanel.modeUnavailable`** nos 8 locales, para as abas
   Camera/Desktop/Microphone ainda não implementadas — em vez de fingir que
   funcionam.
4. **Teardown no core.** O casco Tauri deixou de duplicar a limpeza de
   sessões; o comportamento dos dois cascos é o mesmo por construção.
5. **`ScrcpyConfig::mirror`** e **`ScrcpyState::new`** são o baseline do core,
   para o shell não repetir 45 campos nem o registro de estado.

## 4. Gates

| Gate | Resultado |
| --- | --- |
| `cargo fmt --check` (gpui/core/tauri) | PASS |
| `cargo clippy --all-targets -- -D warnings` (gpui) | PASS |
| `cargo test` (gpui) | **14 passed** (11 unit + 2 paridade + 1 UI) |
| `cargo test` (core) | 73 passed, 1 ignored |
| `npx tsc --noEmit` | PASS |
| `npm test` | 166 passed (33 arquivos) |
| Smoke com dispositivo real | **não executado** — sem `adb` neste host |
| `cargo build --release` | não executado |

## 5. O que falta nesta fase

| Fatia | Situação |
| --- | --- |
| 3.1 Header (menu de idioma real, update badge) | parcial: locale por botão de ciclo |
| 3.2 Device hub | **feito** (USB; wireless/pairing ainda não) |
| 3.3 Wireless (IP/pairing/histórico) | pendente |
| 3.4 Mirror (bitrate/fps/res/codec/renderer/OTG/áudio) | parcial: baseline sem controles |
| 3.5 Camera | pendente (aba desabilitada por texto) |
| 3.6 Desktop | pendente |
| 3.7 Mic (source, meter, default source) | pendente |
| 3.8 Sessão + logs | **feito** (iniciar/parar, status, console 100 linhas) |
| 3.9 Behavior (reconnect/background/notificações) | pendente |
| 3.10 Modais (onboarding/atalhos/sobre/update) | pendente |
| 3.11 Integrações (tray/single-instance/hotkeys/drag-drop) | pendente |
| 3.12 Chrome/titlebar | pendente (decoração nativa) |

## 6. Próximos passos

1. Painel Mirror com os controles do core (`bitrate`, `fps`, `res`, `codec`,
   áudio, vsync) e persistência em `ScrcpyConfig`.
2. Wireless: IP/pairing usando `core::adb`/`core::wireless`.
3. Camera/Desktop/Mic com os campos que já existem no DTO.
4. Tray/single-instance/hotkeys (F2.8 `Platform` + F3.11).
5. Substituir o botão de ciclo de idioma pelo menu e remover a chave
   `modeUnavailable` quando as quatro abas estiverem implementadas.
