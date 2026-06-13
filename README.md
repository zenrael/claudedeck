# ClaudeDeck

An [OpenDeck](https://github.com/nekename/OpenDeck) plugin that turns Stream Deck keys into
**Claude Code session managers**. Each key launches a Claude Code session in a terminal, brings
it to focus on a second press, and renders a live status face: context-window usage, the model,
the session title, and a busy/idle indicator.

It is a **generic action** — drop "Claude Session" onto any key of any OpenDeck-supported device.
Developed and used on an **Ajazz AKP03E / Mirabox N3** (6 LCD keys), but not hardwired to it.

![key faces: model badge top-left, braille spinner top-right, scrolling title, coloured progress bar]

## What a key shows

- **Idle / pre-launch:** the Claude sparkle icon + the configured launch directory's basename
  (e.g. `work`, `xp3-imc`, or `~` for `$HOME`).
- **Live session:**
  - **Top-left:** model badge — family letter in Claude coral + version (`O4.8` = Opus 4.8,
    `S4.6`, `H4.5`), parsed from the model id.
  - **Top-right:** Claude's braille **spinner** (`⠋⠙⠹…`, blue) while the session is *working*; a
    solid blue **dot** when it's *idle / waiting for you*.
  - **Middle:** the session title (set with Claude's `/rename`), marquee-scrolled if it overflows.
  - **Bottom:** a **progress bar** of context-window usage, no number. Its colour ramps in 10%
    bands from Claude blue → green → yellow → orange → red, reaching red at a configurable
    **threshold** (default 60%), marked with a tick.

## How it works (data flow)

```
key press ─▶ plugin (key_down) ─▶ kitty/alacritty --class claudedeck-<id> ─▶ claude --settings <scoped>
                                                                                    │
   ┌── focus (2nd press): kdotool search --class claudedeck-<id> windowactivate    │ statusLine.command
   │                                                                                ▼
plugin render loop ◀── $XDG_RUNTIME_DIR/claudedeck/<id>.json ◀── scripts/statusline.py (writes pct+model)
        │                                                              (keyed by CLAUDEDECK_KEY env)
        └── kdotool getwindowname (title → label + busy/idle glyph) ─▶ set_image(rendered PNG)
```

- **Identity:** each placed key gets a stable Wayland **app-id** `claudedeck-<device>-r<row>-c<col>`.
  `claude` rewrites the window *title* constantly but never the app-id, so that's what we match on.
- **Focus:** on a second press, the existing window is activated via `kdotool` (the only reliable
  way on KWin/Wayland — `wmctrl`/`xdotool` don't work there).
- **Context %:** the plugin can't read a running session directly. It installs a **scoped**
  statusline (`claude --settings ~/.local/share/claudedeck/session-settings.json`, which points at
  `scripts/statusline.py`) so only ClaudeDeck-launched sessions write a per-key file. Your global
  `~/.claude/settings.json` is never touched. Correlation is by the `CLAUDEDECK_KEY` env var the
  plugin sets at launch.
- **Busy/idle:** read from the window title's leading glyph — braille (U+2800–28FF) = working,
  `✳` (U+2733) = idle/awaiting input.
- **Rendering:** the plugin builds an SVG per key and rasterises it with `resvg` → PNG → base64
  data URI → `set_image`. A slow loop (1.5s) refreshes state via `kdotool` + the status file; a
  fast loop (200ms) animates the spinner/marquee and only resends an image when it actually changes.

## Requirements

- **OpenDeck** (tested 2.12.x) and a supported device.
- **Rust** toolchain (build), **`kdotool`** (`cargo install kdotool`) for Wayland window focus.
- **kitty** and/or **alacritty** (the launchable terminals).
- **claude** (Claude Code CLI) on `PATH`.
- **fontconfig** (`fc-list`) for the font picker; **Adwaita Mono** installed (carries the braille
  spinner glyphs); **DejaVu Sans / Sans Mono** for the key text.
- Environment: **KDE Plasma 6 / kwin_wayland** (the focus mechanism is KWin-specific).

## Build & install

```sh
./install.sh          # cargo build --release, stage binary into the bundle,
                      # copy bundle to ~/.config/opendeck/plugins/, deploy the statusline helper
```
Then **restart OpenDeck** (fully quit + reopen) so it loads/reloads the plugin. Drag the
**ClaudeDeck → Claude Session** action onto a key.

> The binary is hot-swapped with a temp-file + `mv` so install works even while OpenDeck is running
> (a plain `cp` would fail with "Text file busy"). OpenDeck still needs a restart to *run* the new
> build. **Never** use OpenDeck's "Uninstall" on a plugin you're iterating — it deletes the whole
> bundle folder; just re-run `install.sh` and restart.

## Configuration (per-button, via the property inspector)

| Field | Meaning | Default |
|-------|---------|---------|
| Directory | working dir to launch `claude` in | `$HOME` |
| Terminal | `kitty` or `alacritty` | kitty |
| Font / Font size | terminal font (passed as `-o` overrides) | terminal default |
| Warn threshold % | where the bar hits red | 60 |
| Title | `scroll` (marquee) or `short` (truncate) | scroll |
| Scroll every (s) | marquee cadence; 0 = continuous | 0 |
| Scroll speed | marquee px/sec | 28 |
| Text size | title font px on the key | 14 |
| Claude args | extra CLI args appended to `claude` | — |

> Note: OpenDeck only attaches the property inspector to action instances created **after** the
> plugin's manifest gained a `PropertyInspectorPath`. To configure pre-existing keys, re-add them.

## Project layout

```
claudedeck/
├── Cargo.toml                       # deps: openaction, tokio, resvg, base64, dashmap, serde, simplelog
├── src/main.rs                      # the whole plugin (one file)
├── scripts/statusline.py            # statusLine writer (pct + model -> per-key file)
├── install.sh                       # build + install bundle + statusline helper
└── plugin/dev.project23.claudedeck.sdPlugin/
    ├── manifest.json                # action def + PropertyInspectorPath
    ├── assets/                      # claude-placeholder.png/svg, icon.png
    └── propertyInspector/           # session.html (config UI) + sdpi.css
```

## Dev tips

- **Preview key art without the device:** `./target/release/claudedeck --render-sample` writes
  `/tmp/rk0..6.png` (4 live states + 3 idle). Montage/zoom them with ImageMagick to eyeball at 64px.
- **Tunable knobs** are constants near the top of `src/main.rs`: palette (`BG FG SPIN ACCENT TRACK
  RAMP`), `TITLE_FONT_PX`, `SCROLL_PXPS`, `SPINNER_FRAMES` / `SPINNER_FRAME_MS`, loop cadences
  (`SLOW_MS` / `FAST_MS`), `GRACE_MISSES`. The whole key layout is `build_key_svg` / `idle_svg`.
- **Logs:** `~/.local/share/opendeck/logs/plugins/dev.project23.claudedeck.sdPlugin.log`.

See **STATUS.md** for the iteration history, known limits, and roadmap, and
**OPENDECK_PLUGIN_GUIDE.md** for a reusable guide to building *other* OpenDeck plugins.
