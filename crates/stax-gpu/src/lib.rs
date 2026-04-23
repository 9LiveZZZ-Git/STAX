// stax-gpu — M9
//
// Explicit GPU operators via wgpu (Vulkan / Metal / DX12 / WebGPU).
// Not transparent offload — explicit words for workloads where GPU wins.
//
// M9 deliverables (WGSL compute shaders):
//   - additive-gpu N: additive synthesis with hundreds/thousands of partials
//   - granular-gpu: grain clouds (thousands of simultaneous grains)
//   - fdtd-plate / fdtd-membrane: 2D/3D physical models
//   - conv-gpu: partitioned convolution reverb
//   - wavetable-gpu: dense wavetable bank interpolation
//
// CPU<->GPU round-trip adds 2-4 blocks latency; suitable for synthesis, not per-sample feedback.
// Feature-flagged so it compiles out on platforms without wgpu support.
