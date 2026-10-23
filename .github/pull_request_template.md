## What

<!-- What changes and why. -->

## Spec

<!-- Link the marknotes-spec PR this implements, or say why the change doesn't affect behaviour. -->

## API impact

<!-- New or changed functions/types in Rust, Kotlin (UniFFI) and npm (index.d.ts). Do the apps need changes? -->

## Checks

- [ ] `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- [ ] `cargo test --workspace` (spec fixtures included)
- [ ] `bindings/npm`: `npm run build && npm test`
- [ ] `bindings/kotlin`: `scripts/kotlin-bindings.sh && ./gradlew test`
