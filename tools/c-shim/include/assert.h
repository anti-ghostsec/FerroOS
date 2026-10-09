/* Minimal headers so freestanding crypto C code (ring) can be compiled for
   x86_64-linux-musl with plain clang; symbols come from Rust's bundled musl. */
#pragma once
#define assert(x) ((void)0)
