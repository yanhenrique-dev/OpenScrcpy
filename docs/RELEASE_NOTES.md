## OpenScrcpy 2.0.0

OpenScrcpy 2.0.0 is the first release published from this repository. It brings
a rebuilt Svelte 5 interface, signed in-app updates, window state persistence
and the full mirror, camera, desktop and microphone feature set.

### Highlights

- **Svelte 5 frontend** with a redesigned settings dashboard, light/dark themes
  and nine interface languages.
- **Mirror, camera, desktop and microphone modes** from a single window.
- **Wireless pairing** with a saved-device history for Android 11+.
- **Precision input over OTG/HID**, hardware keyboard and mouse side by side.
- **Drag and drop**: install an APK or push any file to `/sdcard/Download/`.
- **Signed in-app updates** through the Tauri updater.
- **Remembered window size and position**, with a recentre shortcut.

### Downloads

- **Windows x64**: `.exe` installer or `.msi` package.
- **macOS**: universal `.dmg` for Intel and Apple Silicon.
- **Linux x86_64**: `.AppImage`, `.deb` or `.rpm`.

The app can download the scrcpy core on first run, or use an existing `adb` and
`scrcpy` installation. See `GUIDE.md` in the repository for setup and
troubleshooting.

### Notes

The Windows and macOS builds are not code-signed, so SmartScreen or Gatekeeper
may warn on first launch. The guide explains how to allow the app. The updater
artifacts are signed with the project's Tauri signing key.
