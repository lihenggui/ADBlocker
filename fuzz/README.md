# Parser fuzzing

Install nightly Rust and `cargo-fuzz` with `rustup toolchain install nightly` and
`cargo install --locked cargo-fuzz`. From the repository root, run a target by hand:

```sh
mkdir -p fuzz/corpus/json_prune
cargo +nightly fuzz run json_prune fuzz/corpus/json_prune fuzz/seeds/json_prune -- \
  -max_total_time=300 -timeout=5 -max_len=65536
```

Use `cargo +nightly fuzz list` to find the other targets. JSON and XML inputs can
be raw bodies or a path expression on the first line followed by a body.
`rule_list` builds the complete matcher and projection tables for a list.

A panic in a parser would abort the release daemon. libFuzzer saves the input
under `fuzz/artifacts/<target>/` so it can be replayed by passing that file to
`cargo +nightly fuzz run <target>`.
