# Fase 5 — empacotamento do app GPUI (spike local)

> **Status:** parcial — o casco nativo gera um AppImage type 2 que abre e roda
> neste host. Portabilidade (VM limpa), `.deb`, assinatura e updater pendentes.
> **Data:** 2026-11-23
> **Script:** [scripts/package-gpui-appimage.sh](../../scripts/package-gpui-appimage.sh)
> **Plano:** [docs/GPUI-MIGRATION.md](../GPUI-MIGRATION.md)

## 1. O que foi feito

1. `cargo build --release` no `crates/open-scrcpy-gpui` — PASS (3m52s neste
   host; binário de 43 MB).
2. `scripts/package-gpui-appimage.sh` monta o AppDir (binário, `.desktop`,
   ícone, `AppRun`), roda `linuxdeploy` para as libs resolvidas e gera o
   AppImage com o `linuxdeploy-plugin-appimage` do cache do Tauri.
3. Artefato: `artifacts/gpui/OpenScrcpy-GPUI-x86_64.AppImage` (**16 MB**).
4. Smoke: `APPIMAGE_EXTRACT_AND_RUN=1 timeout 15 ./OpenScrcpy-GPUI-x86_64.AppImage`
   → janela abriu e ficou rodando até o timeout, sem panic. O aviso de atalho
   já registrado pelo OpenScrcpy 2.0 apareceu, como esperado.

## 2. O que o bundle contém

`linuxdeploy` copiou apenas o que resolveu do binário:

- `libXau`, `libXdmcp`, `libxcb-xkb`, `libxkbcommon`, `libxkbcommon-x11`.

Ficam como dependência do host: **Vulkan loader/driver, wayland/X11, fontconfig,
freetype, glibc e o driver de GPU**. Ou seja, este AppImage é um artefato de
validação, não um pacote portátil. Empacotar Vulkan/fontconfig com segurança
(ou declarar a matriz mínima) é trabalho da Fase 5 de verdade.

## 3. Limitações conhecidas

- sem teste em VM limpa (bloqueio B7);
- sem `.deb`/`.rpm`, sem assinatura e sem updater (`latest-gpui.json`);
- sem CI de release para o binário nativo;
- tray/background continuam bloqueados pela ausência de `Window::hide` no GPUI
  0.3.12 (ver [2026-11-phase-3-vertical-slice.md](2026-11-phase-3-vertical-slice.md));
- o ícone da janela ainda é o padrão do GPUI (o `.desktop` usa o ícone do 2.x).

## 4. Como reproduzir

```sh
cd crates/open-scrcpy-gpui
cargo build --release

cd ../..
scripts/package-gpui-appimage.sh \
  crates/open-scrcpy-gpui/target/release/openscrcpy
```

O script aceita um caminho de binário como primeiro argumento (ou
`GPUI_BIN`), útil quando o `CARGO_TARGET_DIR` está fora do crate.

## 5. Próximos passos

1. Smoke em VM limpa (Ubuntu 24.04 e 22.04) e decisão D9 da base mínima.
2. Empacotar/declarar as dependências de runtime (Vulkan, fonte, Wayland/X11).
3. `.deb` e assinatura; depois o updater com manifesto próprio.
4. Job de CI de release para o binário nativo, sem Node/Vite.
