// stax-arrange — M7
//
// Arrangement layer: the temporal/compositional layer above the signal graph.
// Analogous to how a Max patcher sits above Gen.
//
// M7 deliverables:
//   - Transport: bpm, meter, position, tap tempo, Ableton Link
//   - Track: named channel with clip slots and routing
//   - Clip: a stax expression + loop/trigger metadata; triggering instantiates the signal
//   - Pattern: stax Stream driving clip triggers ("x.x.xx.x" or generative)
//   - AutomationLane: a Signal at control rate ("[0 1] 4bars lfsaw" for a cutoff)
//   - Send / Bus: signal routing model
//   - Hot-reload: recompile + crossfade on bar boundary
//   - Views: mixer (05), arrangement (03), session (04) — built in stax-editor using this model
