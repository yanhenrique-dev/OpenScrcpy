# Fase 1 — extração do core (progresso)

> **Status:** parcial — domínio puro, EventBus e o núcleo de `adb`/devices
> concluídos; `commands.rs` ainda está no casco Tauri, mas não emite mais
> eventos direto na janela.
> **Data:** 2026-11-22
> **Branch:** `gpui/phase-1-core`
> **Plano:** [docs/GPUI-MIGRATION.md](../GPUI-MIGRATION.md)

## 1. O que foi extraído

Dois crates novos, ligados ao `src-tauri` por path dependency:

| Crate | Conteúdo | Depende de Tauri/GPUI? |
| --- | --- | --- |
| [`crates/open-scrcpy-core`](../../crates/open-scrcpy-core) | `validate`, `audio`, `capabilities`, `process`, `download`, `state`, `events`, `adb` (devices/connect/pair/kill/shell/push/install), `paths`, `terminal`, `wireless`, `session` (política), `config` (DTO + args/validação), `runtime` (estado retido + spawn/logs + retry/terminate), `scrcpy` (options/render drivers/versão), `report`, `update`, `download` (instalação) | não |
| [`crates/open-scrcpy-platform`](../../crates/open-scrcpy-platform) | `kwin` (KDE Wayland/D-Bus) | não |

O `src-tauri` reexporta os módulos nos caminhos antigos:

```rust
pub use open_scrcpy_core::state::{ScrcpyState, SessionHandle, StopRequest};
pub use open_scrcpy_core::{audio, capabilities, process, validate};
```

Assim, `commands.rs` continua compilando sem uma reescrita de ~4.6k linhas, e o
comportamento não muda. O único adaptador novo é
`src-tauri/src/download.rs`: ele resolve `app_data_dir()` e delega ao core
`init_install_dir(Option<PathBuf>)`.

## 2. Preservação de comportamento

| Evidência | Antes | Depois |
| --- | --- | --- |
| Testes Rust do app | 95 passed, 2 ignored | 35 passed no app |
| Testes do core | — | 63 passed, 1 ignored |
| Testes da platform | — | 4 passed, 1 ignored |
| **Total** | **95 passed, 2 ignored** | **102 passed, 2 ignored** |
| Frontend (Vitest) | — | 166 passed (33 arquivos) |
| `npm run lint` | — | 0 erros, 0 warnings |

Nenhum teste foi removido: os testes dos módulos moveram junto com o código.

Gates executados:

- `cargo fmt --check` nos três pacotes — PASS
- `cargo clippy --all-targets -- -D warnings` nos três — PASS
- `cargo test` nos três — PASS
- `npm run lint` — PASS
- `npm test` — PASS
- `npm run tauri build -- --no-bundle` — PASS (ver seção 6)
- `npm audit --omit=dev --audit-level=high` — PASS (0 vulnerabilidades)
- `cargo audit --file src-tauri/Cargo.lock` — **FAIL pré-existente**
  (`RUSTSEC-2024-0429`, `glib 0.18.5` via Tauri/GTK; a mesma versão está no
  `HEAD`, então o advisory não foi introduzido por esta extração). Não há
  upgrade sem mexer na stack GTK do Tauri.
- `cargo audit` do core — PASS (0 vulnerabilidades); o da platform só reporta
  avisos de manutenção (`zbus_names`, `zvariant`, sem upgrade disponível).

## 3. Decisões

### 3.1 Sem workspace na raiz (por enquanto)

O plano previa um workspace Cargo na raiz. Isso moveria o `Cargo.lock` e o
`target/` para a raiz, quebrando o cache e os comandos de release/`cargo audit`
que apontam para `src-tauri/`. Nesta fase, os crates são **path dependencies**
com workspace próprio; o lock autoritativo continua em `src-tauri/Cargo.lock`.

Consequência: `cargo test` no `src-tauri` não executa os testes dos crates
extraídos. O CI ganhou um passo explícito para eles, com
`CARGO_TARGET_DIR=src-tauri/target` para não compilar a árvore três vezes.

O workspace na raiz fica para o cutover (Fase 6), quando o Tauri sair.

### 3.2 O core não conhece janela

`download::init_install_dir` agora recebe `Option<PathBuf>`; quem resolve o
app-data é o casco. `state` não tem `Window`/`AppHandle`. `kwin` só fala D-Bus.

### 3.3 EventBus sem perda de eventos

O `EventBus` usa `tokio::sync::mpsc` **unbounded** para preservar a ordem e o
volume que o `window.emit` tinha. O ring buffer + batch descrito no plano é
para a UI GPUI, onde o custo de repintura por linha importa; no Tauri atual o
evento já era enfileirado para a webview. Se o consumo da UI parar, o canal
cresce como a fila antiga crescia — trocar por um limite com política de
descarte fica para a fase de performance.

### 3.4 `Capabilities::for_test` virou público

Os testes de `commands.rs` usam esse construtor. Como `commands.rs` ainda está
no casco, o helper precisou cruzar a fronteira. É um construtor de teste
documentado, não um caminho de produção.

## 4. O que ainda falta para fechar a Fase 1

| Item | Situação |
| --- | --- |
| `commands.rs` (~3.0k linhas, 35 testes) | ainda no `src-tauri` |
| `window.emit` / `AppHandle` como barramento | **concluído**: 96 emissões passam pelo `EventBus` |
| loop `run_scrcpy`/monitor, `grab`/`shortcuts`/`tray` | ainda no `src-tauri` |
| `grab.rs` (Windows), `shortcuts.rs`, `tray.rs` | ainda no `src-tauri` |
| Workspace na raiz | adiado para o cutover |
| Contrato de eventos (`AppEvent`/`EventBus`) | concluído + teste de contrato |

O núcleo de **adb/devices** também saiu: `parse_adb_device_line`,
`extract_device_model`, `parse_device_list`, `check_scrcpy`, `list_devices`,
`list_mdns_services`, `connect`, `pair`, `kill_server`, `shell`, `push_file`,
`install_apk` e `goto_wireless` vivem no core. `split_args`/limite de saída e
o executor do terminal também foram para `terminal.rs`; os wrappers Tauri
mantêm os mesmos nomes e argumentos de IPC.

Da sessão, a **política pura** saiu para `session.rs` (payload de término,
backoff, auto-reconnect, `dumpsys power`, posição da janela), a
**configuração/argumentos** saiu para `config.rs` e o **estado retido** do
monitor (`SessionRuntime`/`RetryContext`) e o encanamento de processo
(`spawn_scrcpy_streams` + leitores de log com `EventBus`), o término do
processo e o retry de codec saíram para `runtime.rs`. O KWin entra por uma
interface `SessionHost` (tracking, captura de janela e kill) implementada em
`src-tauri/src/session_adapter.rs`. `stop_scrcpy` e os finalizadores
(`emit_window_pos_and_stop`, `finish_stopped_session`,
`finish_if_session_gone`) também saíram. Gerações e liveness viraram métodos de
`ScrcpyState`. Só o loop e o `run_scrcpy` continuam no casco.

O **EventBus** foi concluído: `open_scrcpy_core::events` define `AppEvent` e
`EventBus`; `src-tauri/src/events.rs` é o único lugar que ainda chama
`window.emit` e reemite os seis nomes atuais (`scrcpy-log`, `scrcpy-status`,
`scrcpy-window-pos`, `scrcpy-audio-status`, `mic-route`, `mic-level`). As 96
emissões de `commands.rs` foram substituídas, e um teste puro valida nome +
payload de cada evento.

A entrega em runtime do adaptador não foi exercitada porque o OpenScrcpy 2.0
estava em execução e o plugin single-instance impede abrir o binário de
desenvolvimento. O `encode` do adaptador está coberto por teste; falta um
smoke com o 2.0 fechado.

## 5. Arquivos tocados

- [`crates/open-scrcpy-core/src/lib.rs`](../../crates/open-scrcpy-core/src/lib.rs)
- [`crates/open-scrcpy-core/src/state.rs`](../../crates/open-scrcpy-core/src/state.rs)
- [`crates/open-scrcpy-core/src/validate.rs`](../../crates/open-scrcpy-core/src/validate.rs)
- [`crates/open-scrcpy-core/src/audio.rs`](../../crates/open-scrcpy-core/src/audio.rs)
- [`crates/open-scrcpy-core/src/capabilities.rs`](../../crates/open-scrcpy-core/src/capabilities.rs)
- [`crates/open-scrcpy-core/src/process.rs`](../../crates/open-scrcpy-core/src/process.rs)
- [`crates/open-scrcpy-core/src/download.rs`](../../crates/open-scrcpy-core/src/download.rs)
- [`crates/open-scrcpy-core/src/events.rs`](../../crates/open-scrcpy-core/src/events.rs)
- [`crates/open-scrcpy-core/src/adb.rs`](../../crates/open-scrcpy-core/src/adb.rs)
- [`crates/open-scrcpy-core/src/paths.rs`](../../crates/open-scrcpy-core/src/paths.rs)
- [`crates/open-scrcpy-core/src/terminal.rs`](../../crates/open-scrcpy-core/src/terminal.rs)
- [`crates/open-scrcpy-core/src/wireless.rs`](../../crates/open-scrcpy-core/src/wireless.rs)
- [`crates/open-scrcpy-core/src/session.rs`](../../crates/open-scrcpy-core/src/session.rs)
- [`crates/open-scrcpy-core/src/config.rs`](../../crates/open-scrcpy-core/src/config.rs)
- [`crates/open-scrcpy-core/src/runtime.rs`](../../crates/open-scrcpy-core/src/runtime.rs)
- [`crates/open-scrcpy-core/src/scrcpy.rs`](../../crates/open-scrcpy-core/src/scrcpy.rs)
- [`crates/open-scrcpy-core/src/report.rs`](../../crates/open-scrcpy-core/src/report.rs)
- [`crates/open-scrcpy-core/src/update.rs`](../../crates/open-scrcpy-core/src/update.rs)
- [`src-tauri/src/session_adapter.rs`](../../src-tauri/src/session_adapter.rs)
- [`src-tauri/src/events.rs`](../../src-tauri/src/events.rs)
- [`crates/open-scrcpy-platform/src/kwin.rs`](../../crates/open-scrcpy-platform/src/kwin.rs)
- [`src-tauri/src/lib.rs`](../../src-tauri/src/lib.rs) — reexports
- [`src-tauri/src/download.rs`](../../src-tauri/src/download.rs) — adaptador
- [`src-tauri/Cargo.toml`](../../src-tauri/Cargo.toml) — path deps
- [`.github/workflows/pull_request.yml`](../../.github/workflows/pull_request.yml) — CI dos crates
- `AGENTS.md`, `CONTRIBUTING.md`, `.github/pull_request_template.md`,
  `.opencode/skills/quality-workflow/SKILL.md` — caminho da validação

## 6. Build

`npm run tauri build -- --no-bundle` — **PASS** (2026-11-22), re-executado
depois do estado em `Arc`. O Vite gerou o frontend em 6,05 s e o binário
release foi linkado em 35,18 s, terminando em
`src-tauri/target/release/openscrcpy`.

`zip`, `flate2`, `tar` e `sha2` saíram das dependências diretas do
`src-tauri` (agora vivem só no core).

## 7. Próximos passos

1. Introduzir `AppEvent` + `EventBus` no core, com um adapter Tauri que
   reemite `scrcpy-log` e `scrcpy-status` sem mudar o frontend.
2. Mover o restante de `commands.rs` por domínio (`session`/`run_scrcpy`,
   `files`, `update`, `mic`), mantendo wrappers finos. `adb`, `devices`,
   `wireless` e o terminal já saíram.
3. Mover `tray`/`shortcuts` para a platform crate e `grab` para a mesma camada
   com `cfg(windows)`.
4. Só então avaliar o workspace na raiz.
