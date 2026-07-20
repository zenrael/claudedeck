use base64::Engine;
use dashmap::DashMap;
use openaction::*;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, LazyLock};
use std::time::Duration;

const ACTION_UUID: &str = "dev.project23.claudedeck.session";

// Loop cadences: kdotool/state polling is slow (cheap on KWin); rendering is fast
// (animates the spinner + title marquee) but only sends an image when it changes.
const SLOW_MS: u64 = 1500;
const FAST_MS: u64 = 200;
const SCROLL_PXPS: f32 = 28.0; // default marquee speed (px/sec)
const TITLE_FONT_PX: f32 = 14.0; // default title font size
// Claude's in-TUI braille spinner frames + cadence, mimicked on the key.
const SPINNER_FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
const SPINNER_FRAME_MS: u128 = 120;
// Tolerate this many consecutive "no window" polls before blanking a key, so a
// live key doesn't flash to the placeholder on a transient kdotool miss.
const GRACE_MISSES: u8 = 3;

// Claude Code dark-theme palette (extracted from the CLI bundle).
const BG: &str = "#1a1a1c";
const FG: &str = "#ededeb";
const SPIN: &str = "#93a5ff"; // Claude blue: busy spinner + ready/waiting dot
const ACCENT: &str = "#d77757"; // Claude coral (model family letter)
const TRACK: &str = "#3a3937"; // progress-bar track (empty part)
// Progress-bar ramp blue -> green -> yellow -> orange -> red (Claude "rainbow" set).
const RAMP: [(f32, (u8, u8, u8)); 5] = [
    (0.0, (130, 170, 220)),
    (0.25, (145, 200, 130)),
    (0.5, (250, 195, 95)),
    (0.75, (245, 139, 87)),
    (1.0, (235, 95, 87)),
];

/// Per-instance settings, configured via the property inspector. All strings
/// (the PI sends strings); empty means "use default".
#[derive(Default, Clone, Serialize, Deserialize)]
#[serde(default)]
struct Settings {
    directory: String,
    terminal: String,
    font: String,
    font_size: String,
    /// Context-usage % at which the bar reaches red. Empty/invalid => 60.
    threshold: String,
    /// "scroll" (default) marquees long titles; "short" truncates.
    title_mode: String,
    /// Seconds between marquee plays. Empty/0 => scroll continuously.
    scroll_secs: String,
    /// Marquee speed in px/sec. Empty/invalid => default.
    scroll_speed: String,
    /// Title font size in px. Empty/invalid => default.
    font_px: String,
    /// Extra command-line arguments passed to `claude` at launch.
    claude_args: String,
    /// Pre-launch sparkle colour: palette name (blue/periwinkle/red/green/amber);
    /// empty or unknown => coral default.
    sparkle_color: String,
    /// Shell command run before `claude` at launch (e.g. `source ./.env`), via
    /// `bash -c`. Empty => exec claude directly with no shell.
    prerun: String,
}

fn scroll_secs_of(s: &Settings) -> f32 {
    s.scroll_secs.trim().parse::<f32>().ok().filter(|&n| n >= 0.0).unwrap_or(0.0)
}

fn scroll_speed_of(s: &Settings) -> f32 {
    s.scroll_speed.trim().parse::<f32>().ok().filter(|&n| (2.0..=400.0).contains(&n)).unwrap_or(SCROLL_PXPS)
}

fn font_px_of(s: &Settings) -> f32 {
    s.font_px.trim().parse::<f32>().ok().filter(|&n| (6.0..=28.0).contains(&n)).unwrap_or(TITLE_FONT_PX)
}

fn threshold_of(s: &Settings) -> f32 {
    s.threshold
        .trim()
        .parse::<f32>()
        .ok()
        .filter(|&n| n > 0.0 && n <= 100.0)
        .unwrap_or(60.0)
}

fn scroll_mode(s: &Settings) -> bool {
    s.title_mode.trim() != "short"
}

/// Map the per-button sparkle-colour setting to a palette hex (default = coral ACCENT).
fn sparkle_color_of(s: &Settings) -> &'static str {
    match s.sparkle_color.trim() {
        "blue" => "#82aadc",
        "periwinkle" => "#93a5ff",
        "red" => "#ff6b80",
        "green" => "#4eba65",
        "amber" => "#fac35f",
        _ => ACCENT,
    }
}

#[derive(Clone, Default)]
struct KeyState {
    alive: bool,
    busy: bool,
    attn: bool,
    label: String,
    pct: Option<i64>,
    model_badge: String,
}

static KEY_STATE: LazyLock<DashMap<String, KeyState>> = LazyLock::new(DashMap::new);
static MISSES: LazyLock<DashMap<String, u8>> = LazyLock::new(DashMap::new);
static SETTINGS: LazyLock<DashMap<String, Settings>> = LazyLock::new(DashMap::new);
static LAST_SIG: LazyLock<DashMap<String, u64>> = LazyLock::new(DashMap::new);
static START: LazyLock<std::time::Instant> = LazyLock::new(std::time::Instant::now);
static FONTDB: LazyLock<Arc<resvg::usvg::fontdb::Database>> = LazyLock::new(|| {
    let mut db = resvg::usvg::fontdb::Database::new();
    db.load_system_fonts();
    Arc::new(db)
});

struct ClaudeSession;

#[async_trait]
impl Action for ClaudeSession {
    const UUID: &'static str = ACTION_UUID;
    type Settings = Settings;

    async fn will_appear(&self, instance: &Instance, settings: &Self::Settings) -> OpenActionResult<()> {
        SETTINGS.insert(instance.instance_id.clone(), settings.clone());
        Ok(())
    }

    async fn will_disappear(&self, instance: &Instance, _settings: &Self::Settings) -> OpenActionResult<()> {
        SETTINGS.remove(&instance.instance_id);
        let app_id = app_id_for(instance);
        KEY_STATE.remove(&app_id);
        LAST_SIG.remove(&app_id);
        MISSES.remove(&app_id);
        Ok(())
    }

    async fn did_receive_settings(&self, instance: &Instance, settings: &Self::Settings) -> OpenActionResult<()> {
        SETTINGS.insert(instance.instance_id.clone(), settings.clone());
        Ok(())
    }

    async fn key_down(&self, instance: &Instance, settings: &Self::Settings) -> OpenActionResult<()> {
        let app_id = app_id_for(instance);
        if window_raw_title(&app_id).await.is_some() {
            log::info!("[{app_id}] window exists -> focus");
            focus_window(&app_id).await;
        } else {
            log::info!("[{app_id}] no window -> launch");
            launch_session(&app_id, settings).await;
        }
        Ok(())
    }

    async fn send_to_plugin(
        &self,
        instance: &Instance,
        _settings: &Self::Settings,
        payload: &serde_json::Value,
    ) -> OpenActionResult<()> {
        if payload.get("request").and_then(|v| v.as_str()) == Some("fonts") {
            let fonts = list_monospace_fonts().await;
            instance
                .send_to_property_inspector(serde_json::json!({ "fonts": fonts }))
                .await?;
        }
        Ok(())
    }
}

/// Stable, unique Wayland app-id for this button instance (claude rewrites the
/// window *title* but never the app-id, so we match on this).
fn app_id_for(instance: &Instance) -> String {
    let base = match &instance.coordinates {
        Some(c) => format!("{}-r{}-c{}", instance.device_id, c.row, c.column),
        None => instance.instance_id.clone(),
    };
    let cleaned: String = base
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '-' })
        .collect();
    format!("claudedeck-{cleaned}")
}

fn home() -> String {
    std::env::var("HOME").unwrap_or_else(|_| "/".to_string())
}

fn status_dir() -> std::path::PathBuf {
    let rt = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp/claudedeck".to_string());
    std::path::Path::new(&rt).join("claudedeck")
}

fn kdotool_bin() -> String {
    let p = format!("{}/.cargo/bin/kdotool", home());
    if std::path::Path::new(&p).exists() {
        p
    } else {
        "kdotool".to_string()
    }
}

/// Raw window title (leading status glyph intact) if a window exists for this
/// app-id, else None. Also the liveness check.
async fn window_raw_title(app_id: &str) -> Option<String> {
    let out = tokio::process::Command::new(kdotool_bin())
        .args(["search", "--class", app_id, "getwindowname"])
        .output()
        .await
        .ok()?;
    let raw = String::from_utf8_lossy(&out.stdout);
    let line = raw.lines().next().unwrap_or("").trim_end();
    if line.trim().is_empty() {
        None
    } else {
        Some(line.to_string())
    }
}

/// (busy, attn) from the title's leading glyph: braille (U+2800-28FF) = working,
/// `✳` (U+2733) = idle/awaiting input.
fn detect_state(raw: &str) -> (bool, bool) {
    match raw.trim_start().chars().next() {
        Some(c) if ('\u{2800}'..='\u{28FF}').contains(&c) => (true, false),
        Some('\u{2733}') => (false, true),
        _ => (false, false),
    }
}

fn clean_label(s: &str) -> String {
    s.trim_start_matches(|c: char| !c.is_alphanumeric()).trim().to_string()
}

fn read_status(app_id: &str) -> (Option<i64>, String) {
    let path = status_dir().join(format!("{app_id}.json"));
    let Ok(text) = std::fs::read_to_string(path) else {
        return (None, String::new());
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else {
        return (None, String::new());
    };
    let pct = v
        .get("pct")
        .and_then(|p| p.as_i64().or_else(|| p.as_f64().map(|f| f.round() as i64)));
    let model = v.get("model").and_then(|m| m.as_str()).unwrap_or("").to_string();
    (pct, model)
}

/// "claude-opus-4-8" -> "O4.8".
fn model_badge(model_id: &str) -> String {
    if model_id.is_empty() {
        return String::new();
    }
    let id = model_id.to_lowercase();
    let fam = if id.contains("opus") {
        "O"
    } else if id.contains("sonnet") {
        "S"
    } else if id.contains("haiku") {
        "H"
    } else {
        ""
    };
    let ver: Vec<&str> = id
        .split(|c: char| !c.is_ascii_digit())
        .filter(|s| !s.is_empty() && s.len() <= 2)
        .collect();
    let v = match ver.as_slice() {
        [a, b, ..] => format!("{a}.{b}"),
        [a] => a.to_string(),
        _ => String::new(),
    };
    format!("{fam}{v}")
}

async fn focus_window(app_id: &str) {
    if let Err(e) = tokio::process::Command::new(kdotool_bin())
        .args(["search", "--class", app_id, "windowactivate"])
        .status()
        .await
    {
        log::error!("kdotool windowactivate failed: {e}");
    }
}

async fn list_monospace_fonts() -> Vec<String> {
    let out = tokio::process::Command::new("fc-list")
        .args([":spacing=100", "family"])
        .output()
        .await;
    let mut set = std::collections::BTreeSet::new();
    if let Ok(o) = out {
        for line in String::from_utf8_lossy(&o.stdout).lines() {
            let family = line.split(',').next().unwrap_or("").trim();
            if family.is_empty() || family.contains("Emoji") || family.contains("SignWriting") {
                continue;
            }
            set.insert(family.to_string());
        }
    }
    set.into_iter().collect()
}

/// systemd/runtime-injected vars that must not leak into the transient unit we
/// create (they belong to OpenDeck's own unit, not the terminal's).
const ENV_BLOCKLIST: &[&str] = &[
    "INVOCATION_ID", "JOURNAL_STREAM", "MANAGERPID", "NOTIFY_SOCKET",
    "LISTEN_PID", "LISTEN_FDS", "LISTEN_FDNAMES", "SYSTEMD_EXEC_PID",
    "WATCHDOG_PID", "WATCHDOG_USEC", "MEMORY_PRESSURE_WATCH", "MEMORY_PRESSURE_WRITE",
];

fn systemd_run_bin() -> Option<&'static str> {
    ["/usr/bin/systemd-run", "/bin/systemd-run"]
        .into_iter()
        .find(|p| std::path::Path::new(p).exists())
}

/// Wrap a terminal invocation so it launches **detached** in its own transient
/// `systemd --user` service — a fresh cgroup off OpenDeck's, so an OpenDeck
/// stop/restart (its unit is KillMode=control-group) can't take the session down
/// with it. The plugin's environment is carried across so the terminal + claude
/// see what they'd see as a direct child. Focus + state tracking are unaffected:
/// both key off the window's `--class` app-id via kdotool (oblivious to process/
/// cgroup topology) and the status file in $XDG_RUNTIME_DIR (path unchanged), so
/// a fresh plugin re-attaches to a surviving session automatically. Falls back to
/// a direct child spawn where systemd-run isn't present.
fn detached_command(
    term_argv: &[String],
    dir: &str,
    extra_env: &[(&str, String)],
) -> tokio::process::Command {
    if let Some(sr) = systemd_run_bin() {
        let mut cmd = tokio::process::Command::new(sr);
        cmd.args(["--user", "--quiet", "--collect"]);
        cmd.arg(format!("--working-directory={dir}"));
        for (k, v) in std::env::vars() {
            if ENV_BLOCKLIST.contains(&k.as_str()) || k.contains('\n') || v.contains('\n') {
                continue;
            }
            cmd.arg(format!("--setenv={k}={v}"));
        }
        for (k, v) in extra_env {
            cmd.arg(format!("--setenv={k}={v}"));
        }
        cmd.arg("--");
        cmd.args(term_argv);
        cmd
    } else {
        let mut cmd = tokio::process::Command::new(&term_argv[0]);
        cmd.args(&term_argv[1..]);
        cmd.current_dir(dir);
        for (k, v) in extra_env {
            cmd.env(k, v);
        }
        cmd
    }
}

fn build_launch_command(app_id: &str, settings: &Settings) -> tokio::process::Command {
    let home = home();
    let claude_settings = format!("{home}/.local/share/claudedeck/session-settings.json");

    let dir = {
        let d = settings.directory.trim();
        if d.is_empty() {
            home.clone()
        } else if std::path::Path::new(d).is_dir() {
            d.to_string()
        } else {
            log::warn!("configured directory '{d}' not found; using $HOME");
            home.clone()
        }
    };

    let font = settings.font.trim();
    let size = settings.font_size.trim();
    let size_ok = size.parse::<f32>().map(|n| n > 0.0).unwrap_or(false);
    let terminal = match settings.terminal.trim() {
        "alacritty" => "alacritty",
        _ => "kitty",
    };

    let extra: Vec<&str> = settings.claude_args.split_whitespace().collect();
    let prerun = settings.prerun.trim();

    // Build the terminal argv: terminal + its options + the program to run.
    let mut argv: Vec<String> =
        vec![terminal.to_string(), "--class".to_string(), app_id.to_string()];

    // Terminal font options (terminal-specific syntax); alacritty also needs `-e`
    // before the program it should run.
    match terminal {
        "alacritty" => {
            if !font.is_empty() {
                argv.push("-o".into());
                argv.push(format!("font.normal.family=\"{font}\""));
            }
            if size_ok {
                argv.push("-o".into());
                argv.push(format!("font.size={size}"));
            }
            argv.push("-e".into());
        }
        _ => {
            if !font.is_empty() {
                argv.push("-o".into());
                argv.push(format!("font_family={font}"));
            }
            if size_ok {
                argv.push("-o".into());
                argv.push(format!("font_size={size}"));
            }
        }
    }

    // With a pre-run command, go through `bash -c` so the user can e.g. `source ./.env`;
    // otherwise exec claude directly (no shell).
    if prerun.is_empty() {
        argv.push("claude".into());
        argv.extend(extra.iter().map(|s| s.to_string()));
        argv.push("--settings".into());
        argv.push(claude_settings);
    } else {
        let shell_cmd =
            format!("{prerun} && exec claude {} --settings '{claude_settings}'", extra.join(" "));
        argv.push("bash".into());
        argv.push("-c".into());
        argv.push(shell_cmd);
    }

    // Launch detached in its own systemd scope so an OpenDeck restart can't kill the session.
    detached_command(&argv, &dir, &[("CLAUDEDECK_KEY", app_id.to_string())])
}

async fn launch_session(app_id: &str, settings: &Settings) {
    match build_launch_command(app_id, settings).spawn() {
        Ok(mut child) => {
            log::info!("launched session (app_id={app_id}) pid={:?}", child.id());
            tokio::spawn(async move {
                let _ = child.wait().await;
            });
        }
        Err(e) => log::error!("failed to launch session: {e}"),
    }
}

// ---- rendering ----------------------------------------------------------

fn lerp(a: u8, b: u8, f: f32) -> u8 {
    (a as f32 + (b as f32 - a as f32) * f).round().clamp(0.0, 255.0) as u8
}

fn sample_ramp(t: f32) -> (u8, u8, u8) {
    let t = t.clamp(0.0, 1.0);
    for i in 0..RAMP.len() - 1 {
        let (t0, c0) = RAMP[i];
        let (t1, c1) = RAMP[i + 1];
        if t <= t1 {
            let f = if t1 > t0 { (t - t0) / (t1 - t0) } else { 0.0 };
            return (lerp(c0.0, c1.0, f), lerp(c0.1, c1.1, f), lerp(c0.2, c1.2, f));
        }
    }
    RAMP[RAMP.len() - 1].1
}

fn bar_color(pct: f32, threshold: f32) -> String {
    let band = ((pct / 10.0).floor() * 10.0).min(100.0); // quantise to 10% bands
    let t = if threshold > 0.0 { (band / threshold).min(1.0) } else { 0.0 };
    let c = sample_ramp(t);
    format!("#{:02x}{:02x}{:02x}", c.0, c.1, c.2)
}

fn xml_esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let head: String = s.chars().take(max.saturating_sub(1)).collect();
        format!("{head}…")
    }
}

fn scroll_offset(period: f32, scroll_secs: f32, speed: f32, elapsed_ms: u128) -> f32 {
    let secs = elapsed_ms as f32 / 1000.0;
    if scroll_secs <= 0.0 {
        return (secs * speed) % period; // continuous
    }
    // Periodic: scroll once, then rest showing the start until the interval elapses.
    let scroll_dur = period / speed;
    let cycle = scroll_secs.max(scroll_dur + 0.5);
    let phase = secs % cycle;
    if phase < scroll_dur {
        phase * speed
    } else {
        0.0
    }
}

fn title_svg(label: &str, scroll: bool, scroll_secs: f32, speed: f32, font_px: f32, elapsed_ms: u128) -> String {
    if label.is_empty() {
        return String::new();
    }
    let char_px = font_px * 0.55;
    let text_w = label.chars().count() as f32 * char_px;
    let inner_x = 3.0;
    let inner_w = 58.0;
    let y = 37.0;
    if !scroll || text_w <= inner_w {
        let maxc = (inner_w / char_px).floor() as usize;
        let shown = xml_esc(&truncate_chars(label, maxc.max(1)));
        return format!(
            r#"<text x="{inner_x}" y="{y}" font-family="DejaVu Sans" font-size="{font_px}" fill="{FG}">{shown}</text>"#
        );
    }
    let esc = xml_esc(label);
    let gap = 18.0;
    let period = text_w + gap;
    let off = scroll_offset(period, scroll_secs, speed, elapsed_ms);
    let x1 = inner_x - off;
    let x2 = x1 + period;
    format!(
        r#"<g clip-path="url(#tc)"><text x="{x1:.1}" y="{y}" font-family="DejaVu Sans" font-size="{font_px}" fill="{FG}">{esc}</text><text x="{x2:.1}" y="{y}" font-family="DejaVu Sans" font-size="{font_px}" fill="{FG}">{esc}</text></g>"#
    )
}

fn indicator_svg(state: &KeyState, elapsed_ms: u128) -> String {
    if state.busy {
        let i = ((elapsed_ms / SPINNER_FRAME_MS) % SPINNER_FRAMES.len() as u128) as usize;
        // Adwaita Mono carries the braille glyphs (DejaVu Sans Mono doesn't).
        format!(
            r#"<text x="54" y="17" font-family="Adwaita Mono, DejaVu Sans Mono, monospace" font-size="19" font-weight="700" fill="{SPIN}">{}</text>"#,
            SPINNER_FRAMES[i]
        )
    } else if state.attn {
        format!(r#"<circle cx="56" cy="11" r="5" fill="{SPIN}"/>"#)
    } else {
        String::new()
    }
}

fn badge_svg(badge: &str) -> String {
    if badge.is_empty() {
        return String::new();
    }
    let mut chars = badge.chars();
    let first = chars.next().unwrap().to_string();
    let rest: String = chars.collect();
    format!(
        r#"<text x="4" y="15" font-family="DejaVu Sans Mono" font-size="13" font-weight="700" fill="{FG}"><tspan fill="{ACCENT}">{}</tspan>{}</text>"#,
        xml_esc(&first),
        xml_esc(&rest)
    )
}

#[allow(clippy::too_many_arguments)]
fn build_key_svg(
    state: &KeyState,
    threshold: f32,
    scroll: bool,
    scroll_secs: f32,
    speed: f32,
    font_px: f32,
    elapsed_ms: u128,
) -> String {
    let pct = state.pct.unwrap_or(0).clamp(0, 100) as f32;
    let bx0 = 4.0;
    let bw = 56.0;
    let fill = bw * pct / 100.0;
    let tick = bx0 + bw * threshold.clamp(0.0, 100.0) / 100.0;
    let col = bar_color(pct, threshold);
    format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="64" height="64" viewBox="0 0 64 64"><defs><clipPath id="tc"><rect x="2" y="23" width="60" height="18"/></clipPath></defs><rect width="64" height="64" fill="{BG}"/>{badge}{ind}{title}<rect x="{bx0}" y="50" width="{bw}" height="8" fill="{TRACK}"/><rect x="{bx0}" y="50" width="{fill:.1}" height="8" fill="{col}"/><rect x="{tick:.1}" y="48" width="1.6" height="12" fill="{FG}" opacity="0.85"/></svg>"#,
        badge = badge_svg(&state.model_badge),
        ind = indicator_svg(state, elapsed_ms),
        title = title_svg(&state.label, scroll, scroll_secs, speed, font_px, elapsed_ms),
    )
}

/// The label shown on an idle/pre-launch key: the configured directory's basename
/// (or "~" for $HOME / unset).
fn dir_label(settings: &Settings) -> String {
    let d = settings.directory.trim();
    if d.is_empty() || d == home() {
        return "~".to_string();
    }
    std::path::Path::new(d)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(d)
        .to_string()
}

/// Pre-launch key: the Claude sparkle icon + the launch-directory label, square corners.
fn idle_svg(label: &str, spark: &str) -> String {
    let shown = xml_esc(&truncate_chars(label, 9));
    format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="64" height="64" viewBox="0 0 64 64"><rect width="64" height="64" fill="{BG}"/><g transform="translate(32,25)" stroke="{spark}" stroke-width="4" stroke-linecap="round"><line x1="0" y1="-15" x2="0" y2="15"/><line x1="-15" y1="0" x2="15" y2="0"/><line x1="-11" y1="-11" x2="11" y2="11"/><line x1="-11" y1="11" x2="11" y2="-11"/></g><text x="32" y="57" font-family="DejaVu Sans" font-size="13" fill="{FG}" text-anchor="middle">{shown}</text></svg>"#
    )
}

fn render_png(svg: &str) -> Option<Vec<u8>> {
    let mut opt = resvg::usvg::Options::default();
    opt.fontdb = FONTDB.clone();
    let tree = resvg::usvg::Tree::from_str(svg, &opt).ok()?;
    let mut pixmap = resvg::tiny_skia::Pixmap::new(128, 128)?;
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::from_scale(2.0, 2.0),
        &mut pixmap.as_mut(),
    );
    pixmap.encode_png().ok()
}

fn hash_str(s: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}

/// Slow loop: refresh each visible key's state via kdotool + the status file.
async fn slow_poll_loop() {
    loop {
        for instance in visible_instances(ACTION_UUID).await {
            let app_id = app_id_for(&instance);
            match window_raw_title(&app_id).await {
                Some(raw) => {
                    let (busy, attn) = detect_state(&raw);
                    let (pct, model_id) = read_status(&app_id);
                    MISSES.insert(app_id.clone(), 0);
                    KEY_STATE.insert(
                        app_id,
                        KeyState {
                            alive: true,
                            busy,
                            attn,
                            label: clean_label(&raw),
                            pct,
                            model_badge: model_badge(&model_id),
                        },
                    );
                }
                None => {
                    let misses = MISSES.get(&app_id).map(|r| *r).unwrap_or(0).saturating_add(1);
                    MISSES.insert(app_id.clone(), misses);
                    let was_alive = KEY_STATE.get(&app_id).map(|r| r.alive).unwrap_or(false);
                    // Keep showing the live key during a brief miss streak; only blank
                    // once we're confident the session's really gone.
                    if !(was_alive && misses < GRACE_MISSES) {
                        KEY_STATE.insert(app_id, KeyState::default());
                    }
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(SLOW_MS)).await;
    }
}

/// Fast loop: animate + render each visible key, sending an image only on change.
/// Sig 0 is reserved for the "no session -> placeholder" state.
async fn fast_render_loop() {
    loop {
        let elapsed = START.elapsed().as_millis();
        for instance in visible_instances(ACTION_UUID).await {
            let app_id = app_id_for(&instance);
            let settings = SETTINGS.get(&instance.instance_id).map(|r| r.clone()).unwrap_or_default();
            let state = KEY_STATE.get(&app_id).map(|r| r.clone()).unwrap_or_default();

            let svg = if state.alive {
                build_key_svg(
                    &state,
                    threshold_of(&settings),
                    scroll_mode(&settings),
                    scroll_secs_of(&settings),
                    scroll_speed_of(&settings),
                    font_px_of(&settings),
                    elapsed,
                )
            } else {
                idle_svg(&dir_label(&settings), sparkle_color_of(&settings))
            };
            let sig = hash_str(&svg);
            if LAST_SIG.get(&app_id).map(|r| *r) != Some(sig) {
                if let Some(png) = render_png(&svg) {
                    let uri = format!(
                        "data:image/png;base64,{}",
                        base64::engine::general_purpose::STANDARD.encode(&png)
                    );
                    let _ = instance.set_image(Some(uri), None).await;
                    LAST_SIG.insert(app_id, sig);
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(FAST_MS)).await;
    }
}

/// Dev hook: `claudedeck --render-sample` renders a few states to /tmp and exits,
/// so the real resvg pipeline can be eyeballed without the device.
fn render_samples() {
    LazyLock::force(&FONTDB);
    let states = [
        KeyState { alive: true, busy: true, attn: false, label: "fix auth bug".into(), pct: Some(15), model_badge: "O4.8".into() },
        KeyState { alive: true, busy: true, attn: false, label: "refactor the parser module".into(), pct: Some(45), model_badge: "S4.6".into() },
        KeyState { alive: true, busy: false, attn: true, label: "summary written".into(), pct: Some(72), model_badge: "O4.8".into() },
        KeyState { alive: true, busy: false, attn: false, label: "idle".into(), pct: Some(5), model_badge: "H4.5".into() },
    ];
    let mut n = 0;
    for s in states.iter() {
        let svg = build_key_svg(s, 60.0, true, 0.0, SCROLL_PXPS, TITLE_FONT_PX, 300);
        if let Some(png) = render_png(&svg) {
            let _ = std::fs::write(format!("/tmp/rk{n}.png"), png);
        }
        n += 1;
    }
    for label in ["work", "xp3-imc", "~"] {
        if let Some(png) = render_png(&idle_svg(label, ACCENT)) {
            let _ = std::fs::write(format!("/tmp/rk{n}.png"), png);
        }
        n += 1;
    }
    println!("wrote /tmp/rk0..{}.png", n - 1);
}

fn init_logging() {
    // usvg/fontdb log a WARN on every font fallback (e.g. glyphs not in a family);
    // that would flood the plugin log each frame, so silence those targets.
    let config = simplelog::ConfigBuilder::new()
        .add_filter_ignore_str("usvg")
        .add_filter_ignore_str("fontdb")
        .add_filter_ignore_str("resvg")
        .build();
    let _ = simplelog::WriteLogger::init(log::LevelFilter::Info, config, std::io::stderr());
}

#[tokio::main]
async fn main() -> OpenActionResult<()> {
    init_logging();
    if std::env::args().any(|a| a == "--render-sample") {
        render_samples();
        return Ok(());
    }
    log::info!("ClaudeDeck plugin starting");
    LazyLock::force(&FONTDB); // load system fonts once, up front
    register_action(ClaudeSession).await;
    tokio::spawn(slow_poll_loop());
    tokio::spawn(fast_render_loop());
    run(std::env::args().collect()).await
}
