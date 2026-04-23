# stax — visual design language

Five HTML mockups establishing the visual vocabulary for the editor. Each one is
self-contained: open in a browser, no build step. Best viewed at ≥ 1300 px wide.

```
01-editor.html         the main node-graph editor (M5)
02-fnport.html         function-valued port detail — drill-in, named binding, nesting
03-arrangement.html    arrangement view — linear timeline, M7
04-session.html        session view — clip-launcher grid, M7
05-mixer.html          mixer view — channel strips with stax inserts/sends
06-text.html           text/IDE view — the canonical source; all other views are projections
```

Design direction is **refined brutalism**: the functional honesty of Pure Data
with the visual care of a well-drafted technical document, instead of the
appearance of a 90s X11 application. The point is intentionality, not intensity.

## Tenets

1. **Color carries meaning, not decoration.** The four port-type hues
   (real / signal / stream / fun / form) appear nowhere else. Track colors
   are a separate, deliberately-muted family. One accent (`--warm`,
   terracotta) marks active / playing / selected — and only those.

2. **No skeuomorphism.** No rounded corners, no shadows, no gradients,
   no glass, no faux-3D knobs. A knob is a line on a circle; a fader is a
   black bar in a frame; a meter is a colored fill in another frame.

3. **Single typeface.** JetBrains Mono throughout, weight and tracking
   carry the typographic hierarchy. No display font, no fallback to Inter.

4. **Information density without noise.** Dotted hairlines (`--rule-2`) for
   sub-divisions, solid for primary structure, 1.5–2 px ink for top-level
   boundaries (master strip, stop-all). Density is fine; competing for
   attention isn't.

5. **PD honesty.** Empty things look empty (dotted ghost outlines).
   The expression that drives a clip / lane / insert is shown literally.
   Documentation lives in the artifact itself.

## Tokens

```
--paper        #f4f1ea     warm off-white background
--paper-2      #efebe1     subtle surface tint
--surface      #ebe7dd     darker panel surface (master, returns)
--rule         #d4cfc3     primary horizontal/vertical divider
--rule-2       #bcb5a4     secondary, often dotted

--ink          #1a1a1a     primary text & strong borders
--ink-2        #6b6558     secondary text & labels
--ink-3        #9a9383     tertiary text & disabled

--warm         #c94820     terracotta — active / playing / selected
--cool         #2d5a4a     deep teal — meters / Ableton Link

--queued       #d4a017     muted gold — queued state (session view)

port colors
--port-real    #1a1a1a     scalars
--port-signal  #c94820     audio-rate signals
--port-stream  #2d5a4a     lazy value sequences
--port-fun     #6b4e8a     functions / higher-order
--port-form    #8a6b2a     dictionaries

track colors (muted, color-of-the-thing-not-of-the-track)
--t-kick       #8a4a4a     low / impact
--t-bass       #2d5a4a     low / sustained
--t-pad        #6b4e8a     atmospheric
--t-lead       #c94820     focal / cutting
--t-fx         #8a6b2a     auxiliary
--t-rev/--t-del  #4a6b8a   muted blue — return channels
```

## Conventions across views

- **Top bar is identical** in all four DAW-style views. Same brand, menu,
  audio status, and transport block. Switching tabs never changes the chrome.
- **Tabs reserve future views** even when not implemented yet (the editor
  view shows `arrangement`, `session`, `mixer` in dotted-border "alt" style;
  the M7 views show them in active style).
- **Inspector is on the right** at 240 px. Same header treatment
  (`<h3>` 10/700/0.18em uppercase ink-3 with dotted underline). Always shows
  the selected thing, never modal.
- **Bottom bar** runs 38 px tall, holds level meters, mode indicators
  (`hot-reload · bar boundary` / `pre/post` / quantization), and contextual
  status messages (MIDI routing, total latency).
- **The same expression is shown identically** in every view that displays
  it — same operator color (`var(--port-signal)`), same `\` / `,` highlighting
  in violet for higher-order constructs, same monospace.

## What this isn't

Not a UI kit, not a component library, not yet a real implementation.
Each file is a single static HTML document — the goal was to commit to a
visual vocabulary clearly enough that the M5 editor work has something
specific to converge on.

The reference implementation in `crates/stax-editor/` (M5) will read these
mockups as the spec, port the tokens to a Rust constants module, and
recreate the layouts in `egui` immediate-mode primitives.
