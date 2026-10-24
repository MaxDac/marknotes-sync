//! Replays every fuzz check (`cargo test --lib` in `fuzz/`) on the spec fixtures (whole files, and the documents and
//! strings inside them), the committed `regressions/<target>/` inputs, the local corpus and crash files if present,
//! and seeded pseudo-random mutations of all of them. Fast enough for CI; the libFuzzer runs are nightly.
use std::path::{Path, PathBuf};

use crate::TARGETS;

fn files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            files(&p, out);
        } else {
            out.push(p);
        }
    }
}

/// Every JSON object and string inside a fixture, so that `plan`'s and `reconcile`'s inputs and the texts reach the
/// checks as whole documents; objects with `local` and `remote` texts also become merge inputs.
fn pieces(v: &serde_json::Value, out: &mut Vec<Vec<u8>>) {
    match v {
        serde_json::Value::Object(map) => {
            out.push(serde_json::to_vec(v).unwrap());
            if let (Some(l), Some(r)) = (
                map.get("local").and_then(|x| x.as_str()),
                map.get("remote").and_then(|x| x.as_str()),
            ) {
                let joined = match map.get("base").and_then(|b| b.as_str()) {
                    Some(b) => format!("{b}\0{l}\0{r}"),
                    None => format!("{l}\0{r}"),
                };
                out.push(joined.into_bytes());
            }
            map.values().for_each(|x| pieces(x, out));
        }
        serde_json::Value::Array(xs) => xs.iter().for_each(|x| pieces(x, out)),
        serde_json::Value::String(s) => out.push(s.clone().into_bytes()),
        _ => {}
    }
}

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

/// Tokens that matter to the core: line endings, NUL, front-matter fences, path separators, characters that case-fold
/// specially (long s, final sigma), a BOM, JSON syntax and numbers at the edges of the safe range.
const TOKENS: &[&[u8]] = &[
    b"\n",
    b"\r\n",
    b"\r",
    b"\0",
    b"---\n",
    b"id: ",
    b"/",
    b".md",
    b" ",
    b"a",
    b"A",
    "\u{17f}".as_bytes(),
    "\u{3c2}".as_bytes(),
    "\u{feff}".as_bytes(),
    b"\"",
    b"{",
    b"}",
    b"[",
    b"]",
    b",",
    b":",
    b"0",
    b"-1",
    b"9007199254740993",
    b"1e400",
    b"null",
    b"\\u0000",
];

fn mutate(rng: &mut Rng, input: &[u8]) -> Vec<u8> {
    let mut v = input.to_vec();
    for _ in 0..1 + rng.below(4) {
        let at = rng.below(v.len() + 1);
        match rng.below(3) {
            0 => {
                let token = TOKENS[rng.below(TOKENS.len())];
                v.splice(at..at, token.iter().copied());
            }
            1 if at < v.len() => {
                let end = (at + 1 + rng.below(8)).min(v.len());
                v.drain(at..end);
            }
            _ if !v.is_empty() => {
                let from = rng.below(v.len());
                let to = at.min(v.len() - 1);
                v[to] = v[from];
            }
            _ => {}
        }
    }
    v
}

#[test]
fn replay() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let spec =
        std::env::var_os("MARKNOTES_SPEC_DIR").map_or_else(|| root.join("../spec"), PathBuf::from);
    let mut fixtures = Vec::new();
    files(&spec.join("fixtures"), &mut fixtures);
    assert!(
        !fixtures.is_empty(),
        "no spec fixtures under {}; run `git submodule update --init` or set MARKNOTES_SPEC_DIR",
        spec.display()
    );
    let mut seeds: Vec<Vec<u8>> = Vec::new();
    for f in &fixtures {
        let bytes = std::fs::read(f).unwrap();
        if let Ok(v) = serde_json::from_slice::<serde_json::Value>(&bytes) {
            pieces(&v, &mut seeds);
        }
        seeds.push(bytes);
    }
    let rounds: usize = std::env::var("MARKNOTES_FUZZ_REPLAY_ROUNDS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(4);
    for (name, check) in TARGETS {
        let mut inputs = seeds.clone();
        for dir in ["regressions", "corpus", "artifacts"] {
            let mut saved = Vec::new();
            files(&root.join(dir).join(name), &mut saved);
            inputs.extend(saved.iter().map(|p| std::fs::read(p).unwrap()));
        }
        let mut rng = Rng(0x4655_5A5A ^ ((name.len() as u64) << 32));
        let originals = inputs.len();
        for i in 0..originals * rounds {
            let m = mutate(&mut rng, &inputs[i % originals]);
            inputs.push(m);
        }
        for input in &inputs {
            if std::panic::catch_unwind(|| check(input)).is_err() {
                panic!(
                    "fuzz check {name} failed on {:?}",
                    String::from_utf8_lossy(input)
                );
            }
        }
    }
}
