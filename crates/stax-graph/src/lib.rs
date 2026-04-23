// stax-graph — M4
//
// Graph IR and bidirectional Op <-> graph round-trip.
// The graph is a *view* over the Op stream — never a separate artifact.
//
// M4 deliverables:
//   - NodeId, PortId, Edge, typed ports (ValueKind drives port color)
//   - Op stream -> graph: lift flat Op stream to node DAG
//   - Graph -> Op stream: lower DAG back to flat Op stream
//   - Headless round-trip tests: parse -> ops -> graph -> ops -> eval -> bit-identical
