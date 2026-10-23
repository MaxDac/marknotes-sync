# Copilot instructions

Follow [AGENTS.md](../AGENTS.md). In short: this repository is the shared sync core of the three MarkNotes apps
(Rust, exposed through UniFFI to Kotlin and through WebAssembly to TypeScript). Behaviour changes land in
[marknotes-spec](https://github.com/MaxDac/marknotes-spec) first, then here (bump `spec/`), and every fixture under
`spec/fixtures/` that this crate covers must pass. Keep the Rust, Kotlin and npm APIs in step, and don't put I/O in
the core: the apps run the actions it plans.
