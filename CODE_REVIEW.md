# ClaudeDeck — Code Review

**Scope:** `src/main.rs`, `scripts/statusline.py`, `install.sh`, `manifest.json`,
`propertyInspector/session.html`, `Cargo.toml`. Review only — no source modified.
**Constraint honoured:** every suggested fix below preserves current behaviour/features
exactly. Anything that would change behaviour is explicitly flagged **out-of-scope**.

## Summary

The plugin is **solid, shipping-quality** code. It's a single ~660-line file with clear
structure, sensible separation between the two background loops, defensive parsing in the
settings accessors, no `unwrap()` on external/fallible data in the hot paths, and no
`DashMap` guard held across an `.await` (the one real deadlock risk for this design — it's
clean: every guard is dropped via `.map(|r| ...).clone()` / `*r` before any await). The
change-detection (hash the SVG, only `set_image` on change) is the right idea and works.

The findings are mostly **efficiency and tidiness**, not correctness. There is **one
genuine resource leak** (stale `KEY_STATE`/`LAST_SIG`/`MISSES` entries are never removed —
High by category, but bounded in practice) and **one meaningful hot-loop inefficiency**
(the fast loop re-formats and re-hashes a full SVG string for every visible key every
200 ms even when nothing changed). Neither is a showstopper; both have clean,
behaviour-preserving fixes. No bug will corrupt output or crash a release build.

---

## Findings (ordered by severity)

| # | Sev | Location | Issue | Why it matters | Behaviour-preserving fix |
|---|-----|----------|-------|----------------|--------------------------|
| 1 | **High** | `main.rs:96–99`, `536–571`, `575–610` | `KEY_STATE`, `LAST_SIG`, `MISSES` are keyed by `app_id` and **never removed**. `will_disappear` (line 119) only removes `SETTINGS` (keyed by `instance_id`). | When a key is removed from the deck or remapped, its `KEY_STATE`/`LAST_SIG`/`MISSES` entries linger forever. Growth is bounded by the number of *distinct grid positions ever used* (so not a runaway leak in practice — `app_id` is deterministic per device+row+col), but it is unbounded across config churn and leaves stale state that can briefly resurface if the same cell is re-added. | In `will_disappear`, also remove the `app_id`-keyed entries. Since `will_disappear` already has `instance`, compute `let app_id = app_id_for(instance);` and `KEY_STATE.remove(&app_id); LAST_SIG.remove(&app_id); MISSES.remove(&app_id);`. This is purely cleanup — visible behaviour is identical because removed keys aren't in `visible_instances` anyway. |
| 2 | **Med** | `main.rs:575–610` (whole `fast_render_loop`) | Every 200 ms, for **every visible key**, the loop clones `Settings` (10 `String`s) + `KeyState`, then **`format!`s a full SVG string and hashes it** — even when nothing changed (the steady-state idle key, or a live key whose pixels are identical frame-to-frame). The expensive `render_png` is correctly skipped on no-change, but the SVG build + hash is not. | This is the dominant per-tick waste. A 6-LCD-key deck does 6 × (2 clones + 1 `format!` of ~700 bytes with ~10 nested `format!` sub-calls in `badge_svg`/`indicator_svg`/`title_svg` + 1 SipHash over ~700 bytes) **5×/sec = 30 SVG builds/sec**, ~99% of which produce a byte-identical string to last tick (idle keys never change; a busy key only changes when the spinner frame or marquee offset ticks). See "Efficiency opportunities" for the cheap fix. | Hash the **render inputs** instead of the SVG output, and only build the SVG when the input-hash changes. The inputs are small and `Hash`-derivable: `state.alive/busy/attn/pct/model_badge/label`, the resolved `threshold/scroll/scroll_secs/speed/font_px`, plus the animation index (`spinner frame index` + quantised `scroll_offset`). Derive `Hash` on `KeyState`, build a tiny tuple sig, compare to `LAST_SIG`; only on miss do `build_key_svg` + `render_png`. Output is byte-identical, so no behaviour change. (Out-of-scope nuance: if you hash raw `elapsed_ms` you'd never get a hit — you must hash the *animation frame indices*, not the raw clock. See notes.) |
| 3 | **Med** | `main.rs:578` & `538` | Both loops call `visible_instances(ACTION_UUID).await` and recompute `app_id_for(&instance)` (allocates + char-maps a `String`) every iteration. The fast loop does this **5×/sec**. | Minor repeated allocation, but it's in the hottest loop and the value is stable per instance for the session's life. | Low-value but free: nothing required if Finding 2 is done (the per-key work shrinks anyway). Not worth a dedicated cache. Noting for completeness; effectively a Nit. |
| 4 | **Med** | `main.rs:558–559` | `let misses = MISSES.get(...).unwrap_or(0) + 1;` where the stored type is `u8`. A key whose window is closed but which stays on the deck accumulates one miss per 1.5 s forever. At 256 consecutive misses (~6.4 min) this is `255u8 + 1`. | **Release builds** (what `install.sh` ships, `--release`) **wrap to 0** silently — harmless here because `was_alive` is already `false` long before, so the grace branch is moot. But a **debug build panics** on overflow. It's a latent foot-gun. | Saturate: `.unwrap_or(0).saturating_add(1)`. Behaviour identical for all values that matter (anything ≥ `GRACE_MISSES` already takes the same branch); just removes the debug-panic / wrap. |
| 5 | **Low** | `main.rs:373–378` (`bar_color`) + `483` (`tick`) | The bar **fill colour** is quantised into 10% *value* bands (`(pct/10).floor()*10`) and mapped through `t = band/threshold`. The **tick** is drawn at the true `threshold` position. So the fill reaches "red" (ramp `t=1.0`) at the band boundary at/under threshold, not exactly at the tick. | This is an intentional aesthetic ("quantise to 10% bands" per the comment) and matches the README ("ramps in 10% bands … reaching red at threshold"). **Not a bug** — flagged only so a future reader doesn't "fix" the band/tick mismatch and change the look. **Out-of-scope to change.** | None. Behaviour is as designed; leave as-is. |
| 6 | **Low** | `main.rs:216–218` (`clean_label`) | `trim_start_matches(|c| !c.is_alphanumeric())` uses Unicode `is_alphanumeric`, so a title beginning with a non-ASCII letter/digit (e.g. an accented or CJK first char) is kept, while the leading status glyph (braille `⠿` / `✳`, both non-alphanumeric) is correctly stripped. Combined with the later `.trim()`, this is fine. | Edge case: a title that is *only* punctuation/emoji becomes empty → key renders with no label (acceptable, matches `title_svg` empty-string guard at line 414). No defect, but worth knowing the label can legitimately be empty. | None needed. Behaviour is reasonable. |
| 7 | **Low** | `main.rs:250–258` (`model_badge`) | Version parsing splits the lowercased id on non-digits, keeps runs of ≤2 digits, takes the **first two** runs as `a.b`. For `claude-opus-4-8` → `["4","8"]` → `O4.8` (correct). But e.g. `claude-3-5-sonnet-20241022` → digit-runs `["3","5"]` then `"20241022"` (len 8, dropped) → `S3.5` (correct), yet a hypothetical id like `claude-opus-4` → `["4"]` → `O4` (correct). A 3-digit middle token (`claude-x-100`) is silently dropped. | These are all reasonable for the current Claude id scheme; the ≤2-digit filter is what makes it robust against date suffixes. No real-world id today breaks it. **Not a bug.** | None. The filter is deliberately conservative; leave as-is. |
| 8 | **Low** | `main.rs:131–137` (`key_down`) | `key_down` calls `window_raw_title` (one `kdotool` spawn) to decide focus-vs-launch. This races the slow loop's identical call but they don't share state, so no correctness issue — just an extra subprocess on each press. | Negligible (only on user press), but it duplicates the liveness signal the slow loop already maintains in `KEY_STATE.alive`. | Out-of-scope to change (using cached `alive` would alter timing/behaviour on first press before the slow loop has run). Leave the direct probe — it's the correct, race-free choice. Noting only. |
| 9 | **Nit** | `main.rs:469` | `#[allow(clippy::too_many_arguments)]` on `build_key_svg` (7 args). | Style only. | Could bundle the 5 resolved settings into a small `RenderCfg` struct, but that's churn for no behaviour gain. Leave it. |
| 10 | **Nit** | `main.rs:614–637` (`render_samples`) | The `--render-sample` dev hook (writes `/tmp/rk*.png`). | **Intentional** dev tooling, documented in README "Dev tips". Not dead code. | None — keep. (Flagged per task instructions.) |
| 11 | **Nit** | `main.rs:380–386` (`xml_esc`) | Five chained `String::replace` calls allocate a new `String` each (5 allocations per escaped fragment). | Called a few times per SVG; cheap relative to `render_png`. Only matters if it's on a no-change tick (Finding 2 removes those). | Leave as-is; not worth a single-pass escaper. |
| 12 | **Nit** | `main.rs:1` & `Cargo.toml:14` | `use base64::Engine;` import is needed for the `.encode` trait method at line 601 (used). All imports are live; no dead `use`. `resvg` pulls `tiny_skia`/`usvg` re-exports — all used. | — | No action; confirming there is **no dead import/fn/const**. `lerp`, `sample_ramp`, `bar_color`, every `*_of` accessor, and all consts are referenced. |
| 13 | **Nit** | `statusline.py:38` | `line = f"{pct}% ctx"` — if `pct` is `0` the `is not None` guard correctly keeps it (prints `0% ctx`); if `pct` is a float it prints e.g. `12.5% ctx`. The plugin side (`read_status`, line 228–230) handles both int and float `pct`. | Consistent. No mismatch between writer and reader. | None. |

---

## Efficiency opportunities (quantified)

**Hot-loop waste (the headline).** `fast_render_loop` runs at `FAST_MS = 200` → 5 Hz. Per
tick, per visible key it unconditionally:

1. clones `Settings` (10 heap `String`s) — `main.rs:580`
2. clones `KeyState` (1 `String` label + 1 `String` badge) — `main.rs:581`
3. resolves 5 settings via `*_of` parsers (each `trim().parse()`)
4. **`format!`s the full key SVG** (~700 bytes, via `build_key_svg` which itself calls
   `badge_svg`, `indicator_svg`, `title_svg`, `bar_color`, each with their own `format!`s and
   `xml_esc` allocations) — `main.rs:485`
5. **hashes the SVG string** with SipHash (`DefaultHasher`) over ~700 bytes — `main.rs:596`

Only step 6 (`render_png`, the genuinely expensive resvg rasterise + PNG encode) is gated on
the change check. So on a **6-key deck**, steps 1–5 run **30×/second**, and the vast majority
are wasted: an **idle key never changes**, and a **live key only changes** when (a) the spinner
frame index advances (every `SPINNER_FRAME_MS = 120 ms`, so ~1.7 of every 5 ticks for a busy
key), (b) the marquee offset moves enough to change a `:.1`-rounded coordinate, or (c) state
flips. Steady-state, **>90% of the SVG builds + hashes produce a byte-identical result** and are
pure waste.

**Highest-value refactor (behaviour-preserving):** hash the *render inputs*, not the *render
output*. Replace step 4–5's "build SVG then hash it" with "build a tiny input signature, hash
that; only build+rasterise the SVG on a sig miss." Concretely:

- `#[derive(Hash)]` on `KeyState` (it's already `Clone, Default`; all fields are `Hash`-able).
- Compute the animation-frame *indices* (not raw `elapsed_ms`): the spinner index
  `((elapsed/SPINNER_FRAME_MS) % len)` and the quantised marquee offset (e.g.
  `(scroll_offset(...) * 10.0) as i64`, matching the `:.1` rounding the SVG already uses).
- Hash `(alive, &state, threshold_bits, scroll, scroll_secs_bits, speed_bits, font_px_bits,
  spinner_idx, marquee_q)` (floats via `to_bits()`), compare to `LAST_SIG`.
- On a **hit**, skip everything (no clone of `Settings` SVG, no `format!`, no rasterise).
- On a **miss**, do exactly what the code does today: `build_key_svg` / `idle_svg` → `render_png`
  → `set_image`.

Because the SVG is a *pure function* of those inputs, the emitted PNG is **identical** to today's
— this is a strict speed-up with zero visible change. It collapses steps 4–5 from "always" to
"only when the image actually changes," eliminating the ~90% wasted SVG builds/hashes. (You'd
still clone `Settings`/`KeyState` to *read* the inputs, or read fields under the guard and drop it
before the await — either is fine and far cheaper than today.)

**Secondary, smaller wins (optional, all behaviour-preserving):**

- `render_png` (`main.rs:515–526`) rebuilds `usvg::Options::default()` each call and clones the
  `Arc<fontdb::Database>` — the clone is just an atomic bump (cheap, fine). `Options::default()`
  is also cheap. No change needed, but note `Pixmap::new(128,128)` allocates 64 KB/render — that's
  inherent and only happens on real changes once Finding 2 lands.
- `kdotool` spawn frequency: the **slow** loop spawns one `kdotool search … getwindowname` per
  visible key per 1.5 s (`main.rs:540`). That's the intended cadence and is "cheap on KWin" per the
  comment — leave it. The only redundant spawn is `key_down`'s probe (Finding 8), which is correct
  to keep.

---

## Things that are *correct* and worth not touching

- **No `DashMap` guard is held across `.await`.** Every access pattern is
  `MAP.get(k).map(|r| r.clone())` / `*r` (lines 558, 560, 580, 581, 597), dropping the guard
  before any await. This is the single biggest deadlock risk for a two-loop `DashMap` design and
  it's handled correctly throughout.
- **The two loops sharing the maps is race-safe** for this use: the slow loop is the sole writer
  of `KEY_STATE`/`MISSES`; the fast loop is the sole writer of `LAST_SIG`; reads tolerate stale
  values by one tick (acceptable — the next fast tick reconciles). No torn state.
- **`GRACE_MISSES` debounce logic is correct:** `!(was_alive && misses < GRACE_MISSES)` keeps a
  live key visible for misses 1 and 2, blanks on miss 3+. Matches the documented intent.
- **Launch-command construction is correct** for both terminals: `--class` first, then per-terminal
  font `-o` overrides, then (kitty) bare `claude …` vs (alacritty) `-e claude …`. `claude_args` is
  `split_whitespace`'d (documented limitation in STATUS.md — no shell quoting; fine for flags).
  `--settings` is appended *after* the user args so it can't be clobbered. `current_dir` + the
  `CLAUDEDECK_KEY` env are set. Spawned child is reaped via a detached `wait()` task (line 346) so
  it won't zombie. Good.
- **`detect_state` glyph ranges** (`U+2800–28FF` braille = busy, `U+2733 ✳` = attn) match the
  README/STATUS contract.
- **`scroll_offset` math** is correct: continuous mode `(secs*speed) % period`; periodic mode
  scrolls once over `period/speed` then rests at 0 until `cycle` elapses, with a sensible
  `cycle = scroll_secs.max(scroll_dur + 0.5)` floor so a too-small interval can't cut a scroll off.
- **`install.sh`** correctly avoids "Text file busy" via temp-file + `mv` (atomic rename), uses
  `set -euo pipefail`, and writes the scoped settings via heredoc. The `cp "$BUNDLE/assets/"*`
  glob assumes assets exist (they do per the bundle) — fine.
- **`statusline.py`** is defensive (`json.load` wrapped, `or {}` fallbacks, `mkdir(parents,
  exist_ok)`) and the `pct`/`model` reader/writer contract matches `read_status`.

## Bottom line

Ship it. The two worth doing before/after a release:
**(1)** clean up the `app_id`-keyed maps in `will_disappear` (Finding 1 — trivial, prevents stale
state), and **(2)** the input-signature refactor of `fast_render_loop` (Finding 2 — the one
high-value perf win, strictly behaviour-preserving). Finding 4 (`saturating_add`) is a one-word
safety tidy. Everything else is Nit/intentional.
