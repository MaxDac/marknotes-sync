Inputs that once failed a fuzz target, one folder per target (`regressions/<target>/<name>`). `cargo test --lib`
in `fuzz/` replays them in CI, so a fixed crash stays fixed. See TESTING.md, "Reproducing a nightly failure".
