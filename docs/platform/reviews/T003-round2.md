# T003 review, round 2: corrections

Reviewer: Claude. Commit b9b934e passes the checks and follows the plan corrections for the migration,
the three triggers, the transactional settings write, `with_conn`, JSON errors and `next_before`. Accepted
as it is. One real defect, which is partly my own spec's fault, and two test gaps remain; fix only these.

## 1. The timezone list rejects current IANA names (must fix)

`Intl.supportedValuesOf('timeZone')` in Node returns ICU's own names, which are often the **old** spellings.
The generated `timezones.rs` therefore contains `Europe/Kiev`, `Asia/Katmandu`, `Asia/Saigon`,
`Asia/Rangoon`, `America/Buenos_Aires`, `America/Godthab`, `Atlantic/Faeroe`, `Pacific/Truk`, `Pacific/Ponape`
and rejects their current IANA names (`Europe/Kyiv`, `Asia/Kathmandu`, `Asia/Ho_Chi_Minh`, `Asia/Yangon`,
`America/Argentina/Buenos_Aires`, `America/Nuuk`, `Atlantic/Faroe`, `Pacific/Chuuk`, `Pacific/Pohnpei`). The
one-off `Asia/Calcutta` -> `Asia/Kolkata` patch in the generator hides the symptom for a single zone.

My spec also said to reject aliases such as `Asia/Calcutta`. That was wrong: browsers report exactly those
ICU names (`Intl.DateTimeFormat().resolvedOptions().timeZone` is `Asia/Calcutta` for users in India), and the
settings UI will send what the browser reports. The rule is now: **accept the IANA names and the names the
browser reports**.

New rule: a name is valid if it is in (a) IANA `zone.tab` or (b) `Intl.supportedValuesOf('timeZone')` of the
generating Node, or is `UTC`.

- Vendor the IANA file at `tools/data/zone.tab` (public domain, 18 KB). Copy it from
  `C:\Users\NaveenDhanasekaran\AppData\Local\Programs\Python\Python313\Lib\site-packages\tzdata\zoneinfo\zone.tab`
  (tzdata 2026.2). Do not download anything.
- `tools/gen_timezones.mjs`: read that file (skip `#` lines and blanks, name is the 3rd tab-separated column),
  union with the Node list and `UTC`, **remove the `Asia/Calcutta` special case**, sort by code point, de-duplicate.
  Before writing, assert that every name is accepted by `new Intl.DateTimeFormat('en', { timeZone: name })`
  and fail generation with the offending names if not. The generated header must say it is generated, give
  the command, the Node version and the tzdata version, and say "IANA zone.tab names plus the names browsers
  report (ICU)". Remove every comment or doc line that says aliases are rejected.
- Regenerate and commit `timezones.rs`.
- Tests: accepted: `UTC`, `Asia/Kolkata`, `Asia/Calcutta`, `Europe/Kyiv`, `Europe/Kiev`, `Asia/Kathmandu`,
  `Asia/Katmandu`, `America/Argentina/Buenos_Aires`, `America/Buenos_Aires`, `America/New_York`,
  `Europe/London`. Rejected: `Mars/Olympus`, `EST`, `US/Eastern`, the empty string, `asia/kolkata`. Keep the
  sorted-and-unique test. Update the API test comment accordingly; `asia/kolkata` must still be a 400.

## 2. Two behaviours are untested (add tests)

My mutation checks showed these survive. Add:

- **`limit` clamp:** move the query parsing in `audit_get` into a small private function (for example
  `parse_audit_query(&HashMap<String, String>) -> Result<(usize, Option<i64>), String>`) and unit-test it:
  no `limit` -> 50; `500` -> 500; `501` -> 500; `100000` -> 500; `0`, `-1` and `abc` -> error; `before`
  `0`, `-5` and `abc` -> error; valid `before` passes through. The handler behaviour does not change.
- **Unchanged values write nothing:** a settings test that sets `timezone` to `Asia/Kolkata`, then sets it
  to `Asia/Kolkata` again, and asserts exactly one `settings_update` row exists (plus a PUT of an empty
  object `{}` adds none).

## Do not

- Do not add dependencies or change `Cargo.toml` / `Cargo.lock`.
- Do not change the migration, the triggers, `audit.rs` or the settings schema.
- Do not amend commit b9b934e; make a new commit.

## Done when

`cd src-tauri && cargo test --no-default-features` (0 warnings), `python -m pytest -q` and
`node tools/live_check.mjs` pass, `node tools/gen_timezones.mjs` reproduces `timezones.rs` byte for byte,
and the new commit message starts with `T003:`.
