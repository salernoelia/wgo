Vendored from crates.io mlx-sys 0.2.0 (MIT OR Apache-2.0).

- build.rs enables MLX_METAL_JIT to embed general Metal kernel sources.
- embed-metallib.cmake patches MLX 0.25.1's default library loader to use the
  remaining precompiled attention/normalization kernels from memory.
- src/lib.rs embeds that metallib in the executable and exposes its bytes to C++.

No runtime files, child processes or non-system dynamic libraries are required.
The patch checks the pinned MLX loader before changing it and is idempotent.
