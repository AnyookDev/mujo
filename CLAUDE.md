## Loop

- `cargo check` for compile verification. Never `cargo build` in the agent loop —
  it runs codegen and links for no benefit.
- `cargo test` only when behaviour changed.
- If a step produces no output for more than a minute, check `ps` for a live cargo
  before assuming it's working.
