# T004 review, round 2: corrections

Reviewer: Claude. Commit 21debb7 passes the checks, follows all ten plan corrections, and the vault,
store and error work is accepted as it is. One real defect and one small gap remain; fix only these.

## 1. `key_hint` reveals the whole key when it is short (must fix)

`key_hint("1234")` returns `…1234` and `key_hint("12")` returns `…12`, and `test_key_hint_formatting`
locks that in. The hint is the only thing the API may ever show about a secret, so it must never show
all of it, or even half of it. A real Binance key is 64 characters, but this helper will also be handed
FYERS app ids, passphrases and whatever else a later task puts through it.

Change it to: show the last 4 characters only when the value has **at least 8 characters**; for anything
shorter (including empty) return just `…`. Keep counting `char`s, not bytes.

Update the test to assert: a 12-character key gives `…` + its last 4; exactly 8 characters gives the
last 4; 7 characters, 4 characters, 2 characters and the empty string all give `…` alone; the emoji case
still works (use a value of at least 8 characters). Do not change the signature of `key_hint` or
`key_hint_secret`.

## 2. Say what the ignored test's 1281 step really proves (small)

In `test_os_keychain_real_store_lifecycle_and_boundaries`, step 6 (1281 characters) is rejected by our
own `check_secret_size` before `keyring` is ever called, so it does not exercise the real store's limit;
step 5 (1280 characters accepted by the real store) is what proves our limit is not set too high. Add a
one-line comment at step 6 saying exactly that. No code change.

## Do not

- Do not add dependencies or change `Cargo.toml` / `Cargo.lock`.
- Do not change anything else in `vault.rs`, and do not touch other files.
- Do not run the `#[ignore]` test (the owner runs it by hand).
- Do not amend commit 21debb7; make a new commit.

## Done when

`cd src-tauri && cargo test --no-default-features` (0 warnings, 1 ignored), `python -m pytest -q` and
`node tools/live_check.mjs` pass, and the new commit message starts with `T004:`.
