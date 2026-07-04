# DevTerm — Implementierungsplan

Ein modernes, pluginfähiges Dev-Terminal für Windows-first: echte Terminal-App (eigenes Fenster, eigener Renderer, ConPTY pro Pane) statt Multiplexer-im-Terminal. Kein tmux-Klon, kein psmux-Nachbau — die Architektur vermeidet deren Flacker-Problem per Design.

Prämisse: **solide vor schnell**. Entscheidungen sind auf Wartbarkeit über Jahre optimiert, nicht auf schnellstes MVP.

---

## 1. Tech-Stack (Entscheidung + Begründung)

### Sprache: Rust

Go wäre das schnellere MVP, aber für eine echte Terminal-App die schwächere Basis:

- **Kein reifes GPU-Rendering-Ökosystem** in Go. Rust hat `wgpu` + `winit` als De-facto-Standard.
- **Terminal-Emulation muss nicht selbst gebaut werden**: `alacritty_terminal` ist ein eigenständiges Crate (VT-Parser + Grid-Modell), produktiv genutzt u. a. von Zed. Einen VT100/xterm-Emulator selbst zu schreiben ist das größte Einzelrisiko des Projekts — Jahre an Edge-Cases. In Go gäbe es keine vergleichbar gehärtete Bibliothek.
- **Kein GC im Render-Hotpath**, saubere FFI zu ConPTY/DirectWrite ohne CGo.
- Alle relevanten Referenzprojekte (WezTerm, Alacritty, Zed-Terminal) sind Rust — deren Crates und Bugfixes sind direkt nutzbar.

### Kern-Crates

| Zweck | Crate | Begründung |
|---|---|---|
| Terminal-Emulation (VT-Parsing, Grid, Scrollback) | `alacritty_terminal` | Gehärtet durch Alacritty + Zed. Wir bauen KEINEN eigenen Emulator. |
| PTY (ConPTY auf Windows) | `portable-pty` (WezTerm) | ConPTY-Eigenheiten sind dort schon ausgebügelt; Unix später gratis. |
| Fenster + Input | `winit` | Standard; IME-, DPI-, Multi-Monitor-Support. |
| GPU-Rendering | `wgpu` | D3D12 auf Windows, später Metal/Vulkan gratis. |
| Text-Shaping + Glyph-Rasterung | `swash` | Shaping, Emoji, Ligaturen; wir bauen den Glyph-Atlas, nicht den Rasterizer. |
| Font-Discovery + Fallback | `fontdb` | Systemfonts finden, Fallback-Ketten (Emoji, CJK, Nerd Font Symbole). |
| UI-Chrome (Tabs, Palette, Dialoge) | `egui` (via `egui-wgpu`) | Nur fürs Chrome, NICHT fürs Terminal-Grid. Später ersetzbar. |
| Config | `serde` + `toml` + `serde_yaml` | TOML für App-Config, YAML für Projekt-Layouts. |
| Async/Threads | std-Threads + `crossbeam-channel` | PTY-I/O ist Thread-per-Pane; kein Tokio im Kern nötig. |
| Plugins (Phase 3) | `wasmtime` | Sandbox, sprachunabhängig (Rust/Go/JS→WASM). |
| CLI/IPC | `clap` + Named Pipes (`windows` crate) | `devterm split -- npm run dev` spricht mit laufender Instanz. |

### Bewusst NICHT

- **Tauri/Electron/Flutter**: Webview-Latenz und Compositing-Umwege sind genau das, was ein Terminal nicht haben darf.
- **Eigener VT-Parser**: erst evaluieren, ob `alacritty_terminal` je zu eng wird (dann Fork, nicht Neuschrieb).
- **tmux-Protokoll-Kompatibilität**: Scope-Falle. Eigene Semantik, eigene Keybindings (tmux-Preset als Keymap ist ok).

---

## 2. Architektur

### Prozessmodell

```
DevTerm.exe (ein Prozess)
├── Main Thread        winit Event-Loop, Input, Rendering (wgpu)
├── pro Pane:
│   ├── PTY-Reader-Thread   ConPTY-Output → Parser → Terminal-Modell
│   └── PTY-Writer          Input-Bytes → ConPTY
├── Config-Watcher-Thread   Hot-Reload
└── IPC-Server-Thread       Named Pipe für devterm-CLI
```

### Die Anti-Flacker-Architektur (Lektion aus PSmux)

PSmux' Kernfehler: Byte-Chunks aus ConPTY werden zu früh als "fertiger Frame" gesnapshottet und gerendert — halbfertige Ink/TUI-Frames werden sichtbar. Unsere Regeln:

1. **Parser und Renderer sind vollständig entkoppelt.** Der Reader-Thread parst kontinuierlich in das Terminal-Modell (hinter einem Mutex/Damage-Layer). Es gibt keinen "Snapshot pro Read".
2. **Damage-getriebenes Rendern mit Coalescing.** Der Renderer läuft VSync-getaktet und rendert den *aktuellen* Modellzustand — nie einen Chunk-Zustand. Zusätzlich ein kurzes Coalescing-Fenster (~1–2 ms nach letztem Byte-Burst), bevor ein Frame als "ruhig" gilt.
3. **Synchronized Output (DECSET 2026) unterstützen.** Moderne TUIs (neovim, neuere Ink-Versionen) markieren Frame-Grenzen explizit — zwischen BSU/ESU wird nicht gerendert.
4. **Damage-Tracking pro Zeile/Region**, damit ein 4-Pane-Layout mit einem lauten Pane nicht drei ruhige Panes mitrendert.

### Workspace-Layout (Cargo)

```
devterm/
├── Cargo.toml                 (workspace)
├── crates/
│   ├── devterm-core/          Domänenmodell: Session, Tab, Pane, Layout-Tree,
│   │                          Events, Commands. KEIN I/O → vollständig unit-testbar.
│   ├── devterm-pty/           portable-pty-Wrapper, Reader/Writer-Threads,
│   │                          Prozess-Lifecycle (spawn, kill, exit-code, restart).
│   ├── devterm-term/          alacritty_terminal-Integration: Parsing, Grid,
│   │                          Scrollback, Damage, Suche, Selektion.
│   ├── devterm-render/        wgpu-Renderer: Zellen-Grid, Glyph-Atlas (swash),
│   │                          Cursor, Selektion, Splits, DPI.
│   ├── devterm-config/        Schema (serde), Laden/Validieren/Hot-Reload,
│   │                          Keymaps, Themes, Projekt-Layouts.
│   ├── devterm-app/           Binary: winit-Loop, Input-Routing, egui-Chrome,
│   │                          IPC-Server, verdrahtet alles.
│   ├── devterm-cli/           (M2) `devterm`-CLI: spricht via Named Pipe mit der App.
│   └── devterm-plugin/        (M3) wasmtime-Host, Plugin-API, Manifest.
```

Abhängigkeitsrichtung strikt: `app → {render, term, pty, config, plugin} → core`. `core` kennt kein wgpu, kein winit, kein ConPTY.

### Zentrale Datenstruktur: Layout-Tree

```
Tab
└── LayoutNode
    ├── Split { direction: H|V, ratio, children: Vec<LayoutNode> }
    └── Leaf  { pane_id }
```

Alle Operationen (split, close, resize, focus-move, zoom) sind reine Funktionen auf diesem Baum in `devterm-core` — property-testbar (Invarianten: Ratios summieren zu 1, kein leerer Split, Fokus existiert immer).

---

## 3. Meilensteine

### M0 — Walking Skeleton (Ziel: ~2 Wochen)

Ein durchgestochener Pfad, um alle Risiko-Integrationen früh zu treffen:

- [x] Cargo-Workspace + CI (GitHub Actions, Windows-Runner: fmt, clippy, test, cargo-deny)
- [x] winit-Fenster mit wgpu-Surface
- [x] Eine ConPTY-Session (PowerShell) via portable-pty
- [x] Output → alacritty_terminal → naives Grid-Rendering (ein Font, kein Fallback)
- [x] Tastatur-Input → PTY (inkl. Ctrl-Sequenzen), Resize Fenster → PTY
- [x] **Abnahme:** `vim`, `htop` (in WSL), Claude Code und `git log` laufen sichtbar korrekt und ohne Flackern

M0 beweist die Kernthese. Alles danach ist Ausbau.

### M1 — v0.1 "täglich benutzbar" (Ziel: +4–6 Wochen)

- [x] Splits horizontal/vertikal, Panes schließen, Fokus-Navigation (Keyboard + Maus-Klick)
- [x] Pane-Resize (Keyboard, richtungsbasiert + Maus-Divider-Drag)
- [x] Scrollback (konfigurierbares Limit) + Maus-/Keyboard-Scrolling
- [x] Selektion (Maus-Drag) + Copy/Paste (System-Clipboard via `arboard`, bracketed paste)
- [x] Font-Fallback-Kette (Nerd Font Symbole, CJK; Farb-Emoji monochrom → M4) + DPI-/Font-Family-/Font-Size-/Line-Height-Hot-Reload
- [x] Shell-Auswahl: PowerShell 7, Windows PowerShell, cmd, Git Bash, WSL
- [x] `config.toml` (Font, Größe, Theme, Shell, Scrollback) mit Hot-Reload (`notify`)
- [x] Keybindings konfigurierbar (Default-Keymap + tmux-Preset als prefixfreie Direktbindungen)
- [x] Anti-Flacker komplett: DECSET-2026 + Byte-Burst-Coalescing (~1,5 ms) + Skip-Clean-Frame-Damage (pro Pane; PSmux-Regressionstest) — feingranulares Damage pro Zeile bleibt Optimierung
- [ ] **Abnahme:** Du benutzt DevTerm eine Woche als Haupt-Terminal, ohne zu Windows Terminal zurückzuwechseln

### M2 — v0.2 "projektorientiert" (Ziel: +4–6 Wochen)

- [ ] Tabs / Workspaces
- [ ] Session-Persistenz: Layout + CWDs + Commands speichern & wiederherstellen
- [ ] Projekt-Profile `devterm.yml` im Repo-Root:
  ```yaml
  name: my-app
  layout:
    - split: horizontal
      panes:
        - cmd: npm run dev
        - cmd: docker compose up
        - cmd: git status
  ```
- [ ] Command Palette (egui): Aktionen, Sessions, Themes fuzzy durchsuchbar
- [ ] `devterm`-CLI + IPC: `devterm open`, `devterm split -- <cmd>`, `devterm run <profil>`
- [ ] Global Hotkey / Quake-Mode (einblendbares Fenster)
- [ ] Exit-Code-Anzeige pro Pane, "Restart failed pane"

### M3 — v0.3 "Plugin-Plattform"

Plugin-API erst jetzt — nach zwei Releases ist klar, welche internen Events stabil sind.

- [ ] Interne Event-/Command-API einfrieren (das wird die Plugin-ABI)
- [ ] wasmtime-Host: Plugins als WASM, Manifest (Permissions: welche Events, welche Commands)
- [ ] Plugin-Oberflächen: Statusbar-Module, Palette-Commands, Pane-Output-Listener (z. B. "URL erkannt", "Test fehlgeschlagen → Notification"), Pane-Decorations
- [ ] Referenz-Plugins (dogfooding der API): Git-Status in Statusbar, Docker-Container-Liste, Pomodoro
- [ ] Suche im Scrollback (Regex, Highlights)

### M4 — Polish & Reichweite (offen)

- Ligaturen, Wide-Char/Grapheme-Härtung, Themes-Galerie
- Hyperlink-Support (OSC 8) + klickbare Datei:Zeile-Pfade
- SSH-Panes, macOS/Linux-Builds (portable-pty/wgpu/winit machen das realistisch)
- Health-Checks pro Pane, AI/Agent-Pane

---

## 4. Qualitätsstrategie (das "solid" im Plan)

- **`devterm-core` ohne I/O**: Layout-Tree, Fokus, Sessions als reine Logik → Unit- + Property-Tests (`proptest`) für Invarianten.
- **Golden-Stream-Tests für die Emulation**: aufgezeichnete Byte-Streams echter Sessions (vim, htop, Claude Code/Ink, `git diff` mit Farben) werden ins Terminal-Modell gespielt, Grid-Endzustand als Snapshot verglichen.
- **Der PSmux-Bug als Regressionstest**: derselbe Byte-Stream in zufälligen Chunk-Grenzen zerteilt → Endzustand muss byte-identisch sein, und im Coalescing-Fenster darf kein Zwischenzustand als Frame markiert werden.
- **ConPTY-Integrationstests** auf dem Windows-CI-Runner: spawn → echo → resize → exit-code.
- **Manuelle Testmatrix** vor jedem Release: Windows 10 + 11, 100 %/150 %/200 % DPI, PowerShell/cmd/Git Bash/WSL, vim/htop/Claude Code.
- **cargo-deny** (Lizenzen, Advisories), `#![deny(warnings)]` in CI, `unsafe` nur in `devterm-pty`/`devterm-render` mit Kommentarpflicht.

---

## 5. Risiken & Gegenmaßnahmen

| Risiko | Gegenmaßnahme |
|---|---|
| VT-Emulation-Edge-Cases | Nicht selbst bauen (`alacritty_terminal`); Golden-Stream-Tests; bei Lücken Upstream-PR oder Fork |
| ConPTY-Eigenheiten (Resize-Repaints, verschlucktes Initial-Output, Verhalten je Windows-Version) | M0 testet genau das zuerst; portable-pty bringt WezTerms Workarounds mit |
| Font-Rendering-Erwartung auf Windows (ClearType-Gefühl, Gamma) | Früh mit echten Fonts (Cascadia, JetBrains Mono) vergleichen; Gamma-korrektes Blending im Shader |
| egui fürs Chrome fühlt sich "fremd" an | egui ist bewusst nur Chrome; Terminal-Grid ist eigener Renderer; Chrome später ersetzbar |
| Scope-Explosion (tmux-Parität, SSH, AI, Web-UI) | Meilenstein-Abnahmen sind Nutzungs-basiert ("eine Woche Haupt-Terminal"), Features hinter M3 bleiben Backlog |
| Plugin-API zu früh eingefroren | API erst in M3, nachdem interne Events zwei Releases stabil waren |

---

## 6. Erste konkrete Schritte

1. `cargo new` Workspace mit `devterm-core`, `devterm-app` (Stubs für den Rest)
2. CI-Pipeline (Windows-Runner) vor dem ersten Feature
3. Spike 1: portable-pty → PowerShell spawnen, Output roh ins Log (ConPTY-Verhalten verstehen)
4. Spike 2: winit + wgpu Fenster, hartkodiertes Test-Grid mit swash-Glyphen rendern
5. Spike 1 + 2 verbinden = M0
