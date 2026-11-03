# T001 review, round 2: corrections

Reviewer: Claude. The implementation in commit 1c9f872 passes all checks and the migration, money and
file-writer work is accepted as it is. One real defect and two small gaps remain; fix only these.

## 1. `is_secret_field` misses common secret names (must fix)

`src-tauri/src/logging.rs` lowercases the name and tests `contains("api_key")` etc. That misses
`apiKey`, `APIKEY`, `X-MBX-APIKEY` (lowercased: `apikey`, `x-mbx-apikey`), `private_key`/`privateKey`,
`passphrase`, `cookie`, `bearer`, `credential`, `pin`. T007 will log Binance requests and Binance's header
is literally `X-MBX-APIKEY`, so this has to be right before then.

Change it to normalise first, then match:

- normalise: lowercase, then drop every `_`, `-`, `.`, space.
- substring match on the normalised name: `apikey`, `accesskey`, `secret`, `token`, `password`, `passwd`,
  `passphrase`, `authorization`, `signature`, `privatekey`, `credential`, `cookie`, `bearer`, `hsmkey`,
  `mnemonic`, `seedphrase`.
- exact match on the normalised name (substring would over-match): `pin`, `key`, `auth`, `pem`, `otp`, `totp`, `cvv`.

Failing closed is correct: a harmless field that happens to contain `token` being masked is acceptable.
Keep the function `pub` and keep its signature.

## 2. Tests for it (must add)

In `logging.rs` tests:

- a table test over `is_secret_field` that asserts TRUE for: `api_key`, `apiKey`, `APIKEY`, `X-MBX-APIKEY`,
  `private_key`, `privateKey`, `passphrase`, `pin`, `key`, `secret_key`, `client_secret`, `access_token`,
  `refreshToken`, `Authorization`, `signature`, `cookie`, `bearer`, `hsm_key`, `credential`, `password`;
  and FALSE for: `user`, `port`, `symbol`, `price`, `qty`, `order_id`, `author`, `spinning`, `monkey`, `side`.
- one end-to-end test through `RedactingFormatter` that logs `apiKey = "leak-me-123"` (camelCase tracing
  field) and `X_MBX_APIKEY = "leak-me-456"` and asserts neither value appears in the output while the field
  names are present with `[REDACTED]`.

## 3. Env var mutation in a parallel test (small, fix)

`tests/migration.rs::data_dir_resolution_does_not_create_directory` sets and removes `ZONEBOARD_DATA`.
Cargo runs tests in one binary on parallel threads, and T003 (settings) will add tests that read
`data_dir()`. Guard it with a `static ENV_LOCK: Mutex<()>` in that file, held for the whole test, and add a
one-line comment saying any test touching `ZONEBOARD_DATA` must take the lock. Do not change production code
for this.

## Do not

- Do not add dependencies.
- Do not change migrations, `Money`, `Journal`, or the file writer.
- Do not amend commit 1c9f872.

## Done when

`cd src-tauri && cargo test --no-default-features` (0 warnings), `python -m pytest -q` and
`node tools/live_check.mjs` all pass, and the new commit message starts with `T001:`.
