# Parser fuzzing

| Target | Input |
| --- | --- |
| `json_prune` | A JSON body, or a JSONPath expression on the first line followed by a body |
| `html_filter` | An HTML or XHTML body |
| `playlist` | An HLS playlist |
| `set_cookie` | A Set-Cookie header value |
| `rule_list` | A filter list, compiled with user-rule permissions |
| `clienthello` | A TLS handshake message, starting with the handshake type and length |
| `dns` | A DNS message, including its header |
| `quic` | A UDP payload checked for a QUIC client Initial |
| `cosmetic` | CSP header values, a CSS selector and an HTML body |
| `datagram` | A raw IPv4 or IPv6 packet carrying UDP |
| `abx` | Android Binary XML for user or package metadata |

For `cosmetic`, the first three lines are the enforced CSP value, the
report-only CSP value and the selector. The rest is the HTML body, which is
also scanned in chunks. Leave a header line empty to omit that header. An
input with fewer than three line breaks is tried as each field.

Plain `cargo test` replays the committed files under `fuzz/seeds/` for
every target. CI runs these tests on stable Rust without cargo-fuzz.

Install nightly Rust and `cargo-fuzz` with `rustup toolchain install nightly` and
`cargo install --locked cargo-fuzz`. From the repository root, run a target by hand:

```sh
mkdir -p fuzz/corpus/json_prune
cargo +nightly fuzz run json_prune fuzz/corpus/json_prune fuzz/seeds/json_prune -- \
  -max_total_time=300 -timeout=5 -max_len=65536
```

Use `cargo +nightly fuzz list` to list the targets.

A panic in a parser would abort the release daemon. libFuzzer saves the input
under `fuzz/artifacts/<target>/` so it can be replayed by passing that file to
`cargo +nightly fuzz run <target>`.
