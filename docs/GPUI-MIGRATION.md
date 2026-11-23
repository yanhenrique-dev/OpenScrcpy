# Plano de migração do frontend para GPUI Kit

> **Status:** rascunho para decisão e execução
> **Data:** 2026-11-22
> **Alvo:** `gpui-kit = "=0.10.0"` (pin exato; pré-1.0)
> **Escopo:** substituir Tauri 2 + Svelte 5 por um binário nativo Rust/GPUI
> **Documento irmão:** `AGENTS.md`, `.opencode/skills/quality-workflow/SKILL.md`

**TL;DR**

- Isto **não é um port de Svelte para GPUI**. É substituir o casco Tauri por um binário nativo e reescrever a UI em Rust; o Tauri deixa de existir.
- Custo realista: **25–32 semanas** para 1 dev sênior; ~17–20 semanas com 2 devs.
- **Não autorize tudo agora.** Autorize a Fase 0 (2–3 semanas), que prova ou refuta tray, updater, Vulkan, macOS 15, IME e RTL.
- Os bloqueadores mais prováveis: tray, updater assinado, exigência de Vulkan, macOS 15+ e o árabe/RTL.
- Se a motivação for só "UI mais moderna", um redesign Svelte é ~10× mais barato e não regride plataforma.

> **Execução:** a Fase 0 foi executada em 2026-11-22 neste repositório
> (`spike/gpui/`). Resultado: **GO condicional** para a Fase 1; veja
> [docs/experiments/2026-11-gpui-spike.md](experiments/2026-11-gpui-spike.md)
> para a matrix, números e bloqueios. A Fase 1 começou no mesmo dia e foi
> concluída em 2026-11-23: `validate`, `audio`, `capabilities`, `process`,
> `download`, `state`, `events`, `adb`, `wireless`, `terminal`, `session`,
> `config`, `runtime`, `scrcpy`, `mic`, `monitor` e `kwin`/`window` estão em
> `crates/`; `commands.rs` ficou só com wrappers finos. Falta apenas o smoke
> manual com dispositivo; veja
> [docs/experiments/2026-11-phase-1-core.md](experiments/2026-11-phase-1-core.md).

---

## 0. Para o agente que vai executar

Este documento é o contrato da migração. Leia nesta ordem:

1. `AGENTS.md` e `CONTRIBUTING.md` (regras do repositório).
2. `.opencode/skills/quality-workflow/SKILL.md` (workflow obrigatório).
3. As skills oficiais do Kit: `gpui-kit` e `gpui-kit-design-guides` (instale com a CLI `skills`, escopo do projeto, a partir de `longbridge/gpui-kit`; rode `npx skills --help` para a sintaxe exata e não copie à mão). Elas contêm as Coding Guides normativas.
4. Este plano, inteiro, antes de escrever código.

Em caso de conflito: as Coding Guides do Kit vencem na **forma de escrever código**; este plano vence no **escopo, produto e ordem de execução**.

Regras de execução:

- **Uma fase por vez.** Não comece a Fase *N+1* sem o portão de saída da Fase *N* registrado no relatório.
- **A Fase 0 é descartável e timeboxed.** O código dela vive em `spike/gpui/` e não entra em `main`.
- **A Fase 1 não muda comportamento.** Ela é um refactor verificável; o app Tauri continua sendo o produto.
- **Não porte CSS linha a linha.** Reconstrua a hierarquia com os componentes do Kit e os tokens do tema.
- **Não invente API.** Se o método não estiver em `https://docs.rs/gpui-kit/0.10.0` ou nos guias, prove com um spike mínimo antes de usar.
- **Não silencie gates** (`#[allow]`, teste removido, `unwrap` em caminho de comando).
- **Toda string visível** vai para os 8 locales; o teste de paridade é gate.
- **Todo valor que vira argumento de processo** continua passando por validação (ver `crates/open-scrcpy-core/src/validate.rs`).
- **Pare e reporte** se um critério de kill da Fase 0 for atingido, se um gate ficar vermelho por motivo ambiental, ou se a estimativa de uma fase estourar 40%.

Branch sugerida:

```
main
├── gpui/phase-0-spike      (descartável, não merge)
├── gpui/phase-1-core       (mergeável, sem mudança de comportamento)
├── gpui/phase-2-foundation
└── gpui/phase-3-...
```

Antes de começar, confirme que a árvore está limpa. No momento em que este plano foi escrito, a branch local era `local/all-fixes` e havia arquivos não rastreados (`AGENTS.md`, `.opencode/`, `skills-lock.json`); a migração deve partir de `main` com esses arquivos já versionados.

---

## 1. Decisão em uma página

### 1.1 O que foi pedido

Migrar todo o frontend para [GPUI Kit](https://gpui-kit.com/), um framework de UI nativa em Rust mantido pela Longbridge (Apache-2.0; 70 componentes estilizados + 40 primitivas; AccessKit; testes de UI integrados).

### 1.2 O que isso significa tecnicamente

Hoje o "frontend" não é um pacote independente: é uma **webview Svelte 5** dentro do casco Tauri, conversando com o Rust por IPC. O GPUI Kit não é uma biblioteca de componentes para webview; é um framework de aplicação nativa, com janela, renderer, foco, acessibilidade e loop de eventos próprios.

Não existe caminho suportado para:

- colocar GPUI dentro da webview do Tauri; ou
- manter o Tauri como casco e trocar só a camada visual.

Portanto, "migrar o frontend" significa, na prática:

> **substituir o casco Tauri por um binário GPUI e reescrever a UI em Rust.**

O Tauri, o `npm`, o Vite, o Tailwind, o CSP, as capabilities e o IPC deixam de existir no produto final. O que permanece é a lógica de domínio: `scrcpy`, `adb`, downloads, validação de argumentos, áudio, rastreio da janela externa do scrcpy e o modo background.

### 1.3 Recomendação

**Não autorize a reescrita completa agora.** Autorize a **Fase 0** (2–3 semanas, timeboxed). Ela responde com código às incógnitas que podem inviabilizar o projeto — tray, updater, Vulkan, macOS 15, IME, RTL, empacotamento. Descobrir isso na Fase 3 custa meses; descobrir na Fase 0 custa semanas.

Se a Fase 0 passar, o caminho é Fase 1 → 2 → 3 → 4 → 5 → 6, com entregas intermediárias e o app Tauri ainda funcionando até o cutover.

Se o objetivo real for "UI mais bonita/moderna", **este não é o caminho**: um redesign em Svelte custa uma fração e não regride plataforma, acessibilidade nem distribuição.

### 1.4 O que este plano não é

- Não é um porta 1:1 de componentes Svelte.
- Não é uma migração incremental via webview: o `gpui-webview` do Kit é experimental, no Linux só funciona em X11/XWayland e não resolve tray/updater; não use como ponte arquitetural.
- Não é uma migração de vídeo: o espelhamento continua sendo uma janela externa do `scrcpy`, não renderizada pelo GPUI.
- Não é uma reescrita da lógica de `scrcpy`/`adb`; ela é extraída, não reescrita.
- Não é um compromisso de data; as estimativas são faixas para um dev sênior.

---

## 2. Baseline verificado

| Item | Número / fato |
| --- | --- |
| Componentes Svelte | 38 arquivos, 3.855 linhas (36 de produto + 2 fixtures de teste) |
| Lógica TypeScript | ~2.700 linhas em `src/lib` fora testes (stores, config, scrcpy, window state) |
| i18n | 8 locales (`en`, `fr`, `pt-BR`, `zh-CN`, `zh-TW`, `ru`, `id`, `ar`), 390 chaves por locale (paridade atual), fallback profundo |
| Comandos Tauri | 24 registrados em `generate_handler!`; ~20 invocados pela UI |
| Backend Rust | `commands.rs` com ~5.4k linhas e 70 testes; 27 testes nos demais módulos |
| Testes de frontend | 30 arquivos, 131 blocos `it`: 14 arquivos de lógica (77) + 16 de DOM/a11y (54) |
| Plugins Tauri | `single-instance`, `process`, `updater`, `opener`, `dialog`, `notification`, `global-shortcut`; feature `tray-icon` |
| Distribuição atual | Linux AppImage/DEB/RPM, macOS universal `.app`/DMG sem assinatura, Windows NSIS/MSI; updater minisign com `latest.json` |
| Janela Linux | `decorations: false` + botões e 8 alças de resize desenhados na webview |
| Espelhamento | Janela **externa** do `scrcpy`, reposicionada via `xdotool`, KWin/D-Bus (KDE Wayland) ou Win32 |
| Tema | Tailwind 4 + variáveis CSS; Inter variable; ícones Lucide |
| Acessibilidade | Testes axe por componente; foco/Escape/roving tabindex manuais |

Consequência importante: como o vídeo não é renderizado pelo app, o ganho de performance do GPUI é **modesto** (UI de configuração), e o risco de regressão é **alto** (casco inteiro). O plano precisa ser justificado por produto, não por benchmark.

### 2.1 Fatos do alvo (GPUI Kit 0.10.0)

| Item | Fato |
| --- | --- |
| Release | 0.10.0 em 2026-11-16; cadência semanal; todas 0.x, com seções de breaking changes |
| Dependência | `gpui-pre = 0.3.12` pinado exato; subir o Kit move o snapshot do GPUI junto |
| Toolchain | Rust 1.92+; edition 2024 |
| Renderers | Linux Vulkan; macOS Metal; Windows Direct3D 11 — sem fallback de software documentado |
| SO | Linux Wayland/X11, Windows 10+, macOS 15+ |
| Componentes | 70 estilizados + 40 primitivas; `Settings`/`SettingPage`/`SettingField`; 38 presets de tema |
| Acessibilidade | AccessKit em AT-SPI, macOS e UIA; cobertura "partial" (gaps na seção 14.3) |
| Testes | `#[gpui_kit::test]` headless nos 3 SO; suite de pixels separada |
| Skills oficiais | `gpui-kit` e `gpui-kit-design-guides` no repo `longbridge/gpui-kit`, com Coding Guides normativas |
| Licença | Apache-2.0 (código); docs CC BY 4.0 |

---

## 3. Arquitetura alvo

```
┌──────────────────────────────────────────────────────────────┐
│ crates/open-scrcpy-gpui (binário openscrcpy)                  │
│  Views GPUI, tema, i18n, assets, tray, janelas, updater       │
│  ─ chama a API pública do core; nunca fala com scrcpy direto  │
└───────────────▲──────────────────────────────────────────────┘
                │ chamadas de função + eventos
┌───────────────┴──────────────────────────────────────────────┐
│ crates/open-scrcpy-core (sem tauri, sem gpui)                  │
│  AppState, sessões, adb, scrcpy args, download, validação,    │
│  áudio, EventBus (logs/status/progresso), testes de unidade   │
└───────────────▲──────────────────────────────────────────────┘
                │
┌───────────────┴──────────────────────────────────────────────┐
│ crates/open-scrcpy-platform (cfg por SO)                       │
│  KWin/D-Bus, xdotool, Win32 grab, tray, hotkeys, single-inst, │
│  diálogos, notificações, paths, updater                       │
└──────────────────────────────────────────────────────────────┘

Durante a transição:
src-tauri/  →  vira um adaptador fino: #[tauri::command] → core + EventBus → window.emit
```

Regras de dependência (gate de review):

1. `core` não importa `tauri`, `gpui`, `gtk` nem `windows`.
2. `gpui` não chama `std::process::Command` para `adb`/`scrcpy`; chama `core`.
3. `platform` é a única camada com `#[cfg(target_os)]` de integração nativa.
4. Nenhum `unwrap()`/`expect()` em caminho de comando; erros viram `Result<_, AppError>`.

### 3.1 Barramento de eventos

O ponto mais delicado da extração é que hoje o backend emite eventos direto na janela:

```rust
window.emit("scrcpy-log", line)
window.emit("scrcpy-status", payload)
```

No alvo, o core publica em um barramento e cada casco consome:

```rust
pub enum AppEvent {
    Log { device: Option<String>, line: String, level: LogLevel },
    SessionStatus(SessionStatus),
    Download(DownloadEvent),
    DevicesChanged(Vec<String>),
}
```

- `tokio::sync::broadcast` para eventos de baixa frequência (status, devices, download).
- **Logs de alta frequência não vão um a um para a UI.** O core mantém um ring buffer (1.000–5.000 linhas para diagnóstico) + `watch` de revisão; a UI lê o snapshot e faz batch por frame. O painel pode manter o teto atual de 100 linhas (ou tornar configurável). Isso evita repaint storm, que é o risco de performance mais provável.
- O adaptador Tauri consome o barramento e reemite com os nomes/payloads atuais, preservando a UI Svelte durante a Fase 1.

### 3.2 Ownership de estado

- `Arc<AppState>` no core, com `Mutex`/`RwLock` de escopo curto e `tokio::runtime::Handle`.
- A view GPUI é dona do estado de apresentação (aba ativa, texto de input, modal aberto, scroll), não do estado de sessão.
- `Entity<T>` para views retidas; `RenderOnce` para pedaços reutilizáveis; nenhum estado global mutável fora de `Global` explícito.

---

## 4. Portões de decisão (go/no-go)

Estas são as decisões que a Fase 0 precisa fechar. Cada uma pode mudar escopo, produto ou inviabilizar a migração.

| # | Questão | Fato verificado | Se falhar |
| --- | --- | --- | --- |
| D1 | **macOS 15+** | Docs do GPUI Kit exigem macOS 15 ou later. O app atual não declara `minimumSystemVersion` e o Tauri 2 roda em versões bem anteriores. | Ou aceita perder usuários antigos, ou mantém uma build Tauri 2.x paralela, ou cancela. |
| D2 | **Vulkan no Linux** | GPUI exige sessão gráfica com driver Vulkan; `libvulkan1` sozinho não basta. O app atual roda sobre WebKitGTK/GL. | Testar em Intel antiga, NVIDIA, AMD, VMs e `lavapipe`. Se a cobertura for ruim, manter Linux no Tauri ou exigir fallback. |
| D3 | **Tray / background mode** | O GPUI Kit **não tem** API de tray; há só um pedido aberto ([#3843](https://github.com/longbridge/gpui-kit/issues/3843)), sem recomendação oficial. Hoje o tray usa libayatana-appindicator via Tauri. | Spike com `ksni` (D-Bus/StatusNotifierItem) e `tray-icon` (GTK). Sem tray, o modo background perde o affordance de restaurar/sair. |
| D4 | **Updater assinado** | O GPUI Kit **não** fornece updater: a aplicação e a distribuição são donas da política, verificação, instalação e rollback. Hoje há minisign + `latest.json` + artefatos assinados. | Reimplementar cliente (minisign/`self_update`) ou descartar update in-app — regressão de produto. |
| D5 | **Empacotamento** | O Kit entrega o app; não há bundler oficial. A doc cobre `.deb`/`.rpm`/tar.gz, `.app`/DMG com `codesign`+`notarytool` e **Inno Setup** no Windows — sem receita de AppImage nem MSI. | Avaliar `cargo-packager`/scripts; se não cobrir AppImage + Inno + DMG, o esforço da Fase 5 cresce. |
| D6 | **Árabe / RTL** | O Kit **não** espelha a interface ao trocar o locale; ele só troca strings. Hoje o app usa `dir="rtl"` no `<html>` e o CSS espelha a maior parte, mas há overrides físicos (`padding-left`, `border-right`, `right:2px`, tooltip `left-1/2`) que já quebram o espelhamento. | O alvo realista é "não pior que hoje": implementar direção no layout, revisar tela a tela e aceitar as mesmas exceções. Orçar 2–3 semanas. |
| D7 | **Chrome customizado no Linux** | Hoje `decorations: false`, botões próprios, 8 alças de resize e arraste por `startDragging()`. O Kit suporta `WindowDecorations::Client` + `TitleBar` + `WindowBorder` (banda de resize de 4 px). | Manter o chrome customizado é viável; o theming nativo do titlebar no Linux ainda tem issue aberto ([#3994](https://github.com/longbridge/gpui-kit/issues/3994)). |
| D8 | **IME / CJK** | Há locales `zh-CN`/`zh-TW`; inputs de IP, busca e pairing precisam de IME. O GPUI implementa `EntityInputHandler` (preedit/commit), mas a doc do Kit admite que o input "não modela composição completa" e há bugs abertos (Pinyin no macOS [#3605](https://github.com/longbridge/gpui-kit/issues/3605), dead keys [#3456](https://github.com/longbridge/gpui-kit/issues/3456)). | Se quebrar, usuários chineses não conseguem digitar; bloquear release até resolver. |
| D9 | **Base de instalação Linux** | A CI atual compila em Ubuntu 22.04; a doc do Kit valida Ubuntu 24.04 e depende de Vulkan. | Definir matrix mínima (24.04? 22.04?) e testar em VM limpa. |
| D10 | **Hotkeys globais e instância única** | Nenhum dos dois existe no GPUI/Kit: `KeyBinding` só vale com a janela focada; não há API de single-instance. | Usar `global-hotkey` e `single-instance`; são os mesmos crates que os plugins do Tauri usam por baixo. |

**Regra:** D1, D2, D3, D4, D6 e D8 são bloqueantes de escopo. Nenhuma delas pode ficar "para depois".

---

## 5. Fase 0 — Spike de viabilidade (2–3 semanas)

**Objetivo:** provar ou refutar as integrações de risco em código descartável, sem tocar no produto.

**Onde:** branch `gpui/phase-0-spike`, diretório `spike/gpui/` (workspace Cargo independente). Não altere `src/` nem `src-tauri/`.

### Tarefas

| ID | Tarefa | Critério de aceite |
| --- | --- | --- |
| S0.0 | Instalar as skills oficiais (`gpui-kit`, `gpui-kit-design-guides`) com a CLI `skills` no escopo do projeto; ler as Coding Guides | Skills em `.agents/skills/` + `skills-lock.json`; agente cita a guide usada |
| S0.1 | Skeleton: janela 1200×890, tema claro/escuro, Inter, `Button`/`Input`/`Select`, 3 telas falsas | `cargo run` abre e navega; sem warnings de clippy |
| S0.2 | Matrix de SO: KDE Wayland, GNOME Wayland, X11, Windows 10/11, macOS 15 arm64 + x86_64 | Tabela com versão de SO, driver, resultado, tempo de startup e RSS; sem hardware Intel, marcar UNKNOWN e tratar como R13 |
| S0.3 | Tray: `ksni` no Linux; `tray-icon` no Windows/macOS; menu com Mostrar/Sair; esconder/restaurar | Funciona em KDE e GNOME (com extensão AppIndicator), Windows e macOS; documentar dependências e o loop GTK/SNI |
| S0.4 | Serviços: `single-instance`, `global-hotkey`, diálogos nativos (`cx.prompt_for_paths`/`prompt_for_new_path`, portal/ashpd), drag-and-drop de arquivos, `SystemNotification` com app identity | Cada serviço com teste manual e log; notificação aparece no centro do SO; sem GTK extra no Linux se o portal bastar |
| S0.5 | Processo: iniciar um processo longo falso e despejar 1.000 linhas/s em uma lista GPUI | UI não congela; memória estável; estratégia de batch definida |
| S0.6 | Janela externa: abrir `scrcpy --version`, localizar janela por PID (xdotool/KWin), reposicionar | Mesma lógica do app atual continua funcionando a partir do binário GPUI |
| S0.7 | Updater: ler `latest.json` local, verificar assinatura minisign, baixar artefato dummy, substituir cópia em `/tmp`, reiniciar | Fluxo completo em Linux com artefato dummy; AppImage real na S0.10; Windows/macOS com estratégia documentada |
| S0.8 | i18n: `rust-i18n` com `en`, `pt-BR`, `ar`, `zh-CN`; troca em runtime; input CJK; texto árabe | Strings trocam; CJK renderiza; árabe tem shaping/bidi correto; limitações de RTL registradas |
| S0.9 | Teste: um `#[gpui_kit::test]` com role/label/foco; rodar em CI headless (sem display) | Teste passa nos 3 SO; se houver teste de pixel, documentar Vulkan/Metal e o skip no Windows |
| S0.10 | Pacote: `.deb` e AppImage montados à mão (ou `cargo-packager`) e abertos em VM limpa | Instala, abre, desinstala; dependências de runtime listadas |
| S0.11 | Relatório GO/NO-GO | Matrix, números, riscos e recomendação assinada |

### Relatório de saída

Crie `docs/experiments/2026-11-gpui-spike.md` com:

1. Matrix de resultados por SO (PASS/FAIL/PASS com ressalva).
2. Screenshots (claro/escuro) de cada SO.
3. Números: startup, RSS, uso de GPU, tamanho do binário/pacote.
4. Lista de crates adicionais e versões.
5. Riscos remanescentes e recomendação **GO / NO-GO / GO com escopo reduzido**.

Use esta matrix (preencha com PASS/FAIL e uma nota curta):

| SO / sessão | build | abre | tray | hotkey | diálogo | notificação | IME | teste UI | pacote |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| KDE Wayland | | | | | | | | | |
| GNOME Wayland | | | | | | | | | |
| X11 | | | | | | | | | |
| Windows 11 | | | | | | | | | |
| Windows 10 | | | | | | | | | |
| macOS 15 arm64 | | | | | | | | | |
| macOS 15 x86_64 | | | | | | | | | |

### Critérios de kill

- Tray não funciona em KDE **e** GNOME sem hack de loop GTK.
- Vulkan indisponível em uma parcela relevante da base (definir com o dono; sugestão: >10% dos usuários ativos ou qualquer distro suportada).
- Updater não consegue substituir o AppImage com segurança e não há plano B aceitável.
- IME quebrado para `zh-CN`/`zh-TW`.
- Acessibilidade por teclado pior que a atual em fluxo crítico (pareamento, iniciar sessão).
- Crash ou freeze reproduzível no fluxo de logs.

**Portão:** decisão registrada pelo dono do produto. Sem GO explícito, a Fase 1 não começa.

---

## 6. Fase 1 — Extrair o core (4–5 semanas, mergeável)

> **Execução (2026-11-22 → 2026-11-23):** o core, o `EventBus`, o monitor de
> sessão, o bridge de microfone e a geometria da janela externa saíram do
> casco com o mesmo número de testes (102 passed, 2 ignored). `commands.rs`
> agora só tem wrappers `#[tauri::command]`; tray/atalhos/grab continuam no
> casco por serem plugins do Tauri (abstração fica para a Fase 2). Ver
> [docs/experiments/2026-11-phase-1-core.md](experiments/2026-11-phase-1-core.md).

**Objetivo:** separar domínio de UI sem mudar comportamento. Esta fase tem valor mesmo se o GPUI for abandonado: o core fica testável sem webview.

### Passos

1. Criar workspace na raiz com `crates/open-scrcpy-core` e `crates/open-scrcpy-platform`; `src-tauri` passa a depender do core. *(Adiado para o cutover: nesta fase os crates são path dependencies para não mover o lock/target do Tauri.)*
2. Mover, nesta ordem, sem reescrever lógica:
   - `validate.rs` → `core::validate`
   - `process.rs` → `core::process`
   - `download.rs` → `core::download`
   - `capabilities.rs` → `core::capabilities`
   - `audio.rs` → `core::audio` (classificação de falha/codec é domínio, não plataforma)
   - `audio.rs` (parte de sink/dispositivo, se separar) → `platform::audio` (Linux)
   - `kwin.rs` → `platform::kwin`
   - `grab.rs` → `platform::grab` (Windows)
3. Quebrar `commands.rs` por domínio: `adb`, `devices`, `wireless`, `session`, `scrcpy_args`, `mic`, `update`, `files`.
4. Substituir `tauri::State<'_, ScrcpyState>` por `Arc<AppState>` e `tauri::Window` por `EventBus`.
5. Manter wrappers `#[tauri::command]` finos: recebem argumentos, chamam o core, devolvem o mesmo JSON.
6. O adaptador Tauri assina o barramento e reemite `scrcpy-log`, `scrcpy-status` e progresso com os nomes atuais.
7. Funções que hoje recebem `Window` para diálogo/progresso (`download_scrcpy`, `save_report`) viram `core` + `Platform` no adaptador; o core devolve dados, não abre janela.
8. Ajustar CI/cache para o workspace (`cargo test --workspace`); se o workspace na raiz destabilizar o Tauri CLI, usar workspace separado com path deps e registrar a decisão.

### Critérios de aceite

- `cargo tree -p openscrcpy-core` não contém `tauri` nem `gpui`.
- Nenhum `window.emit` fora do adaptador Tauri.
- Testes existentes continuam passando; os 70 testes de `commands.rs` migram para o core ou para o adaptador, sem deleção.
- Smoke manual idêntico: espelhar, câmera, desktop, mic, wireless, push de arquivo, download, tray, atalhos, checagem de update.
- `npm run lint`, `npm test`, `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test` verdes.
- Nenhuma string visível nova; nenhum comportamento alterado.

**Portão:** PR de refactor aprovado com diff revisável (movimentos separados de mudanças) e app Tauri 2.x ainda shippável.

---

## 7. Fase 2 — Fundação do app GPUI (3–4 semanas)

> **Execução parcial (2026-11-23):** `crates/open-scrcpy-gpui` existe com o
> shell (header, device hub, abas de modo e console), tema claro/escuro/sistema,
> troca dos 8 locales em runtime, settings versionados com escrita atômica e
> bounds de janela restaurados. O pipeline TS→YAML e o teste de paridade cobrem
> as 390 chaves nos 8 locales. Faltam fontes/ícones, a trait `Platform`, os
> overlays e o panic hook; veja
> [docs/experiments/2026-11-phase-2-foundation.md](experiments/2026-11-phase-2-foundation.md).

> **Execução parcial (2026-11-23):** a primeira fatia vertical liga o app ao
> core: device hub com polling de `adb`, seleção, iniciar/parar espelhamento e
> console alimentado pelo `EventBus`, sem Tauri. Ver
> [docs/experiments/2026-11-phase-3-vertical-slice.md](experiments/2026-11-phase-3-vertical-slice.md).

**Objetivo:** ter um casco vazio, porém completo: tema, fontes, ícones, i18n, settings, janela, overlays e serviços.

| ID | Entrega | Detalhe |
| --- | --- | --- |
| F2.1 | Shell | `Root`, header, sidebar, main, log panel; layout responsivo equivalente ao grid atual |
| F2.2 | Tema | 50+ tokens de `src/index.css` → `Theme` do Kit; superfícies `color-mix`, focus ring de 2 px, scrollbars, claro/escuro/sistema; persistência |
| F2.3 | Tipografia | Inter embutida; fallback do sistema para CJK/árabe; escala 100–200% |
| F2.4 | Ícones | Inventário dos ~40 ícones Lucide; usar `Icon` do Kit ou SVG embutido; nada de raster |
| F2.5 | i18n | Script TS→YAML, 8 locales, paridade de chaves e placeholders em teste Rust, troca em runtime |
| F2.6 | Settings | `AppSettings` com `serde(default)`, versão de schema, escrita atômica, validação na leitura, paths via `directories` |
| F2.7 | Janela | Restaurar bounds com `WindowBounds`, checar monitores, salvar em move/resize, maximizar |
| F2.8 | Serviços | Trait `Platform` (diálogos, notificações, opener, tray, hotkeys, single-instance, updater) + fake para testes |
| F2.9 | Alertas | `Notification`/`Dialog` do Kit para substituir `AlertHost`, `Modal`, `ThemedModal`, `SheetModal`; foco e Escape |
| F2.10 | Erros | `panic::set_hook` + tela de erro + relatório de diagnóstico (substitui `ErrorBoundary`) |

### Critérios de aceite

- App abre, troca os 8 locales e os 3 modos de tema sem reiniciar.
- Settings e posição da janela sobrevivem a restart.
- Nenhuma string hardcoded; teste de paridade verde.
- Nenhum `localStorage`, `navigator`, `window` ou dependência npm no binário.
- Testes de UI do shell (foco, Tab, Escape) passando.

---

## 8. Fase 3 — Telas por fatia vertical (8–10 semanas)

Cada fatia entrega UI + ligação com o core + testes + i18n + acessibilidade + screenshot claro/escuro + smoke nos 3 SO. Não abra a próxima antes do aceite da anterior.

| Ordem | Fatia | Escopo | Teste-chave |
| --- | --- | --- | --- |
| 3.1 | Header | versão, update badge, tema, idioma, menu do engine, botões de janela | roving focus do menu de idioma; Escape devolve foco |
| 3.2 | Device hub | lista, refresh, kill adb, abas USB/wireless, estado desconectado | seleção por teclado; polling só com janela visível/background/sessão (eventos de ativação/oclusão do GPUI) |
| 3.3 | Wireless | IP, código de pareamento, histórico, erros | validação de IP/porta; erro do adb vira mensagem traduzida |
| 3.4 | Mirror | bitrate, fps, resolução, codec, rotação, renderer, OTG, áudio | args idênticos ao core atual; subtree de performance desabilitado no OTG sem `inert` (use disabled + skip de foco) |
| 3.5 | Camera | facing, lente, torch, zoom, AR, high-speed, mic sink | estados desabilitados por capability; câmera inexistente |
| 3.6 | Desktop | largura/altura/DPI, flex, rotação, res | valores inválidos bloqueados antes do processo |
| 3.7 | Mic | source, codec, bitrate, denoise, meter, default source | meter sem alocar por frame; parar restaura source |
| 3.8 | Sessão + logs | iniciar/parar/reconectar, progresso, log com autoscroll/pausa/limpar/relatório | 1k linhas/s sem travar; batch por frame; teto de 100 visível |
| 3.9 | Behavior | auto-reconnect, background, notificações, posição, vsync | tray + background + fechar janela |
| 3.10 | Modais | onboarding, atalhos, sobre, update, confirmações | focus trap, `data-autofocus` precedence, z-tiers, Escape, retorno de foco |
| 3.11 | Integrações | tray, single-instance, hotkeys, drag-drop, file dialogs | segundo launch restaura janela; atalho global funciona |
| 3.12 | Chrome | decoração nativa ou titlebar custom + resize (decisão D7) | arrastar, maximizar, resize em 100/125/150% |

**Portão da fase:** paridade funcional com o app 2.x no checklist de features do README; nenhuma feature removida sem decisão registrada.

---

## 9. Fase 4 — Paridade de plataforma e acessibilidade (3–4 semanas)

| Área | O que validar | Critério |
| --- | --- | --- |
| Linux KDE Wayland | KWin/D-Bus, reposicionar janela, tray, atalhos, escala fracionária | fluxo completo sem xdotool |
| Linux GNOME Wayland | tray (extensão), notificações, foco, IME | fluxo completo; documentar dependências |
| Linux X11 | xdotool, janela borderless, resize | paridade com 2.x |
| Windows 10/11 | DPI, Win32 grab/recenter, instalador, tray, notificações | 100/125/150/200% sem corte |
| macOS 15+ | `.app`, permissão de notificação, menu bar, universal arm64/x86_64 | abre em ambas as archs; Gatekeeper documentado |
| IME | `zh-CN`, `zh-TW`, composição em IP/busca/pairing | digitar, colar, cancelar composição |
| RTL | árabe: navegação, alinhamento, ícones direcionais, ordem de foco | decisão D6 aplicada e testada tela a tela |
| Acessibilidade | Orca (Linux), NVDA (Windows), VoiceOver (macOS), teclado-only | nomes/roles/estados corretos; nenhum controle inalcançável |
| Performance | startup, RSS, throughput de logs, CPU em idle | sem regressão relevante vs. 2.x; números no relatório |

**Portão:** matrix de plataformas assinada; nenhum bloqueador de acessibilidade; RTL com decisão explícita.

---

## 10. Fase 5 — Empacotamento, updater e release (3–4 semanas)

### 10.1 Empacotamento

| Plataforma | Formato mínimo | Ferramenta provável |
| --- | --- | --- |
| Linux | AppImage + `.deb` (+ `.rpm` se mantiver a promessa) | script próprio ou `cargo-packager`; validar em VM limpa |
| Windows | Instalador Inno Setup + ZIP portátil | script/CI; decidir assinatura de código (hoje não há). MSI/NSIS não têm receita no Kit. |
| macOS | `.app` universal em DMG | `lipo` + `codesign`/`notarytool` se houver Developer ID; senão, documentar Gatekeeper |

Requisitos de runtime que entram no README e no instalador: **Vulkan**, `libxkbcommon`, sessão Wayland/X11, macOS 15+.

### 10.2 Updater

Hoje: minisign + `latest.json` + `.sig`, e o Tauri instala. No GPUI:

1. Reutilizar a chave minisign e o contrato de release, mas **com um manifesto novo** (`latest-gpui.json`), porque o payload muda.
2. Linux/AppImage: baixar, verificar assinatura + SHA-256, substituir fora do processo vivo, reiniciar.
3. Windows: baixar o instalador e executá-lo (o instalador é o dono da substituição).
4. macOS: substituir o `.app` completo (não só o binário) ou abrir o DMG; idealmente Sparkle, mas isso adiciona dependência e assinatura.
5. **Congelar `latest.json` na linha 2.x.** Clientes antigos não podem receber um artefato GPUI — o updater deles instalaria um bundle incompatível. Publicar um 2.9.x "ponte" que avisa "3.0 disponível para download manual" antes do cutover.

### 10.3 CI

- Matrix: Ubuntu 24.04 (build + testes), Windows, macOS 15 (universal).
- **Construir na distro mais antiga que você suporta** (hoje 22.04) para não elevar a glibc mínima; só suba para 24.04 se as dependências do Kit exigirem, e declare o novo mínimo no README.
- Testes de UI: `#[gpui_kit::test]` é headless e roda nos 3 SO sem display; o suite de pixels (opcional) exige Metal/Vulkan e é skipado no Windows.
- Cache de Cargo; `cargo audit`; checksums, SBOM e assinaturas; remover `npm audit`, Node e Vite.
- Smoke de instalação em VM/container por formato.

**Portão:** instalar, atualizar e desinstalar em cada SO; update 3.0.0 → 3.0.1; nenhuma elevação inesperada.

---

## 11. Fase 6 — Cutover e limpeza (2 semanas)

1. Remover `src/`, `src-tauri/`, `package.json`, `package-lock.json`, `node_modules/`, `dist/`, `vite.config.ts`, `svelte.config.js`, `tsconfig*.json`.
2. Arquivar a linha 2.x em branch/tag (`v2.x`) por pelo menos 6 meses.
3. Atualizar:
   - `AGENTS.md` (stack, gates, i18n em Rust, validação, remoção de CSP/capabilities).
   - `CONTRIBUTING.md`, `.github/pull_request_template.md`.
   - `.opencode/skills/quality-workflow/SKILL.md` (não referencia mais Svelte/Tauri/Vitest).
   - `README.md` (requisitos, screenshots, instalação) e `GUIDE.md`.
   - `docs/RELEASING.md` (novos formatos e updater).
4. Publicar `3.0.0` com notas de migração e aviso de requisitos (macOS 15, Vulkan).
5. Monitorar issues por 4 semanas; hotfixes na 2.x só para segurança/crash.

---

## 12. Mapa de substituições

| Hoje (Tauri/web) | Alvo (GPUI) | Observação |
| --- | --- | --- |
| `#[tauri::command]` + `invoke()` | chamada direta à API do core | sem IPC; validar entradas mesmo assim |
| `window.emit` / `listen` | `EventBus` + ring buffer de logs | batch por frame |
| `@tauri-apps/api/window` | APIs de `Window` do GPUI | drag/maximize/restore: confirmar método exato no spike |
| `plugin-dialog` | `cx.prompt_for_paths` / `cx.prompt_for_new_path` (portal/ashpd) | `rfd` só como fallback se o portal não cobrir |
| `plugin-notification` | `SystemNotification` / `Notification::system()` | app identity; sem replace no Linux |
| `plugin-opener` | `cx.open_url` | sem shell |
| `plugin-updater` | cliente próprio + minisign/`self_update` | ver 10.2 |
| `plugin-single-instance` | crate `single-instance` | mesma semântica |
| `plugin-global-shortcut` | crate `global-hotkey` | registrar na thread principal |
| `tray-icon` (Tauri) | `tray-icon` ou `ksni` | decisão D3 |
| `localStorage` | JSON em config dir, escrita atômica | migração de dados: ver 12.1 |
| CSP + capabilities | removidos | superfície menor; validação de args permanece |
| Tailwind + CSS vars | tokens de tema + estilos `div()` | sem CSS |
| Svelte runes/stores | `Entity`, `Context`, `Global` | sem stores legados |
| `@lucide/svelte` | `Icon` do Kit / SVG | inventário de ícones |
| i18n em TS (bundles + deep merge) | `rust-i18n` + YAML | ver seção 13 |
| Vitest + RTL + axe | `#[gpui_kit::test]` + AccessKit + manual | ver seção 14 |
| `ErrorBoundary.svelte` | panic hook + error view | relatório de diagnóstico |

### 12.1 Migração de dados do usuário

Config, locale, tema, histórico wireless e posição de janela vivem em `localStorage`. No GPUI, irão para `~/.config/...` (Linux), `%APPDATA%` (Windows) e `~/Library/Application Support` (macOS).

Formatos antigos: SQLite (WebKitGTK/WKWebView) e LevelDB (WebView2). Migração automática é frágil. Recomendação:

- **Não migrar automaticamente** no 3.0; começar com defaults e onboarding curto.
- Oferecer importação opcional apenas no Linux (SQLite do WebKitGTK) se houver demanda.
- Nunca ler o armazenamento antigo como confiável: validar tudo com o mesmo cuidado de `validate.rs`.

---

## 13. i18n e RTL

### 13.1 Pipeline

1. Script único (`scripts/export-i18n.mjs` ou `tsx`) lê `src/i18n/locales/*.ts` e gera `locales/*.yml` com `_version: 2`.
2. `rust-i18n = "4.2"`, `rust_i18n::i18n!("locales", fallback = "en")`, `extend!(gpui_component)` antes de `gpui_kit::init`.
3. Placeholders: hoje `{v}`; no `rust-i18n` viram `%{v}`. O script converte e um teste garante que nenhum placeholder sumiu.
4. Teste de paridade em Rust: mesmas chaves nos 8 locales; mesmos placeholders; nenhum valor vazio.
5. Detecção inicial: `sys-locale` (substitui `navigator.languages`), respeitando o mapeamento `zh-*`/`pt-*` atual; persistir a escolha.
6. Troca em runtime: `set_locale` + `cx.refresh_windows()`; menus em cache precisam ser reconstruídos.

Observações do alvo:

- O `rust-i18n` embute os locales em tempo de compilação; não há carregamento dinâmico documentado. Com 8 locales isso é aceitável, mas qualquer catálogo baixado em runtime exigiria outro mecanismo.
- Não há ICU MessageFormat/plural. Hoje o único caso é `device(s)` em `{count}`; mantenha como está ou separe em duas chaves (`one`/`other`) sem fingir suporte a plural.
- O `deepMerge` atual (fallback por chave e rejeição de `__proto__`) vira o fallback do `rust-i18n` + validação no teste de paridade.

### 13.2 RTL

O Kit só troca strings; **não espelha layout**. Hoje o `dir="rtl"` faz o CSS espelhar boa parte de margens, paddings, ordem e alinhamento, mas overrides físicos já quebram alguns pontos.

Se o árabe continuar suportado:

- Criar `LayoutDirection` no tema e helpers `row_start/row_end`, `text_align_start`, `flip_icon`.
- Revisar tela a tela: header, sidebar, tabs, sliders, modais, logs, onboarding.
- Espelhar ícones direcionais (chevrons, setas, "external link") e não espelhar ícones de mídia.
- Testar bidi em textos mistos (IP, serial, código) e a ordem de foco.
- Estimar 2–3 semanas adicionais na Fase 4.

Se não continuar: remover `ar` dos locales e documentar a regressão no release notes. É uma decisão de produto, não técnica.

---

## 14. Testes e gates

### 14.1 Pirâmide alvo

| Nível | Ferramenta | O que cobre |
| --- | --- | --- |
| Unidade de domínio | `cargo test -p openscrcpy-core` | validação, args, sessões, download, parsers, eventos |
| Integração de UI | `#[gpui_kit::test]` + `TestWindowExt` | clique, teclado, foco, estado, AccessKit; headless, sem display |
| Contrato de plataforma | testes com fake `Platform` | tray/hotkeys/updater sem SO real |
| Manual | matrix de SO | IME, RTL, screen reader, DPI, tray, instalador |
| Empacotamento | smoke em VM | instalar/abrir/atualizar/desinstalar |

Os 30 testes de frontend atuais se dividem em:

- **Lógica pura** (14 arquivos, 77 blocos): config, storage, window state, notifications, scrcpy args, i18n, alerts, tipos. Portar para Rust, mantendo os casos e os valores de borda.
- **DOM/a11y** (16 arquivos, 54 blocos): componentes, modais, foco e axe. Reescrever no harness do Kit; o que não for expressável vira checklist manual.

Lacunas de teste atuais que a Fase 3 deve fechar: shell/effects do App, drag-and-drop externo, chrome/resize do Linux, `Tooltip`, `SwitchRow`, `SheetModal`, pairing wireless, `DesktopPanel`, `FileDropCard`, `PerformanceGrid`, selects individuais e o canal `scrcpy-log`. Este último é emitido pelo backend e **não é consumido** hoje; no GPUI, ligue-o ao painel de logs ou remova o canal (decisão explícita).

### 14.2 Gates do novo stack

| Check | Comando |
| --- | --- |
| Formatação | `cargo fmt --all --check` |
| Lints | `cargo clippy --workspace --all-targets -- -D warnings` |
| Testes | `cargo test --workspace` |
| UI tests | `cargo test -p openscrcpy-gpui --features test-support` |
| Build | `cargo build --release --locked` |
| Audit | `cargo audit` |
| Pacote | smoke script por plataforma |

`clippy::allow`, `#[ignore]` e testes removidos continuam proibidos como atalho para verde.

### 14.3 Acessibilidade

- Todo controle interativo: role, nome acessível, estado, foco visível, teclado (Tab/Shift+Tab/Enter/Space/setas/Escape).
- Diálogos: foco inicial, trap, retorno ao elemento de origem, Escape.
- Testes automatizados verificam a árvore AccessKit, mas **não substituem** teste com leitor de tela real.
- Rodar Orca (Linux), NVDA (Windows) e VoiceOver (macOS) no fluxo: conectar dispositivo → iniciar espelho → parar.

**Gaps conhecidos do Kit (2026-11):** o estado `disabled` não é anunciado ([#3660](https://github.com/longbridge/gpui-kit/issues/3660)); `Dialog` não expõe título nem modalidade ([#3595](https://github.com/longbridge/gpui-kit/issues/3595)); não há live region ([#3659](https://github.com/longbridge/gpui-kit/issues/3659)). Isso atinge diretamente dois pontos do app: os muitos controles desabilitados por capability e o `LogPanel`, que hoje usa `aria-live="polite"` para anunciar logs. Planeje workarounds (rótulo/descrição explícitos, foco inicial, status textual alternativo, não depender de anúncio de log) e teste com leitor de tela; se o gap for inaceitável, isso vira bloqueador de release.

---

## 15. Riscos

| # | Risco | Prob. | Impacto | Mitigação |
| --- | --- | --- | --- | --- |
| R1 | Perda de usuários por macOS 15+ | Alta | Alto | Comunicação antecipada; manter 2.x; ou adiar migração no macOS |
| R2 | Vulkan exclui máquinas/VM | Média | Alto | Teste em hardware real; documentar; fallback `lavapipe` só para emergência |
| R3 | Tray/background quebra | Média | Alto | Spike S0.3; `ksni` como plano A no Linux; testar GNOME/KDE |
| R4 | Updater do 2.x instala artefato 3.x | Média | Crítico | Congelar `latest.json` na 2.x; release ponte; manifesto novo |
| R5 | RTL fica pior que hoje | Alta | Médio | Decisão D6; inventariar os overrides físicos atuais e reproduzir as mesmas exceções; checklist por tela; release notes |
| R6 | IME quebra CJK | Média | Alto | Spike S0.8; teste com teclado chinês real; bloquear release |
| R7 | gpui-kit pré-1.0 muda API | Alta | Médio | Pin `=0.10.0` (0.10.0 saiu em 2026-11-16; cadência semanal, `gpui-pre` pinado em `=0.3.12`); atualizar por PR trimestral; testes de UI como rede |
| R8 | Empacotamento/assinatura consome mais que o previsto | Alta | Médio | Fase 5 com spike de `cargo-packager`; cortar formatos secundários; AppImage/MSI não têm receita no Kit |
| R9 | Logs/throughput travam a UI | Média | Médio | Ring buffer + batch; teste S0.5 com 1k linhas/s |
| R10 | Acessibilidade pior que axe atual | Média | Alto | Testes AccessKit + leitor de tela; orçamento de correções; gaps #3660/#3595/#3659 |
| R11 | Escopo escorrega para "reescrever o core" | Alta | Alto | Fase 1 só move código; mudanças de comportamento em PRs separados |
| R12 | Curva de aprendizado de GPUI | Alta | Médio | Spike com dev sênior; orçamento +30–50% se o time não conhece |
| R13 | macOS Intel fora da CI do Kit | Média | Médio | Universal via `lipo` + teste em máquina Intel real; não confiar no build aarch64 |
| R14 | Texto bidirecional (árabe + latim) | Média | Médio | Bug aberto [#3442](https://github.com/longbridge/gpui-kit/issues/3442); testar linhas mistas (IP, serial) antes de prometer RTL |

---

## 16. Estimativa

Para **1 dev sênior** familiarizado com Rust (sem experiência prévia em GPUI):

| Fase | Semanas |
| --- | --- |
| 0 — Spike | 2–3 |
| 1 — Core | 4–5 |
| 2 — Fundação | 3–4 |
| 3 — Telas | 8–10 |
| 4 — Plataformas/a11y/RTL | 3–4 |
| 5 — Packaging/updater | 3–4 |
| 6 — Cutover | 2 |
| **Total** | **25–32 semanas** |

Com 2 devs (um em UI, um em core/plataforma), o caminho crítico cai para ~17–20 semanas, mas adiciona custo de coordenação. Sem experiência em GPUI, acrescente 30–50%. Não inclui o tempo de revisão de produto/design nem a produção de screenshots/traduções.

**Fase 0 é a única autorização recomendada agora.**

---

## 17. Backlog detalhado — Fases 0 a 2

### Fase 0

| ID | Tarefa | Depende | Est. |
| --- | --- | --- | --- |
| S0.0 | Instalar skills oficiais + ler Coding Guides | — | 0,5d |
| S0.1 | Workspace `spike/gpui` + skeleton | S0.0 | 1d |
| S0.2 | Matrix Linux/Windows/macOS | S0.1 | 2d |
| S0.3 | Tray (`ksni`/`tray-icon`) | S0.1 | 2d |
| S0.4 | Single-instance, hotkey, diálogos, drag-drop, notificação | S0.1 | 2d |
| S0.5 | Throughput de logs + batch | S0.1 | 1d |
| S0.6 | Janela externa do scrcpy | S0.1 | 1d |
| S0.7 | Updater minisign (artefato dummy) | S0.1 | 2d |
| S0.8 | i18n + CJK + árabe | S0.1 | 2d |
| S0.9 | Teste de UI + CI headless | S0.1 | 1d |
| S0.10 | `.deb` + AppImage em VM | S0.1 | 2d |
| S0.11 | Relatório GO/NO-GO | S0.2–S0.10 | 1d |

### Fase 1

| ID | Tarefa | Depende | Est. |
| --- | --- | --- | --- |
| C1.1 | Workspace + crate core vazio + CI | GO | 1d |
| C1.2 | Mover `validate`, `process`, `download`, `capabilities` | C1.1 | 2d |
| C1.3 | `AppState` + `EventBus` | C1.1 | 2d |
| C1.4 | Mover `adb`/`devices`/`wireless` | C1.2, C1.3 | 3d |
| C1.5 | Mover `session`/`scrcpy_args` (inclui `run_scrcpy`) | C1.3 | 5d |
| C1.6 | Mover `mic`/`audio` | C1.2 | 2d |
| C1.7 | Mover `kwin`/`grab`/window tracking | C1.1 | 2d |
| C1.8 | Adaptador Tauri + reemissão de eventos | C1.3–C1.7 | 2d |
| C1.9 | Migrar testes e smoke completo | C1.8 | 3d |

### Fase 2

| ID | Tarefa | Depende | Est. |
| --- | --- | --- | --- |
| F2.1 | Shell GPUI | GO, C1 | 2d |
| F2.2 | Tema + tokens | F2.1 | 2d |
| F2.3 | Fontes/ícones | F2.1 | 2d |
| F2.4 | Pipeline i18n + testes | F2.1 | 3d |
| F2.5 | Settings + paths + migração | F2.1 | 2d |
| F2.6 | Janela/estado | F2.5 | 1d |
| F2.7 | Serviços + fake | F2.5 | 3d |
| F2.8 | Alertas/modais base | F2.1 | 2d |
| F2.9 | Erros/panic/relatório | F2.5 | 1d |

---

## 18. Prompt de abertura para o agente

Copie o bloco abaixo ao iniciar a Fase 0.

```text
Você é o agente de implementação da migração do OpenScrcpy para GPUI Kit.

Leia, nesta ordem:
1. AGENTS.md e CONTRIBUTING.md.
2. .opencode/skills/quality-workflow/SKILL.md.
3. As skills oficiais `gpui-kit` e `gpui-kit-design-guides`
   (instale com a CLI `skills` no escopo do projeto).
4. docs/GPUI-MIGRATION.md (plano completo).

Tarefa: executar SOMENTE a Fase 0 (spike de viabilidade), em uma branch
`gpui/phase-0-spike`, no diretório `spike/gpui/`. Não altere `src/` nem
`src-tauri/`. O spike é descartável e não deve ser mergeado.

Siga as tarefas S0.0 a S0.11 do plano. Use `gpui-kit = "=0.10.0"`.
Para cada tarefa, registre: comando executado, resultado, SO/driver,
screenshot quando visual, e link da doc/API usada.

Restrições:
- Não invente API: se não estiver em docs.rs/gpui-kit ou nos guias,
  escreva um teste mínimo que prove o comportamento antes de usar.
- Não silencie warnings, não remova testes, não use unwrap em caminho
  de processo.
- Não adicione dependência sem justificar licença/manutenção.
- Se um critério de kill for atingido, pare e escreva o relatório
  parcial em vez de contornar com hack.

Entregável final: docs/experiments/2026-11-gpui-spike.md com a matrix
por SO, números de startup/RSS, dependências, riscos remanescentes e
recomendação GO / NO-GO / GO com escopo reduzido. Não comece a Fase 1.
```

Para a Fase 1, troque o parágrafo de tarefa por:

```text
Tarefa: executar a Fase 1 (extração do core), branch `gpui/phase-1-core`.
O comportamento do app Tauri deve permanecer idêntico. Mova código antes
de mudar qualquer lógica; mantenha os wrappers #[tauri::command] finos e
os nomes/payloads dos eventos. Rode todos os gates do AGENTS.md e um
smoke manual de espelhamento, câmera, desktop, mic, wireless, arquivos,
tray, atalhos e update. Não comece a Fase 2.
```

---

## 19. Decisões que o dono do produto precisa tomar

| # | Decisão | Opções | Recomendação |
| --- | --- | --- | --- |
| P1 | Autorizar a Fase 0? | sim/não | **Sim**, 2–3 semanas, com critérios de kill |
| P2 | macOS 15+ é aceitável? | sim / manter 2.x no macOS / adiar | Comunicar antes; não esconder no release notes |
| P3 | Exigir Vulkan no Linux? | sim / manter Tauri no Linux / detector + mensagem | Medir na Fase 0 antes de decidir |
| P4 | Manter árabe/RTL? | sim (2–3 semanas extras) / remover | Manter se há usuários; remover é regressão |
| P5 | Tray no Linux | `ksni` / `tray-icon` / sem tray | `ksni` se cobrir KDE+GNOME; senão manter Tauri |
| P6 | Update in-app | todos / só AppImage / nenhum | Todos é caro; só AppImage + link nos demais é o corte realista |
| P7 | Formatos de pacote | AppImage+deb / +rpm / +tar.gz | Manter o que a 2.x promete; cortar o resto explicitamente |
| P8 | Chrome customizado no Linux | nativo / custom | Nativo no 3.0; custom só se o frameless for requisito de design |
| P9 | Prazo/orçamento | 1 dev / 2 devs / parceiro | 1 dev = ~6–7 meses; 2 devs = ~4 meses |
| P10 | Beta fechado | sim/não | Sim: 2–4 semanas antes do 3.0, com rollback para 2.x |

---

## Apêndice A — Mapa de telas e componentes

| Svelte | Linhas | Equivalente GPUI | Observação |
| --- | --- | --- | --- |
| `App.svelte` | 173 | `AppShell` (Entity) | composição, lifecycle, listeners |
| `Header.svelte` | 422 | shell + `Button`/`Menu`/`Tabs`/`Select` | roving focus manual, menus com Escape |
| `Sidebar.svelte` | 105 | `div` + `Tabs` + `List` | manter WirelessPanel montado ao trocar aba |
| `Sidebar/DeviceList.svelte` | 193 | `List` + item custom | dezenas de devices, sem virtualização hoje |
| `Sidebar/WirelessPanel.svelte` | 297 | `Form`/`Input`/`Button` + dialog | IP, código, histórico, erros |
| `Sidebar/FileDropCard.svelte` | 33 | `on_drop` + `Button` | caminhos externos |
| `ControlPanel.svelte` | 176 | `Tabs` + painéis | 4 modos |
| `ControlPanel/MirrorPanel.svelte` | 65 | `Form` + `Select`/`Slider`/`Switch` | args do scrcpy |
| `ControlPanel/CameraPanel.svelte` | 137 | idem | capability-aware |
| `ControlPanel/DesktopPanel.svelte` | 154 | idem | validação numérica |
| `ControlPanel/MicPanel.svelte` | 96 | idem + meter custom | source padrão |
| `ControlPanel/MicMeter.svelte` | 84 | `canvas`/paint | sem alocação por frame |
| `ControlPanel/PerformanceGrid.svelte` | 71 | `Form` + `Select`/`Switch` | bitrate/fps/res |
| `ControlPanel/*Select.svelte` | 20–39 | `Select` | opções derivadas |
| `ControlPanel/BitrateSlider.svelte` | 20 | `Slider` | faixa/step |
| `ControlPanel/SettingSelect.svelte` | 39 | `Select` | label + hint |
| `Select.svelte` | 189 | `Select`/`Dropdown` | listbox, teclado, posicionamento |
| `SwitchRow.svelte` | 88 | `Switch` + row | label/descrição/disabled |
| `SliderRow.svelte` | 63 | `Slider` + row | valor formatado |
| `ProgressBar.svelte` | 44 | `Progress` | compacto/rotulado |
| `Tooltip.svelte` | 63 | `Tooltip` | atraso, foco, teclado |
| `Modal.svelte` | 115 | `Dialog` | backdrop, foco, Escape |
| `ThemedModal.svelte` | 154 | `Dialog` | variantes |
| `SheetModal.svelte` | 63 | `Dialog` | na prática é um dialog centralizado, não um sheet |
| `OnboardingModal.svelte` | 150 | `Dialog` multi-step | persistir conclusão |
| `ShortcutsPanel.svelte` | 35 | tabela custom | kbd + descrição |
| `LogPanel.svelte` | 196 | `List` + toolbar | 100 entradas, autoscroll, pausa, limpar, relatório |
| `AlertHost.svelte` | 23 | `Notification`/`Dialog` | fila de alertas |
| `ErrorBoundary.svelte` | 96 | panic hook + view | diagnóstico |
| `AppBehavior.svelte` | 43 | form | preferências |
| `SessionBehavior.svelte` | 236 | form | auto-reconnect etc. |
| `WindowButtons.svelte` | 71 | nativo ou titlebar | Linux only |
| `WindowResizeEdges.svelte` | 35 | nativo ou custom | 8 direções |
| fixtures | 34 | testes | portar cenários |

## Apêndice B — Comandos e eventos

Comandos registrados (24): `check_scrcpy`, `get_devices`, `adb_connect`, `get_mdns_devices`, `adb_pair`, `adb_shell`, `push_file`, `install_apk`, `kill_adb`, `run_scrcpy`, `stop_scrcpy`, `recenter_scrcpy_window`, `set_active_device`, `download_scrcpy`, `list_scrcpy_options`, `get_render_drivers`, `get_videos_dir`, `save_report`, `get_scrcpy_bin_dir`, `run_terminal_command`, `check_scrcpy_update`, `open_app_release`, `goto_wireless`, `configure_background_mode`.

Eventos Tauri: `scrcpy-log`, `scrcpy-status` (download + sessão), `tauri://drag-drop`.

## Apêndice C — Peças de UI que exigem trabalho manual

Classificação: **D** = mapeamento direto para uma primitiva do Kit; **H** = a primitiva existe, mas o comportamento precisa ser reimplementado; **C** = view custom, sem equivalente.

| # | Peça | Flag | Por que é difícil |
| --- | --- | --- | --- |
| 1 | `MicMeter` | C | dois streams de evento, 10 Hz, dB→0–100, reset em stop/unmount; não pode alocar por frame |
| 2 | `LogPanel` | C | ring de 100, stick-to-bottom de 48 px, live dot, heurística de cor, cópia, relatório JSON, terminal; `aria-live` não existe no Kit |
| 3 | `Select` | H | combobox ARIA com foco no trigger, Home/End/wrap, Escape/Tab, click-outside, lista absoluta que pode clipar, RTL |
| 4 | `Modal`/`ThemedModal` | H | captura/restauro de foco, `data-autofocus`, trap de Tab, scroll lock, z-tiers, progresso que desabilita dismissão |
| 5 | Chrome Linux | C | drag no header com exclusões, double-click maximize, 8 alças de resize, sync de estado de maximização |
| 6 | Editor por modo de sessão | H | invariantes: OTG desabilita performance, câmera+mic só Linux, audio source/dup só em mirror/playback, ratio do desktop, mic esconde Session Behavior |
| 7 | RTL/árabe | C | `dir=rtl` do CSS não existe; overrides físicos atuais já são imperfeitos; ícones direcionais e ordem de foco |
| 8 | Tema | H | 50+ tokens, `color-mix`, focus ring, scrollbars, `force-dark` do terminal, reduced motion |
| 9 | Troca de locale | H | 8 bundles, deep merge, race token, `lang`/`dir`, relabels de tray e menus em cache |
| 10 | Tray/background/notificações/polling | C | tray não existe no Kit; notificações têm dedup/grace; polling depende de visibilidade/background/sessão |

Observações que evitam over-engineering:

- O app **não usa** `Checkbox`, `Table`/`DataTable`, `Sheet` nem `VirtualList` hoje. Não introduza virtualização onde a lista é de dezenas de itens.
- O painel de logs é limitado a 100 entradas; o ring maior no core serve para diagnóstico/relatório, não para renderizar tudo.
- `SheetModal` é, na prática, um dialog centralizado — não force um bottom sheet só porque o nome diz "sheet".

## Apêndice D — Referências

- GPUI Kit: <https://gpui-kit.com/> · docs: <https://gpui-kit.com/docs/getting-started>
- Skills oficiais: <https://gpui-kit.com/skills> · <https://github.com/longbridge/gpui-kit/tree/main/skills>
- Coding Guides: <https://gpui-kit.com/docs/coding-guides> · Design Guides: <https://gpui-kit.com/docs/design-guides>
- Instalação/requisitos: <https://gpui-kit.com/docs/installation>
- Empacotamento: <https://gpui-kit.com/docs/packaging>
- Auto-update: <https://gpui-kit.com/docs/auto-update>
- i18n: <https://gpui-kit.com/docs/i18n>
- Acessibilidade: <https://gpui-kit.com/docs/accessibility>
- Testes: <https://gpui-kit.com/docs/test>
- Notificações: <https://gpui-kit.com/docs/system-notification>
- API: <https://docs.rs/gpui-kit/0.10.0>
- Repositório: <https://github.com/longbridge/gpui-kit>
- GPUI: <https://gpui.rs>
