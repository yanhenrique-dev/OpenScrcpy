# Plano de UI/UX — casco nativo GPUI

Catálogo montado a partir da documentação oficial de cada componente em
<https://gpui-kit.com/component/>. O objetivo é montar uma interface completa e
organizada sem "sopa de componentes": cada peça existe porque resolve um papel
do produto.

Legenda de status:

- ✅ já usado no casco GPUI
- 🟡 próximo (alto valor, baixo risco)
- ⚪ candidato (só quando o caso aparecer)
- ⛔ fora (não se aplica ao OpenScrcpy)

## 1. Princípios

Os Design Guides do Kit são a régua:

1. clareza antes de personalidade;
2. composição antes de invenção;
3. tokens antes de valores;
4. convenção de desktop antes de web;
5. estado sempre visível (hover, foco, seleção, disabled, loading, erro);
6. geometria explica contenção (raio, borda, elevação);
7. feedback reflete realidade (nada de animação decorativa).

Regras práticas para este app:

- `Button` para comando; `Link` só para URL/e-mail.
- **um** primary por área de decisão; `ghost` para toolbar; `outline` para secundário.
- `danger` só em compromisso destrutivo (parar sessão, instalar update).
- ícone sempre acompanhado de rótulo ou `Tooltip` + nome acessível.
- estado não pode depender só de cor (ponto + texto).
- nada de modal empilhado; Escape fecha a camada do topo e devolve o foco.

## 2. O que já está em uso

| Componente | Onde |
| --- | --- |
| ✅ `TitleBar` | barra própria da janela, com arraste e controles |
| ✅ `TabBar`/`Tab` (segmented) | modos Tela/Câmera/Desktop/Microfone, com ícones |
| ✅ `Button` (+ variantes) | header, sidebar, sessão, console, diálogos |
| ✅ `Avatar` | identidade do dispositivo |
| ✅ `Tag` | versão do Android |
| ✅ `Input` | IP wireless, pairing, terminal |
| ✅ `Radio`/`RadioGroup` | fps, resolução, codec, renderer, rotação, DPI |
| ✅ `Slider` | bitrate, largura/altura/DPI do desktop |
| ✅ `Switch` | áudio, VSync, HID, gravação, preferências |
| ✅ `Progress` | download do core no onboarding |
| ✅ `Dialog`/`AlertDialog` | idioma, Sobre, Aplicativo, confirmação de Stop |
| ✅ notificação do SO (GPUI) | fim inesperado de sessão — não é o toast do Kit |

## 3. Catálogo por família

### 3.1 Ações e navegação

| Componente | Papel | Status |
| --- | --- | --- |
| `button` | ação com variantes, ícone, loading e seleção | ✅ |
| `toggle` / `toggle-group` | escolha persistente em botão; tema claro/escuro/sistema | 🟡 substituir os 3 botões de tema por `ToggleGroup` |
| `tabs` | seções irmãs alternadas | ✅ |
| `menu` | contexto e "mais opções" | 🟡 mover Sobre/Atualização/Idioma para um menu |
| `dropdown_button` | ação principal + variantes | ⚪ "Iniciar" + "Iniciar com opções" |
| `command` | paleta de comandos (⌘/Ctrl+K) | 🟡 unificar ações: modo, start/stop, refresh, pairing, update, tema, idioma |
| `toolbar` | contêiner de comandos | 🟡 header e console |
| `sidebar` | navegação lateral | ⚪ o sidebar atual é específico de domínio; manter custom |
| `status-bar` | estado na base da janela | 🟡 adb/scrcpy, dispositivo, sessão, logs |
| `breadcrumb` | hierarquia | ⛔ |
| `pagination` | páginas | ⛔ |
| `accordion` / `collapsible` | seções avançadas | ⚪ opções avançadas do Mirror |
| `stepper` | etapas | 🟡 onboarding (core → device → start) e pairing guiado |
| `nav-stack` (base) | pilha de telas | ⛔ navegação atual é por abas |

### 3.2 Formulários e entrada

| Componente | Papel | Status |
| --- | --- | --- |
| `input` | texto de uma linha | ✅ |
| `input-group` | input/textarea com addons | 🟡 `IP` + `Porta` com prefixo/sufixo e botão limpar |
| `number-input` | número com +/− e faixa | 🟡 porta de pairing, DPI/bitrate exatos |
| `textarea` | texto multilinha | ⚪ notas de sessão; terminal não é textarea |
| `select` | escolha única com busca | 🟡 fonte de áudio, renderer, rotação |
| `combobox` | busca + seleção | 🟡 lente da câmera (quando crescer) e busca de dispositivo |
| `checkbox` | binário/multi-seleção | ⚪ fila de arquivos com incluir/excluir |
| `switch` | preferência com efeito imediato | ✅ |
| `radio` | 2–7 opções exclusivas | ✅ |
| `slider` | ajuste contínuo | ✅ |
| `form` / `label` | layout rotulado | 🟡 wireless/pairing/configurações |
| `otp-input` | código de 6 dígitos | 🟡 o pairing code do Android é numérico: encaixa melhor que input solto |
| `calendar` / `date-picker` / `time-field` | data/hora | ⛔ (não há agendamento) |
| `color-picker` | cor | ⛔ (tema é claro/escuro/sistema) |
| `rating` | estrelas | ⛔ |
| `questionnaire` | multi-etapas com validação | ⛔ (onboarding é mais simples) |

### 3.3 Feedback e status

| Componente | Papel | Status |
| --- | --- | --- |
| `progress` | percentual conhecido | ✅ |
| `spinner` | espera curta/indeterminada | 🟡 botões Start/Pairing/Refresh |
| `skeleton` | preserva layout ao carregar | 🟡 lista de dispositivos e lentes |
| `shimmer` | texto "gerando…" | ⛔ |
| `alert` | aviso inline persistente | 🟡 erro de core/updater/manifesto na tela |
| `notification` (toast) | confirmação passageira | 🟡 copiar/report/par pairing/update |
| `empty` | estado vazio com ação | 🟡 sem dispositivos, sem logs, sem recentes |
| `marker` | divisor/status em linha | ⚪ marcar início/fim de sessão no console |
| `badge` | contador/ponto sobreposto | ⚪ contador de dispositivos; **não** é selo de texto |
| `tag` | metadado textual | ✅ |
| `avatar` | identidade | ✅ |
| `tooltip` | explicação de ícone/atalho | 🟡 ícones do header quando virarem icon-only |
| `kbd` | atalho visual | 🟡 card de atalhos e menus |
| `clipboard` | botão copiar com feedback | 🟡 substituir Copy do console |
| `icon` | ícone | ✅ |
| `image` | imagem/foto | ⛔ (sem imagens de conteúdo) |

### 3.4 Overlays

| Componente | Papel | Status |
| --- | --- | --- |
| `alert-dialog` | confirmação consequente | ✅ Stop; 🟡 instalar update, sair com sessão ativa |
| `dialog` | conteúdo focado | ✅ idioma/Sobre/Aplicativo; 🟡 mover preferências para Sheet |
| `sheet` | painel lateral persistente | 🟡 Preferências, detalhes do dispositivo, fila de arquivos |
| `popover` | conteúdo ancorado | ⚪ filtros do console |
| `hover-card` | prévia por hover | ⛔ não é acessível por teclado |
| `focus-trap` | prender foco | ✅ indireto (Dialog/Sheet) |
| `toast` (base) | fila de toasts | ✅ indireto via `Notification` |

### 3.5 Dados e listas

| Componente | Papel | Status |
| --- | --- | --- |
| `list` | lista virtualizada com busca/seleção | ⚪ recentes e lentes quando passarem de ~20 |
| `virtual-list` | milhares de linhas variáveis | ⚪ console se o limite subir de 100 |
| `table` | tabela simples | ⛔ (não há dado tabular) |
| `data-table` | milhares de linhas, sort/colunas | ⛔ |
| `tree` | hierarquia | ⛔ |
| `description-list` | pares chave/valor | 🟡 resumo da sessão e ficha do dispositivo |
| `attachment` | arquivo com preview/estado | ⚪ fila de push de arquivos |
| `diff` / `editor` | código | ⛔ |
| `chart` / `plot` | séries | ⚪ gráfico de bitrate/fps da sessão (nice-to-have) |
| `marker` | divisores | ⚪ |

### 3.6 Layout e contêineres

| Componente | Papel | Status |
| --- | --- | --- |
| `resizable` | painéis ajustáveis | 🟡 separar painel/console com altura arrastável |
| `scrollable` | rolagem com barra | 🟡 painéis de configuração e console |
| `group-box` | grupo com título | 🟡 Performance / Áudio / Entrada / Janela |
| `form` | layout de campos | 🟡 seção wireless |
| `toolbar` | comandos | 🟡 header/console |
| `settings` | página de preferências buscável | 🟡 substituir diálogos espalhados por uma página |
| `dock` / `grid-layout` | workspace arrastável | ⛔ complexidade sem demanda |
| `root` | camadas de overlay | ✅ indireto via `open_window` |
| `theme` | tokens e aparência | ✅ |

### 3.7 Conteúdo e comunicação

| Componente | Papel | Status |
| --- | --- | --- |
| `bubble` / `message` / `message-scroller` | conversa | ⛔ não há thread |
| `carousel` | destaque/snap | ⛔ |
| `text-view` | Markdown/HTML | ⚪ notas de release no update |
| `speech` | ditado | ⛔ (o app captura áudio do device, não do usuário) |

## 4. Arquitetura de UI proposta

### 4.1 Casco

```
┌ TitleBar (marca, versão, controles) ─────────────────────────────┐
│ Toolbar: status da sessão · busca/⌘K · update · configurações ⋯  │
├──────────────┬───────────────────────────────────────────────────┤
│ Sidebar      │ Tabs (Tela/Câmera/Desktop/Mic)                    │
│ Device Hub   │ Toolbar do painel: Iniciar (primary) Parar (danger)│
│ (cards)      │ Configurações em GroupBox/Form                    │
│ Wireless     │ ───────────────────────────────────────────────── │
│ Recentes     │ Console (Resizable, filtros, Kbd)                 │
│ Atalhos      │                                                   │
├──────────────┴───────────────────────────────────────────────────┤
│ StatusBar: adb · scrcpy · dispositivo · sessão · logs · update    │
└──────────────────────────────────────────────────────────────────┘
```

- O header deixa de ter 7 botões: tema vira `ToggleGroup`; Sobre,
  Verificar atualização e Idioma entram num `DropdownButton`/`Menu`;
  Configurações viram `Button` com `Tooltip` que abre uma `Sheet`.
- `Command` (Ctrl+K) vira a via rápida para tudo que é ação, sem tirar os
  botões visíveis das tarefas principais.
- `StatusBar` dá lugar aos estados hoje só no console (versões, dispositivo,
  sessão, contagem de logs, update).

### 4.2 Dispositivo

- Card atual: Avatar + estado + transporte + Tag Android (manter).
- `Skeleton` enquanto o polling descobre; `Empty` com "Conecte um aparelho" e
  botão de ajuda quando a lista está vazia.
- Ficha do dispositivo em `Sheet`: `DescriptionList` com modelo, serial,
  Android, transporte, IP e ações (copiar serial, desconectar).
- Wireless/pairing em `Form` + `InputGroup` + `OtpInput` + `Stepper`
  (1 descobrir, 2 parear, 3 conectar), com progresso e erro inline (`Alert`).

### 4.3 Painéis de modo

- Cabeçalho do painel: nome do modo + dispositivo + resumo em
  `DescriptionList` (bitrate, fps, resolução, codec, áudio).
- Configurações em `GroupBox` (Performance, Áudio, Entrada, Janela) com
  `Collapsible` para avançadas; `Select` para listas longas (fonte de áudio,
  renderer); `NumberInput` para DPI/porta; `Slider` + valor para bitrate.
- Câmera: `Combobox` para lente quando houver muitas.
- Botões: Iniciar (`primary`, `loading`), Parar (`danger` + `AlertDialog`).

### 4.4 Console

- `Resizable` vertical entre painel e console; altura persistida.
- `Scrollable` com barra, follow explícito e `Kbd` para atalhos.
- `Marker` no início/fim de cada sessão; `Clipboard` para copiar; `Alert`
  banner para o último erro; `Empty` quando não há logs.
- Filtros por nível em `Menu`/`ToggleGroup` (SYS, server, mic, erro).

### 4.5 Preferências e comandos

- `Settings` em `Sheet` (ou página) com páginas: Aplicação, Sessão,
  Aparência, Atualizações. `Form`/`Label` para os campos.
- `Command palette` com grupos: Sessão, Dispositivo, Navegação, Aparência,
  Ajuda; atalhos exibidos com `Kbd`.
- Confirmações: `AlertDialog` para parar/instalar/sair; `Notification` para
  resultados; `Alert` para erros persistentes.

## 5. Roadmap

| Fase | Entrega | Componentes | Critério de pronto |
| --- | --- | --- | --- |
| **1. Casco** | header enxuto + status + toasts | `Toolbar`, `ToggleGroup`, `Menu`/`DropdownButton`, `StatusBar`, `Notification`, `Tooltip`, `Kbd` | 1 primary por área, foco visível, toasts acessíveis, testes de UI |
| **2. Preferências** | Sheet de configurações com busca | `Settings`, `Form`, `Label`, `InputGroup`, `Select`, `NumberInput`, `Switch` | abrir/fechar por teclado, persistência, paridade dos 8 locales |
| **3. Dispositivo** | ficha + pairing guiado | `Sheet`, `DescriptionList`, `Stepper`, `OtpInput`, `Empty`, `Skeleton`, `Alert` | pairing completo por teclado, estados vazio/erro/loading |
| **4. Sessão** | painéis agrupados + console | `GroupBox`, `Select`, `Collapsible`, `Resizable`, `Scrollable`, `Marker`, `Clipboard` | console redimensiona, resumo em DescriptionList, filtros |
| **5. Comandos** | paleta + atalhos | `Command`, `Kbd`, `Menu`, `Tooltip` | Ctrl+K, Ctrl+1..4, Enter no comando, foco devolvido |
| **6. Extra** | fila de arquivos e métricas | `Attachment`, `Chart`/`Plot` | só se houver demanda real |

Cada fase: testes de UI para o fluxo principal, auditoria AccessKit verde,
strings nos 8 locales, screenshot no PR.

## 6. O que não vamos usar (e por quê)

- **Dock, GridLayout, DataTable, Tree, Diff, Editor**: custo de modelo e
  persistência sem caso de uso.
- **Bubble, Message, MessageScroller**: não há conversa.
- **Calendar/DatePicker/TimeField**: não há agendamento.
- **ColorPicker, Rating, Carousel, Speech, Questionnaire**: sem caso.
- **HoverCard**: informação essencial não pode depender de hover/ponteiro.
- **Pagination**: listas curtas e contínuas.
- **Badge como selo de texto**: ele é overlay/contador; para metadado use
  `Tag`.
- **Skeleton/Spinner em tudo**: só onde a espera é real; não decorar.

## 7. Dívidas de UX conhecidas

1. Header ainda concentra muitos botões (Fase 1 resolve).
2. Preferências em diálogos separados (Fase 2 resolve com Settings/Sheet).
3. Console tem altura fixa e sem filtros (Fase 4).
4. Erros só aparecem no console, não como `Alert`/toast (Fase 1/3).
5. Resumo da sessão em uma linha de texto (Fase 4: `DescriptionList`).
6. Pairing manual sem etapas e sem `OtpInput` (Fase 3).
7. Tema em três botões (Fase 1: `ToggleGroup`).
8. Atalhos documentados só no card (Fase 5: `Command` + `Kbd`).
