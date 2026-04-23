// stax-jit — M10
//
// cranelift JIT for pure Signal+Real subgraphs.
// Any stax expression touching only Signal + Real (no streams, forms, refs)
// is a pure DSP kernel and can be compiled to native via cranelift.
//
// M10 deliverables:
//   - Eligibility analysis: walk Op stream, extract signal-typed dataflow subgraphs
//   - cranelift IR lowering: Op subgraph -> CL IR -> native machine code
//   - Hot-swap: interpreter stays default; JIT kicks in on stable play'd graphs
//   - Same semantics as interpreter; order-of-magnitude speedup on complex patches
