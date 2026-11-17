# Lacunas de integração — OpenScrcpy × scrcpy

**Data:** 2026-11-17
**Âmbito:** pontos onde o motor `scrcpy` não está bem ligado à aplicação, ao
desktop ou ao ciclo de vida esperado.
**Já corrigido:** identidade da janela do espelhamento no KDE
(`SDL_APP_ID=openscrcpy`), ponte de posição/recentragem para KDE Wayland via
KWin scripting + D-Bus (`src-tauri/src/kwin.rs`) e áudio do modo câmara
(`--audio-source=mic`). Os restantes pontos ficam registados abaixo por ordem
de impacto.

---

## 1. Áudio

### 1.1 Modo câmara captura áudio de saída, não o microfone — **corrigido**

O modo câmara passou a enviar `--audio-source=mic` (`build_scrcpy_args`), pelo
que webcam/OBS recebe o microfone do telemóvel. O modo espelho mantém a saída
do dispositivo. Uma opção para escolher "microfone / saída do dispositivo" no
modo câmara fica como melhoria futura.

### 1.2 Câmara + microfone não podem coexistir

`run_scrcpy` impõe uma sessão por dispositivo e termina a anterior
(`commands.rs`, comentário "One session per device"). Isso impede usar o modo
câmara e a ponte de microfone ao mesmo tempo no mesmo telefone — exatamente o
caso "webcam + mic do telefone no OBS".

**Correção sugerida:** permitir `--video-source=camera --audio-source=mic`
numa única sessão, ou autorizar a ponte mic em paralelo (com `--no-control`).

### 1.3 Microfone virtual só existe no Linux

A ponte cria um sink nulo e define `PULSE_SINK` apenas em Unix
(`commands.rs`, `ensure_mic_sink` / `spawn_scrcpy_streams`). Em Windows/macOS o
áudio da ponte vai para a saída padrão, pelo que não aparece como dispositivo
de entrada.

**Correção sugerida:** documentar claramente; a médio prazo, driver virtual
(VB-Cable/CoreAudio) ou integração com o sistema.

### 1.4 Sem escolha de fonte de áudio

Só existem dois caminhos: `output` (default, que **silencia o telemóvel**
enquanto espelha) ou a ponte `mic`. Não há UI para `playback`, `mic`,
`voice-call`, `voice-performance`, nem `--audio-dup` (Android 13+, mantém o
áudio no telemóvel).

**Correção sugerida:** seletor de fonte no modo espelho, com aviso de que
`output` silencia o dispositivo e `playback` pode ser bloqueado por apps.

### 1.5 Sem roteamento para sink/dispositivo específico

Em sessões normais não há escolha de dispositivo de saída; `PULSE_SINK` só é
usado na ponte de microfone. Também não há controlo de volume/mute.

**Correção sugerida:** seletor de sink no Linux (PipeWire/PulseAudio) e
respeitar o sink padrão do sistema nos restantes casos.

---

## 2. Capacidades e versão do motor

### 2.1 Deteção de capacidades parcial

Apenas os render drivers são descobertos a partir de `scrcpy --help`
(`list_scrcpy_options` / `get_render_drivers`). As restantes flags são enviadas
às cegas, por exemplo:

- `--require-audio`
- `--flex-display`
- `--new-display`
- `--camera-torch`
- `--camera-zoom`
- `--ignore-video-encoder-constraints`
- `--window-x` / `--window-y`

Com um scrcpy antigo ou um binário custom, a sessão falha a arrancar sem uma
mensagem clara de "flag não suportada".

**Correção sugerida:** cachear a lista de flags suportadas por binário
(executar `scrcpy --help` uma vez) e filtrar/avisar antes de montar os
argumentos.

### 2.2 Download sempre "latest"

`download_scrcpy` consulta `releases/latest` da Genymobile e descarrega a
última versão, sem matriz de compatibilidade nem fallback. Uma versão futura
que remova/renomeie flags pode quebrar a app.

**Correção sugerida:** fixar uma versão mínima/máxima testada e validar a
versão do binário no arranque.

---

## 3. Ciclo de vida da sessão

### 3.1 Fechar a GUI mata as sessões

`teardown_sessions` (chamado em `RunEvent::Exit`) envia stop e mata os filhos.
Não existe modo "continuar em segundo plano", tray ou detach. O scrcpy
standalone continua a correr se o terminal fechar; a app não.

**Correção sugerida:** ícone de bandeja com "sair" vs "fechar janela" e opção
de manter sessões ativas.

### 3.2 Sem reconexão automática

Se o scrcpy terminar inesperadamente (USB removido, crash), a sessão é dada
como terminada. Só a cadeia de codecs de áudio tem retry.

**Correção sugerida:** política de retry com backoff para quedas de ADB e
reconexão quando o dispositivo voltar.

### 3.3 Sem modo detach

Não é possível parar a GUI e deixar o scrcpy a correr, nem reattach mais tarde.

---

## 4. Integração com o sistema

### 4.1 Sem notificações nativas

Conexão/desconexão de dispositivos, erros de sessão e atualizações usam apenas
alertas internos e o console. Não há `tauri-plugin-notification`.

**Correção sugerida:** notificações nativas para "dispositivo conectado",
"desconectado", "sessão falhou" e "atualização disponível".

### 4.2 Sem associações de ficheiros

`tauri.conf.json` não tem `bundle.fileAssociations`. Não é possível abrir um
`.apk` com o OpenScrcpy nem tratar links de emparelhamento.

**Correção sugerida:** associar `.apk` à ação de instalação e, se fizer sentido,
um esquema `openscrcpy://` para pairing.

### 4.3 Sem autostart / minimizar para tray

Não há opção de arrancar com o sistema nem de esconder a janela para a bandeja.

### 4.4 `Kill ADB` é global

O botão executa `adb kill-server` (`commands.rs`, `kill_adb`). Isso afeta
Android Studio, scrcpy manual e qualquer outra ferramenta que use o mesmo
servidor ADB. Não há confirmação nem âmbito por aplicação.

**Correção sugerida:** avisar na UI e preferir `adb disconnect` do dispositivo
ativo quando possível.

### 4.5 Nome do `.desktop` vs app id

O `.desktop` embutido no AppImage chama-se `OpenScrcpy.desktop` (capital),
enquanto o app_id da GUI é `openscrcpy` (minúsculas, derivado do binário). O
Gear Lever normaliza para `openscrcpy.desktop` e funciona, mas um `.deb`/`.rpm`
que instale `OpenScrcpy.desktop` pode não associar no KDE.

**Correção sugerida:** alinhar o nome do `.desktop` empacotado com o app_id
(`openscrcpy.desktop`) ou definir explicitamente o app_id da GUI para
`OpenScrcpy`.

### 4.6 Sem perfis por dispositivo

Bitrate, modos, resolução e caminho de gravação são globais. Trocar entre dois
telefones obriga a reconfigurar.

---

## 5. Funcionalidades do scrcpy não expostas

Sem UI/argumentos para:

- `--v4l2` (webcam nativa no Linux, evita a captura de janela no OBS)
- `--no-playback` (espelhar sem reproduzir localmente)
- `--no-control` (espelhar só leitura)
- `--audio-dup` (Android 13+, manter áudio no telemóvel)
- `--no-clipboard-autosync`
- `--power-off-on-close`
- escolha de encoder de vídeo/áudio
- várias sessões numa única janela / dashboard multi-dispositivo

Algumas são decisões de produto; outras são lacunas de paridade com o scrcpy.

---

## 6. Prioridade sugerida

1. Câmara+mic numa só sessão (o áudio da câmara já usa o microfone).
2. Deteção de flags suportadas por versão do scrcpy.
3. Tray + notificações nativas + modo "manter sessões".
4. Associações de ficheiros e perfis por dispositivo.
5. Paridade de flags menos usadas (`--v4l2`, `--audio-dup`, `--no-playback`).

---

## 7. Evidências recolhidas

- KWin, antes da correção: janela do espelhamento com
  `resourceClass=scrcpy` / `desktopFile=scrcpy`; GUI com `openscrcpy`.
- `plasmashell`: `Failed to find service for Unity Launcher "scrcpy.desktop"`.
- KDE Wayland: `try_capture_window_pos`/`recenter_window` usavam apenas
  `xdotool`, que não vê janelas Wayland nativas.
- Sessão do utilizador: `XDG_SESSION_TYPE=wayland`,
  `XDG_CURRENT_DESKTOP=KDE`.
