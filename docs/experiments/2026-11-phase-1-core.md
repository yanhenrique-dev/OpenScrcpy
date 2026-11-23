# Fase 1 — extração do core (progresso)

> **Status:** concluída em tudo que é mergeável. O `open-scrcpy-core` não
> conhece Tauri/GPUI; o monitor de sessão, o bridge de microfone e a geometria
> da janela externa saíram do casco. Restam apenas integrações que hoje são
> plugins do Tauri (tray, atalhos globais, drag do Windows) e o smoke manual
> com dispositivo.
> **Datas:** 2026-11-22 (primeira passada) · 2026-11-23 (retomada)
> **Branch:** `gpui/phase-1-core`
> **Plano:** [docs/GPUI-MIGRATION.md](../GPUI-MIGRATION.md)

## 1. O que foi extraído

Dois crates novos, ligados ao `src-tauri` por path dependency:

| Crate | Conteúdo | Depende de Tauri/GPUI? |
| --- | --- | --- |
| [`crates/open-scrcpy-core`](../../crates/open-scrcpy-core) | `validate`, `audio`, `capabilities`, `process` (inclui `force_kill_pid`), `download`, `state`, `events`, `adb`, `paths`, `terminal`, `wireless`, `session` (política), `config` (DTO + args/validação), `runtime` (estado retido + spawn/logs + retry/terminate), `mic` (sink virtual, rota e medidor), `monitor` (loop de sessão/reconexão), `scrcpy`, `report`, `update` | não |
| [`crates/open-scrcpy-platform`](../../crates/open-scrcpy-platform) | `kwin` (KDE Wayland/D-Bus), `window` (captura e reposicionamento da janela do scrcpy via Win32, xdotool/KWin e osascript) | não |

O `src-tauri` reexporta os módulos nos caminhos antigos, então o restante do
casco continua compilando com `crate::validate`, `crate::process` etc.

### 1.1 Fronteira do monitor

`core::monitor::run_session(SessionRequest)` recebe tudo o que é nativo já
resolvido pelo casco:

- `Arc<ScrcpyState>` e `Option<EventBus>` (estado e eventos);
- `video_dir` (o core não pergunta a um toolkit de UI onde ficam os vídeos);
- `Arc<dyn SessionHost>` (KWin, Win32 e `force_kill` entram por esse trait).

`commands::run_scrcpy` ficou com ~25 linhas: resolve `video_dir`, monta a
requisição com `TauriSessionHost` e delega. Nada de `window.emit`, nada de
`AppHandle` dentro da política de sessão.

### 1.2 Fronteira do microfone

`core::mic` é `unix`-only e recebe `Option<&EventBus>`. O casco só chama
`restore_default_source_sync`/`teardown_mic_sink` no `RunEvent::Exit`, que é
síncrono e não tem runtime.

### 1.3 Geometria da janela

`platform::window` concentra os três backends (`EnumWindows`/`ClientToScreen`,
`xdotool` + cache do KWin, `osascript`). O `TauriSessionHost` apenas encaminha.
O comando de recentralizar continua no casco porque lê o PID em
`ScrcpyState`.

## 2. Preservação de comportamento

| Evidência | Antes da extração | Depois |
| --- | --- | --- |
| Testes Rust do app | 95 passed, 2 ignored | 25 passed |
| Testes do core | — | 72 passed, 1 ignored |
| Testes da platform | — | 5 passed, 1 ignored |
| **Total Rust** | **95 passed, 2 ignored** | **102 passed, 2 ignored** |
| Frontend (Vitest) | — | 166 passed (33 arquivos) |
| `npm run lint` | — | 0 erros, 0 warnings |

Nenhum teste foi removido nem silenciado: os 10 testes que dependiam das
funções movidas foram para o crate de destino (`parse_shell_var_int` →
platform; regras de modo/áudio/janela e o filtro do sink → core).

Gates da retomada (2026-11-23):

- `cargo fmt --check` nos três pacotes — PASS
- `cargo clippy --all-targets -- -D warnings` nos três — PASS
- `cargo test` nos três — PASS (102 passed, 2 ignored)
- `npm run lint` — PASS
- `npm test` — PASS (166)
- `npm run tauri build -- --no-bundle` — PASS
- `npm audit --omit=dev --audit-level=high` — PASS
- `cargo audit --file src-tauri/Cargo.lock` — **FAIL pré-existente**:
  `RUSTSEC-2026-0594` (`cookie`) e `RUSTSEC-2026-0592` (`url`), herdadas do
  Tauri. O lock de `HEAD` produz exatamente os mesmos 2 erros e 6 avisos;
  nenhum advisory novo veio da extração.

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

### 3.2 O core não conhece janela nem AppHandle

`download::init_install_dir` recebe `Option<PathBuf>`; `monitor` recebe
`SessionHost`; `mic` recebe o `EventBus`; `state` não tem `Window`/`AppHandle`.
O único arquivo do casco que ainda chama `window.emit` é
[`src-tauri/src/events.rs`](../../src-tauri/src/events.rs).

### 3.3 EventBus sem perda de eventos

O `EventBus` usa `tokio::sync::mpsc` **unbounded** para preservar a ordem e o
volume que o `window.emit` tinha. O ring buffer + batch descrito no plano é
para a UI GPUI, onde o custo de repintura por linha importa; no Tauri atual o
evento já era enfileirado para a webview. Se o consumo da UI parar, o canal
cresce como a fila antiga crescia — trocar por um limite com política de
descarte fica para a fase de performance.

### 3.4 `Capabilities::for_test` é público

Os testes usam esse construtor em vários módulos. É um helper de teste
documentado, não um caminho de produção.

### 3.5 Base da branch

O plano pedia para partir de `main` com `AGENTS.md`/`.opencode/` já
versionados. A branch `gpui/phase-1-core` foi criada a partir de
`local/all-fixes`, que está 2 commits à frente de `main` (reconexão automática
e o ajuste do bridge de microfone). Isso é intencional — o core extraído
precisa refletir o código que está sendo shipado —, mas o PR deve declarar a
base e decidir se esses commits entram junto ou se a extração é reaplicada
sobre `main`.

### 3.6 Tray, atalhos e grab continuam no casco

`tray.rs` usa `tauri::tray`; `shortcuts.rs` usa
`tauri_plugin_global_shortcut`; `grab.rs` usa `AppHandle` e o plugin de
atalhos. Movê-los para `open-scrcpy-platform` exige uma abstração de tray e de
hotkeys que só se paga quando o GPUI substituir o casco (Fase 2). Tentar movê-los
agora criaria uma camada de indireção sem segundo consumidor — exatamente o
over-engineering que o plano manda evitar.

## 4. O que ainda falta para fechar a Fase 1

| Item | Situação |
| --- | --- |
| `commands.rs` (745 linhas, 25 testes) | só wrappers `#[tauri::command]` + `teardown_sessions` |
| `window.emit` / `AppHandle` como barramento | **concluído**: o único emissor é `events.rs` |
| loop `run_scrcpy`/monitor e reconexão | **concluído** → `core::monitor` |
| bridge de microfone (sink, rota, medidor) | **concluído** → `core::mic` |
| geometria da janela externa | **concluído** → `platform::window` |
| `grab.rs` (Windows), `shortcuts.rs`, `tray.rs` | permanecem no casco (plugins do Tauri; ver 3.6) |
| Workspace na raiz | adiado para o cutover |
| Contrato de eventos (`AppEvent`/`EventBus`) | concluído + teste de contrato |
| Smoke manual (espelhar, câmera, desktop, mic, wireless, arquivos, tray, atalhos, update) | **pendente** — exige dispositivo e o 2.0 fechado |

A entrega em runtime do adaptador não foi exercitada porque o plugin
single-instance impede abrir o binário de desenvolvimento com o OpenScrcpy 2.0
aberto. O `encode` do adaptador e o contrato dos seis eventos estão cobertos
por teste; falta o smoke com o 2.0 fechado.

## 5. Arquivos tocados

Core:

- [`lib.rs`](../../crates/open-scrcpy-core/src/lib.rs), [`state.rs`](../../crates/open-scrcpy-core/src/state.rs),
  [`validate.rs`](../../crates/open-scrcpy-core/src/validate.rs), [`audio.rs`](../../crates/open-scrcpy-core/src/audio.rs),
  [`capabilities.rs`](../../crates/open-scrcpy-core/src/capabilities.rs), [`process.rs`](../../crates/open-scrcpy-core/src/process.rs),
  [`download.rs`](../../crates/open-scrcpy-core/src/download.rs), [`events.rs`](../../crates/open-scrcpy-core/src/events.rs),
  [`adb.rs`](../../crates/open-scrcpy-core/src/adb.rs), [`paths.rs`](../../crates/open-scrcpy-core/src/paths.rs),
  [`terminal.rs`](../../crates/open-scrcpy-core/src/terminal.rs), [`wireless.rs`](../../crates/open-scrcpy-core/src/wireless.rs),
  [`session.rs`](../../crates/open-scrcpy-core/src/session.rs), [`config.rs`](../../crates/open-scrcpy-core/src/config.rs),
  [`runtime.rs`](../../crates/open-scrcpy-core/src/runtime.rs), [`scrcpy.rs`](../../crates/open-scrcpy-core/src/scrcpy.rs),
  [`report.rs`](../../crates/open-scrcpy-core/src/report.rs), [`update.rs`](../../crates/open-scrcpy-core/src/update.rs)
- novos: [`mic.rs`](../../crates/open-scrcpy-core/src/mic.rs), [`monitor.rs`](../../crates/open-scrcpy-core/src/monitor.rs)

Platform:

- [`kwin.rs`](../../crates/open-scrcpy-platform/src/kwin.rs), [`lib.rs`](../../crates/open-scrcpy-platform/src/lib.rs),
  [`Cargo.toml`](../../crates/open-scrcpy-platform/Cargo.toml) (dep `windows`)
- novo: [`window.rs`](../../crates/open-scrcpy-platform/src/window.rs)

Casco:

- [`commands.rs`](../../src-tauri/src/commands.rs), [`events.rs`](../../src-tauri/src/events.rs),
  [`session_adapter.rs`](../../src-tauri/src/session_adapter.rs), [`lib.rs`](../../src-tauri/src/lib.rs),
  [`download.rs`](../../src-tauri/src/download.rs), [`Cargo.toml`](../../src-tauri/Cargo.toml)
- [`.github/workflows/pull_request.yml`](../../.github/workflows/pull_request.yml) — CI dos crates
- `AGENTS.md`, `CONTRIBUTING.md`, `.github/pull_request_template.md`,
  `.opencode/skills/quality-workflow/SKILL.md` — caminho da validação

## 6. Build

`npm run tauri build -- --no-bundle` — **PASS** nas duas passadas. A retomada
re-executou o build depois de mover o monitor e o microfone.

## 7. Próximos passos

1. Smoke manual com o OpenScrcpy 2.0 fechado: espelhar, câmera, desktop, mic,
   wireless, push de arquivo, download, tray, atalhos e checagem de update.
2. Marcar a Fase 1 como fechada no plano e abrir o PR de refactor (movimentos
   separados de mudanças, diff revisável).
3. Na Fase 2, ao criar o casco GPUI, extrair tray/hotkeys via trait `Platform`
   — aí sim com um segundo consumidor real.
