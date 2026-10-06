# VdMot Revamped STM32 in Rust: port notes

C++ behaviour of `software_stm32/lib/core` that the Rust port (`software_stm32_rust/core`) keeps
although it looks odd, and the places where the C++ has no defined behaviour to keep. Each entry:
module, what, why it matters. The Rust tests assert the C++ behaviour.

| module | what | why it matters |
|---|---|---|
| presence_test | `sample()` takes the magnitude with `v < 0 ? -v : v` on an `int32_t`: undefined for `INT32_MIN` (in practice it wraps to a negative value and counts as no current). The Rust computes it in `i64`, so `i32::MIN` counts as a large current. | None in practice: the glue feeds the filtered current of the end-stop detector, which never leaves ±100000. |
| move_classifier | `positionAfterEndStop()` is documented as "clamped to 0..100", but a close from a start above 100 returns `start - delta` unclamped (start 150, delta 10 -> 140); only an open is capped at 100. The Rust does the same; test_move_classifier__mut.cpp checks it. | None in practice: the start is the believed position, always 0..100. A caller that passes an unchecked start gets a position above 100. |
