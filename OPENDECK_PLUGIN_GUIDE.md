# Building OpenDeck plugins — a field guide

A distilled, reusable rundown of everything learned building **ClaudeDeck**, written so an agent can
build a *different* OpenDeck plugin (for any use case) without re-discovering the same things. The
host environment here is **Arch Linux, KDE Plasma 6 / kwin_wayland**, OpenDeck 2.12.x, with an
**Ajazz AKP03E / Mirabox N3** macro pad — adjust device/WM specifics as needed.

---

## 0. Safety first — hardware is not reversible like code

Before any of the fun stuff: **a device can be permanently bricked.** Some of these
Stream-Deck-style decks store a persistent boot image / config in onboard flash, and a
bad write there can wedge the device at boot so it never re-enumerates — with no
user-accessible recovery. Code has `git revert`; a bad flash write has nothing. So when
you build or probe a device plugin:

- **Stay non-invasive by default.** Send only the *volatile* traffic the stock driver
  uses — draw images, read input, read firmware, set brightness. Nothing that survives a
  power-cycle.
- **Persistent / irreversible commands need explicit user sign-off *before* you send
  them** — flash/boot-image writes, firmware, config-persist, anything that outlives a
  reboot. Flag such a command as different from the safe ones *out loud* before running
  it; don't bury a one-way-door action in a list of safe ones.
- **Treat un-understood vendor/protocol commands as presumed-destructive.** "Probably
  RAM-only" is not safe enough when the downside is a dead device. Prove volatility
  (power-cycle, confirm it's gone) before relying on it.
- **Reverse-engineering by driving your only device with the vendor library can brick
  it** — a normal-looking "set background image" call can turn out to be a flash write.
  Isolate which commands persist before exercising them on hardware you can't afford to
  lose.
- **Don't invent requirements** (like "make the art persist across reboots") and then
  reach for an irreversible command to satisfy a goal nobody asked for.

The cost of pausing to ask is one sentence; the cost of guessing wrong is the device.
Keep this in mind for every section below — `set_image` and friends are safe; anything
that writes device state is not, until proven and approved.

---

## 1. The plugin model

OpenDeck is an open-source Stream Deck controller. Plugins are **separate processes** that OpenDeck
spawns and talks to over a local WebSocket using the **OpenAction** protocol (a superset of Elgato's
Stream Deck SDK). A plugin is a `*.sdPlugin` **folder** containing a `manifest.json`, the executable,
assets, and an optional property-inspector web page.

Two flavours of plugin:
- **Action plugins** (what you usually build): define one or more *actions* the user drags onto keys.
- **Device backend plugins** (e.g. `st.lynx.plugins.opendeck-akp03`): drive a specific hardware
  device. You rarely write these, but you may need to *patch* one (see §7).

Plugins live in `~/.config/opendeck/plugins/<uuid>.sdPlugin/`. OpenDeck scans that dir on startup
and registers each bundle (watch `~/.local/share/opendeck/logs/opendeck.log`).

---

## 2. Bundle anatomy & manifest.json

```
com.you.myplugin.sdPlugin/
├── manifest.json
├── <binary per platform>            # e.g. myplugin-x86_64-unknown-linux-gnu
├── assets/ or img/ or icons/        # PNG/SVG; referenced WITHOUT extension
└── propertyInspector/               # optional config UI: *.html + sdpi.css
```

`manifest.json` (verified working minimal Linux form):
```json
{
  "Name": "My Plugin",
  "Author": "You",
  "Version": "1.0.0",
  "Description": "…",
  "Category": "My Plugin",
  "Icon": "assets/icon",
  "CodePaths": { "x86_64-unknown-linux-gnu": "myplugin-x86_64-unknown-linux-gnu" },
  "CodePathLin": "myplugin-x86_64-unknown-linux-gnu",
  "OS": [{ "Platform": "linux" }],
  "Actions": [
    {
      "UUID": "com.you.myplugin.action1",
      "Name": "My Action",
      "Icon": "assets/action1",
      "Tooltip": "…",
      "Controllers": ["Keypad"],           // and/or "Encoder" for dials
      "PropertyInspectorPath": "propertyInspector/action1.html",
      "States": [{ "Image": "assets/action1" }]
    }
  ]
}
```
- `Icon`/`Image` reference files **without extension**; OpenDeck resolves `.png`/`.svg`.
- `CodePathLin` is the Linux entry binary; `CodePaths` maps target triples (add mac/win if shipping).
- A device backend manifest instead carries `PluginUUID` + `DeviceNamespace` and an empty `Actions`.

---

## 3. The `openaction` Rust crate (plugin runtime)

Add `openaction = "2.6"` (re-exports `async_trait`, `usvg`/etc. are separate). Pattern:

```rust
use openaction::*;

struct MyAction;

#[async_trait]
impl Action for MyAction {
    const UUID: &'static str = "com.you.myplugin.action1";   // must match manifest
    type Settings = MySettings;                              // Serialize+Deserialize+Default(+Clone)

    async fn key_down(&self, i: &Instance, s: &Self::Settings) -> OpenActionResult<()> { Ok(()) }
    // also: key_up, will_appear, will_disappear, dial_rotate/down/up,
    //       did_receive_settings, send_to_plugin, property_inspector_did_(appear|disappear),
    //       title_parameters_did_change
}

#[tokio::main]
async fn main() -> OpenActionResult<()> {
    register_action(MyAction).await;
    // spawn your own background loops here if needed
    run(std::env::args().collect()).await   // parses -port/-pluginUUID/-registerEvent/-info, connects
}
```

**`Instance`** (the per-button context) exposes:
- Fields: `action_uuid`, `instance_id` (the OpenDeck "context" string), `device_id`,
  `controller`, `coordinates: Option<Coordinates{row:u8, column:u8}>`, `is_in_multi_action`,
  `current_state_index`.
- Methods (async): `set_title(Option<impl Into<String>>, Option<u16>)`,
  `set_image(Option<impl Into<String>>, Option<u16>)` (image = a data URI; `None` reverts to the
  manifest state image), `set_state(u16)`, `show_alert`, `show_ok`, `set_settings`, `get_settings`,
  `send_to_property_inspector(impl Serialize)`.
- Free fns: `register_action`, `run`, `visible_instances(uuid) -> Vec<Arc<Instance>>`,
  `get_instance(id) -> Option<Arc<Instance>>`, `get_connected_devices`.

**GOTCHA — settings aren't readable off an `Instance`.** `Instance.settings_json` is `pub(crate)`.
Handlers receive `&Self::Settings` (deserialized for you), but a background loop iterating
`visible_instances()` cannot read them. **Cache settings yourself**: in `will_appear` /
`did_receive_settings`, store `settings.clone()` in a `DashMap<instance_id, Settings>`; remove in
`will_disappear`. (Requires `Settings: Clone`.)

**Background rendering pattern** (used by ClaudeDeck): one slow loop polls external state into a
shared map; one fast loop iterates `visible_instances()`, builds each key's image, and calls
`set_image` **only when the rendered bytes change** (hash the SVG/string; skip if unchanged) — this
keeps the device's USB update rate and CPU sane.

---

## 4. Property Inspector (config UI)

The PI is a plain HTML page (`PropertyInspectorPath`) loaded in OpenDeck's WebView. Use the standard
Elgato handshake (copy `sdpi.css` from any installed plugin for native styling):

```html
<script>
  let update = () => {};
  function connectElgatoStreamDeckSocket(inPort, inUUID, inRegisterEvent, inInfo, inActionInfo) {
    const ws = new WebSocket("ws://localhost:" + inPort);
    inActionInfo = JSON.parse(inActionInfo);
    const ctx = inActionInfo.context, action = inActionInfo.action;
    const s = inActionInfo.payload.settings || {};
    // populate fields from s.<key> …
    ws.onopen = () => {
      ws.send(JSON.stringify({ event: inRegisterEvent, uuid: inUUID }));
      // optional: ask the plugin for data (handled in send_to_plugin):
      ws.send(JSON.stringify({ event: "sendToPlugin", action, context: ctx, payload: { request: "x" } }));
    };
    ws.onmessage = (e) => { const d = JSON.parse(e.data);
      if (d.event === "sendToPropertyInspector") { /* use d.payload */ } };
    update = () => ws.send(JSON.stringify({ event: "setSettings", context: ctx, payload: { /* fields */ } }));
  }
</script>
```
- **Persist** with `setSettings` (the plugin then gets `did_receive_settings` and updated settings
  on each event).
- **PI ⇄ plugin messaging:** PI sends `{event:"sendToPlugin", action, context, payload}` → plugin
  `send_to_plugin(instance, settings, payload)`; plugin replies with
  `instance.send_to_property_inspector(json)` → PI `onmessage` sees
  `{event:"sendToPropertyInspector", context, payload}`. (ClaudeDeck uses this to send the system
  font list from `fc-list` to a dropdown.)
- **GOTCHA:** OpenDeck only attaches the PI to instances created **after** the manifest gained
  `PropertyInspectorPath`. Pre-existing keys must be re-added.

---

## 5. Installing & reloading

- **Install** = copy the `.sdPlugin` folder into `~/.config/opendeck/plugins/` and **restart
  OpenDeck** (fully quit from tray + reopen). A dev `install.sh` that does `cargo build --release`,
  stages the binary, and rsyncs the bundle is the fastest loop.
- **ETXTBSY:** you can't `cp` over a running plugin binary. Write to a temp name and `mv` (rename
  swaps the dir entry without truncating the busy inode). OpenDeck still needs a restart to *run* the
  new build.
- **NEVER use OpenDeck's "Uninstall"** while iterating — it deletes the whole bundle folder (and the
  device's driver if it's a device plugin). Just re-`install.sh` + restart. If you do nuke it,
  reinstall the full folder.
- **Logs:** per-plugin at `~/.local/share/opendeck/logs/plugins/<uuid>.sdPlugin.log` (OpenDeck
  captures the plugin's stderr; init a logger like `simplelog::WriteLogger` to stderr).

---

## 6. Rendering key images

`set_image(Some(data_uri))` accepts a data URI. ClaudeDeck renders **SVG → PNG via `resvg`**:

```rust
// deps: resvg = "0.44", base64 = "0.22"
static FONTDB: LazyLock<Arc<resvg::usvg::fontdb::Database>> = LazyLock::new(|| {
    let mut db = resvg::usvg::fontdb::Database::new(); db.load_system_fonts(); Arc::new(db)  // load ONCE
});
fn render_png(svg: &str) -> Option<Vec<u8>> {
    let mut opt = resvg::usvg::Options::default();
    opt.fontdb = FONTDB.clone();
    let tree = resvg::usvg::Tree::from_str(svg, &opt).ok()?;
    let mut pm = resvg::tiny_skia::Pixmap::new(128, 128)?;                    // render at 2× for AA
    resvg::render(&tree, resvg::tiny_skia::Transform::from_scale(2.0, 2.0), &mut pm.as_mut());
    pm.encode_png().ok()
}
// data URI: format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(&png))
```

Gotchas:
- **`load_system_fonts()` is slow — do it once** (a `LazyLock`/`OnceCell`), never per render.
- **Raw-string trap:** a Rust raw string `r#"…"#` ends at the first `"#`. SVG like `fill="#3a3937"`
  contains `"#` and will terminate the string early — put hex colours in a const and interpolate, or
  use `r##"…"##`.
- **Font fallback spam:** usvg logs a WARN per render when a family lacks a glyph (e.g. DejaVu Sans
  Mono has no braille). Point the element at a font that *has* the glyph (Adwaita Mono has braille),
  and/or filter the `usvg`/`fontdb`/`resvg` log targets (`simplelog::ConfigBuilder::add_filter_ignore_str`).
- **Animation:** derive frames from elapsed time; redraw on a timer but only `set_image` when the
  output changes (hash it). Keep the device update rate modest (ClaudeDeck: 200ms fast loop).
- Simpler alternative to images: `set_title(Some("text"))` draws centred text over the manifest
  image — fine for quick text, but you don't control layout/colour/scroll.

---

## 7. Hardware: per-key resolution & patching a device driver

Lessons from the **Ajazz AKP03E / Mirabox N3** (`4ndv/opendeck-akp03`, on the `mirajazz` crate):
- OpenDeck only learns **rows/columns/encoders** from a device plugin — **never pixel dimensions**.
  The device plugin alone decides the JPEG size it sends, in `image_format()` (per-device, hardcoded).
- The N3's keys are physical caps over **one shared LCD panel**. The firmware paints each key's JPEG
  at its slot origin **at the bitmap's literal size, no clipping** → an oversized image **bleeds into
  neighbouring keys**. So the per-key size is a hard constraint, not a scaling target.
- This device's true tile is **~64×64** (upstream shipped 60; 64 is crisper, 128 bled). To change it,
  patch `vendor/.../src/mappings.rs` `size: (W, H)` (both protocol-version branches), `cargo build
  --release`, and swap the freshly built `*-linux` binary into the installed device-plugin bundle.
  **Keep a backup** of the original binary for revert (ETXTBSY rules from §5 apply — quit OpenDeck or
  temp+mv).
- Different devices on the same backend use different sizes (e.g. AKP153 ≈ 85/95) — always confirm
  per device; don't assume 128 or a power of two.

---

## 8. Window focus on KWin / Wayland

`wmctrl`/`xdotool` **do not work on Wayland**. Use **`kdotool`** (`cargo install kdotool`; it drives
KWin's scripting API). Match windows by a stable **app-id** you control:

```sh
kdotool search --class <app_id>                 # list matching window IDs (liveness)
kdotool search --class <app_id> windowactivate   # focus it
kdotool search --class <app_id> getwindowname    # the window title (one call, liveness + title)
```
- Launch each window with a unique app-id you can re-find: **kitty** `--class <id>` or **alacritty**
  `--class <id>` set a real per-window Wayland `app_id`. The *title* is rewritten by the running
  program, so **match on `--class`/app-id, not the title**.
- **Terminology is unusable** for this: single-instance server (no trackable new window) + fixed
  app-id. Avoid.
- `kdotool` binaries land in `~/.cargo/bin`, which may not be on OpenDeck's PATH — resolve it
  explicitly (`$HOME/.cargo/bin/kdotool`) with a PATH fallback.

---

## 9. Claude Code integration specifics (if relevant to your plugin)

- **statusLine JSON** (piped to a `statusLine.command` on stdin) includes:
  `context_window.used_percentage` (may be null early), `context_window.*` token counts,
  `model.id` / `model.display_name`, `session_id`, `cwd` / `workspace.current_dir`,
  `transcript_path`, `version`, `session_name` (only if the user set one). **No auto session
  summary** field exists.
- **Scope a statusline to your sessions** without touching the user's global config: launch
  `claude --settings <your-file.json>` where that file defines `statusLine.command`. Correlate the
  resulting per-session data to a button via an **env var** you set at launch (the statusline command
  inherits the session's env).
- **Window-title status glyphs:** leading **braille** char (U+2800–28FF) = busy/working; **`✳`**
  (U+2733) = idle/awaiting input. Claude's in-TUI thinking spinner is the 10-frame braille set
  `⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏`.
- **Session label:** users set it with `/rename`; surface that (the window title) rather than trying
  to auto-summarise.
- **Palette** (from the CLI bundle, dark theme): blue `#93a5ff`, coral `#d77757`, error `#ff6b80`,
  success `#4eba65`; meter ramp `#82aadc→#91c882→#fac35f→#f58b57→#eb5f57`.

---

## 10. Reference plugins already on this machine

Read these for working examples (`~/.config/opendeck/plugins/`):
- `com.victormarin.volume-controller.sdPlugin` — real Rust plugin (openaction, images, PI).
- `com.amansprojects.starterpack.sdPlugin` — multi-action Rust starter + `propertyInspector/*.html`
  + `sdpi.css` (the canonical PI handshake — copy from here).
- `info.degois.damien.opendeck.plugins.rotary-command.sdPlugin` — encoder/dial example.
- `st.lynx.plugins.opendeck-akp03.sdPlugin` — the device backend (and ClaudeDeck patches it).

---

## 11. Gotchas checklist

- [ ] Action `UUID` in code **must** equal the manifest UUID, or events never fire.
- [ ] Cache settings yourself for background loops (`Instance.settings_json` is `pub(crate)`).
- [ ] Raw-string `"#` terminates SVG strings — interpolate hex colours.
- [ ] `load_system_fonts()` once, not per render.
- [ ] Use a braille-capable font (Adwaita Mono) for braille glyphs; filter usvg log noise.
- [ ] ETXTBSY on binary install → temp + `mv`; restart OpenDeck to run the new build.
- [ ] Never "Uninstall" via OpenDeck while iterating (deletes the bundle).
- [ ] Wayland: `kdotool` only; match windows by app-id (`--class`), not title.
- [ ] Per-key image size is device-specific and a hard limit (shared-panel bleed) — verify it.
- [ ] PI only attaches to keys created after `PropertyInspectorPath` existed — re-add old keys.

---

## 12. Useful commands

```sh
cargo install kdotool                          # Wayland window control
fc-list :spacing=100 family                    # monospace font families (for a font picker)
./target/release/<bin> --render-sample         # ClaudeDeck dev hook: render key art to /tmp
tail -f ~/.local/share/opendeck/logs/plugins/<uuid>.sdPlugin.log
kdotool search --class <id> getwindowname      # inspect a window's title/liveness
rsvg-convert -w 64 -h 64 in.svg -o out.png     # quick SVG→PNG to prototype key art
```
