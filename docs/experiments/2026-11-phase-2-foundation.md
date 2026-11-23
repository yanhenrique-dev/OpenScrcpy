# Fase 2 — fundação do app GPUI (progresso)

> **Status:** parcial — shell, tema, i18n, settings e bounds de janela prontos.
> A fatia seguinte (device hub + sessão + console) está em
> [2026-11-phase-3-vertical-slice.md](2026-11-phase-3-vertical-slice.md).
> Faltam fontes/ícones, a trait `Platform`, os overlays e o tratamento de erro.
> **Data:** 2026-11-23
> **Crate:** `crates/open-scrcpy-gpui` (binário `openscrcpy`)
> **Plano:** [docs/GPUI-MIGRATION.md](../GPUI-MIGRATION.md)

## 1. O que existe

O primeiro casco nativo compila, abre uma janela de 1.200 × 890 (mínimo
960 × 620) e renderiza o layout-base:

- **Header:** nome, versão, três botões de tema (claro/escuro/sistema) com
  estado selecionado e um botão de idioma.
- **Sidebar:** "Device Hub" com empty state; as abas de modo ficam na área
  principal para bater com o layout alvo.
- **Conteúdo:** abas Screen/Camera/Desktop/Microphone com estado selecionado e
  um painel vazio por modo.
- **Console:** painel inferior com o título e o estado "Waiting for sequence…".

Nenhuma string visível está hardcoded no código: tudo sai dos catálogos.

### 1.1 i18n

`scripts/export-i18n.mjs` lê os bundles Svelte e gera
`crates/open-scrcpy-gpui/locales/ui.yml`:

- **8 locales × 390 chaves**, o mesmo conjunto do app 2.x;
- placeholders convertidos de `{name}` para `%{name}` (sintaxe do `rust-i18n`);
- o script falha se uma locale perder chave ou placeholder — a paridade é gate
  de geração, não só de runtime;
- `tests/i18n_parity.rs` repete a checagem sobre o YAML que é embarcado no
  binário (chaves, locale ausente, valor vazio, placeholders);
- `build.rs` registra `locales/ui.yml` como input, senão o macro `i18n!` não
  recompila o binário quando o catálogo muda.

O formato é o **v2 key-first** do `rust-i18n` (chave → locale → texto), e não
um arquivo por locale: o parser v2 é key-first, e o spike provou que esse é o
formato que resolve `t!("controlPanel.screen")`.

### 1.2 Settings

`AppSettings` (schema v1) cobre locale, tema e geometria da janela:

- leitura normaliza dados não confiáveis (locale desconhecida, tamanho fora da
  faixa, coordenadas `NaN`, posição pela metade);
- escrita atômica (arquivo temporário + rename, com fallback no Windows);
- o shell salva os bounds no `on_window_should_close` e restaura no próximo
  start; `maximized` é preservado;
- o caminho vem de `directories` (`dev/yanhenrique/OpenScrcpy`), sem tocar no
  `localStorage` antigo.

## 2. Decisões

### 2.1 Idioma por botão de ciclo (stub consciente)

O header troca de locale em runtime por um botão que mostra
`Language: <endônimo>` e avança para o próximo catálogo. É funcional e cobre o
critério da fase, mas não é a UX final: o menu de idioma com os 8 itens é a
fatia 3.1. Não montei um `Select`/`DropdownMenu` agora para não inventar API
sem precisar; o spike só provou `Button`.

### 2.2 Tema em três botões, não em ciclo

Três botões `Light`/`Dark`/`System` com estado selecionado tornam o resultado
previsível — um botão de ciclo esconde qual será o próximo estado. `System` lê
`cx.window_appearance()` no momento da aplicação.

### 2.3 Uma locale de cada vez não foi adotada

O plano falava em "8 locales"; não exigia oito arquivos. O formato v2 do
`rust-i18n` é key-first e resolve com um único `ui.yml`, como no spike.

### 2.4 CI do crate ainda não está ligado

Compilar GPUI na matrix atual exige Vulkan/X11/xkbcommon e a decisão D9
(Ubuntu 22.04 × 24.04). Ligar agora deixaria o job vermelho em todas as
plataformas. O crate entra no CI quando a fase de packaging/CI da Fase 2 for
fechada, junto com a decisão de base Linux.

## 3. Números

| Item | Valor |
| --- | --- |
| Chaves por locale | 390 |
| Locales geradas | 8 |
| Testes Rust | 9 unitários + 2 de paridade + 1 de UI = **12 passed** |
| Binário debug | ~211 MB (`debug = 0`, sem strip; release ainda não medido) |
| Fonte | do sistema (Inter embutida fica para F2.3) |

## 4. Gates

| Gate | Resultado |
| --- | --- |
| `cargo fmt --check` (`crates/open-scrcpy-gpui`) | PASS |
| `cargo clippy --all-targets -- -D warnings` | PASS |
| `cargo test` | PASS (12) |
| `cargo build --release --locked` | **não executado** |
| Smoke visual (`cargo run` numa sessão real) | **não executado** — os testes de UI renderizam a janela headless; não abri uma janela sobre a sessão do usuário |
| CI | **não ligado** (ver 2.4) |

## 5. Pendências da Fase 2

| ID | Entrega | Situação |
| --- | --- | --- |
| F2.1 | Shell (header/sidebar/main/log) | **feito** (console fixo; colapsar fica na F3.8) |
| F2.2 | Tema + tokens | **parcial**: claro/escuro/sistema; os ~50 tokens do CSS ainda não foram mapeados |
| F2.3 | Inter embutida + fallback CJK/árabe + zoom 100–200% | pendente |
| F2.4 | Inventário dos ~40 ícones Lucide | pendente (o shell ainda não usa ícones) |
| F2.5 | Pipeline i18n + 8 locales + paridade | **feito** |
| F2.6 | Settings com schema, escrita atômica, validação | **feito** |
| F2.7 | Bounds de janela + monitores | **parcial**: salva/restaura e maximizado; não valida contra monitores desconectados |
| F2.8 | Trait `Platform` + fake | pendente |
| F2.9 | Alertas/modais do Kit | pendente |
| F2.10 | Panic hook + tela de erro + relatório | pendente |

## 6. Como reproduzir

```sh
node --experimental-strip-types scripts/export-i18n.mjs
cd crates/open-scrcpy-gpui
cargo test
cargo run          # abre a janela do casco
```

## 7. Próximos passos

1. F2.3/F2.4: embutir Inter, mapear o inventário de ícones e a escala de zoom.
2. F2.8: trait `Platform` com `FakePlatform` para diálogos, notificações,
   opener, tray, hotkeys, single-instance e updater.
3. F2.9/F2.10: `Dialog`/`Notification` do Kit no lugar dos modais e panic hook
   com relatório de diagnóstico.
4. Ligar o crate no CI quando D9 e as dependências de sistema estiverem
   decididas; até lá, o gate local é fmt + clippy + test.
