# ClaudeDeck — Status

Last updated: 2026-06-07. Built iteratively, one small slice per iteration, confirmed on real
hardware before extending.

## Current state: working, in active use

All of iterations 1–5 are implemented, installed, and confirmed working on the user's AKP03E.
The plugin launches/focuses sessions, tracks context %, renders rich key faces with animation,
and is configurable per button.

## Iteration history

- **Iter 1 — launch / focus.** Press a key → spawn a terminal running `claude`; press again →
  focus that window. Window identity via a unique Wayland app-id (`--class`); focus via `kdotool`.
- **Iter 2 — context %.** A scoped `claude --settings` statusline (`scripts/statusline.py`) writes
  each session's `context_window.used_percentage` to a per-key file keyed by the `CLAUDEDECK_KEY`
  env; the plugin polls it. Label taken from the window title.
- **Iter 3 — per-button config.** Property inspector (`session.html` + `sdpi.css`): directory,
  terminal (kitty/alacritty), font + size. Fonts enumerated by the plugin via `fc-list :spacing=100`.
- **Iter 4 — rendered key faces.** Switched from `set_title` to plugin-rendered images
  (`set_image`, SVG→`resvg`→PNG). Coloured progress bar (Claude palette ramp), model badge,
  animated braille spinner / blue ready-dot, scrolling title. Configurable threshold + title mode.
  Two-loop design (slow kdotool/state @1.5s, fast render @200ms with change-detection).
- **Iter 5 — polish + config.** Spinner flush top-right; ready dot blue (was green); square corners
  everywhere; idle/pre-launch face shows the sparkle icon + launch-dir basename; settings for scroll
  speed, title text size, and extra `claude` CLI args. Plus a debounce (`GRACE_MISSES`) so a
  transient `kdotool` miss no longer flashes a live key back to the placeholder.

## The device (Ajazz AKP03E / Mirabox N3) — important quirks

- Presents to OpenDeck as a **3×3 grid (9 cells)** + 3 knobs: 6 are LCD keys, 3 are plain buttons.
  No touchscreen. ~3–4mm gap between keys.
- **The 6 keys are physical caps over ONE shared LCD panel** (a unified framebuffer). The driver
  paints each key's image into a fixed-origin slot at the image's *literal* decoded size, with no
  clipping — so an oversized image **bleeds into neighbouring keys**.
- **Effective per-key resolution ≈ 64×64.** Upstream `4ndv/opendeck-akp03` shipped `(60,60)`
  (a slight undershoot); patching the device backend's tile size to `(64,64)` is crisper with no
  bleed (keep a backup of the original device-backend binary for revert).
  - We render the key SVG at 128×128 and let OpenDeck/driver downscale to 64 (supersampling/AA).
  - **Do not exceed ~64** — larger bleeds across keys (this was discovered the hard way).
- ClaudeDeck is a generic action and works unchanged on larger LCD-key devices (e.g. a 15-key AKP153).

## Known limitations / quirks

- **KWin/Wayland only.** Focus relies on `kdotool` (KWin scripting). `wmctrl`/`xdotool` don't work
  on Wayland. Not portable to X11/GNOME without a different focus path.
- **Terminology is unsupported** as the launch terminal: it runs a single-instance server (no
  trackable new window) and has no stable per-window app-id. Use kitty (default) or alacritty,
  which set a unique app-id via `--class`.
- **Busy/idle is heuristic** — inferred from the window title's leading glyph. If Claude changes its
  title glyphs, update `detect_state` and `SPINNER_FRAMES` (see `reference_claude_palette` memory).
- **No auto session-summary** exists in the statusline JSON; the label is the window title, which
  the user sets meaningfully via Claude's `/rename`. Don't build elaborate auto-summarisation.
- **PI on pre-existing keys:** OpenDeck attaches a property inspector only to instances created
  after the manifest had `PropertyInspectorPath`. Re-add old keys to configure them.
- **Single-user / single-machine assumptions:** hard-coded `~/.cargo/bin/kdotool` fallback, helper
  paths under `$HOME/.local/share/claudedeck`, palette/glyphs from a specific Claude Code version.
- `claude_args` is split on whitespace (no shell quoting). Fine for simple flags; add `shell-words`
  if quoted values are needed.

## Roadmap / deferred

- **opencode support** (run the same key UX against locally-run AIs via `opencode` instead of
  `claude`). Will need a terminal/command abstraction and a different status source.
- **Unified multi-key dashboard** for the 15-key device: slice one design across keys, *gap-aware*
  (account for the inter-key dead space so art aligns across seams).
- Aesthetic dial-in (the user iterates on this directly — see the tunable constants in `main.rs`).
- Possibly: detect empty-title vs no-window distinctly (split liveness from label into two kdotool
  calls) if the debounce ever proves insufficient.

## Where to change things (src/main.rs)

- **Colours / palette:** `BG FG SPIN ACCENT TRACK RAMP` consts.
- **Sizes / speeds:** `TITLE_FONT_PX`, `SCROLL_PXPS` (defaults; both overridable per-button),
  `SPINNER_FRAMES` / `SPINNER_FRAME_MS`, `SLOW_MS` / `FAST_MS`, `GRACE_MISSES`.
- **Layout:** `build_key_svg` (live face), `idle_svg` (pre-launch face), `indicator_svg`,
  `title_svg`, `badge_svg`, `bar_color`.
- **Launch command:** `build_launch_command` (terminal/font/dir/args).
- **State detection:** `window_raw_title`, `detect_state`, `read_status`, `model_badge`.
- **Settings:** the `Settings` struct + `*_of` accessor fns; mirror any new field in
  `propertyInspector/session.html` (read it, add to the `setSettings` payload, add a field).
