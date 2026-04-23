// stax-plugin — M8
//
// VST3/CLAP plugin target via nih-plug.
// Exposes a compiled stax patch as a loadable plugin (Ableton, Bitwig, Reaper, FL).
//
// M8 deliverables:
//   - StaxPlugin: nih-plug ProcessPlugin wrapping stax-eval + stax-audio
//   - Parameter mapping: Ref bindings in the patch become DAW-visible parameters
//   - Preset serialization: save/load patch.stax as plugin preset
//   - Note: plugin *host* is post-M10; this is the plugin *target*
