# Claude and Antigravity workflow

Goal: Antigravity does most of the implementation; Claude owns architecture, specs, review and re-planning, and spends as little as possible. Shared context lives in files, not in anyone's memory.

## 1. What is actually available (verified 2026-11-02)

- Antigravity's CLI `agy` v1.4.12 is installed at `%LOCALAPPDATA%\agy\bin\agy.exe`.
- It has a **non-interactive mode**: `agy --print "<prompt>"`, with `--mode plan|accept-edits|auto`, `--sandbox`, `--effort`, `--model`, `--output-format text|json|stream-json`, `--print-timeout`.
- A read-only smoke test in `plan` mode worked: it ran in 24 s, **loaded `AGENTS.md` by itself**, answered correctly and changed nothing.
- No MCP connector or other agent integration for Antigravity exists in Claude's tools. The channel is the command line, git and these files.

So there are two real ways to delegate. Claude uses **A** and tells you what it dispatched; **B** is the fallback and for work you want to watch.

- **A. Dispatch by script:** `tools/delegate.ps1` creates a git worktree and branch for the task and runs `agy --print` in it. Output goes to a log file.
- **B. Manual:** open the worktree in Antigravity and paste the prompt printed by `tools/delegate.ps1 -DryRun`.

**Evidence rule:** a task counts as delegated only if there is a log file, a branch `task/Txxx` and (when finished) a commit hash recorded in `PROGRESS.md`. Claude never reports work as delegated or done without them.

## 2. Task lifecycle

```
SPEC (Claude) -> PLAN (agy, plan mode) -> approve -> IMPLEMENT (agy, accept-edits) -> SELF-CHECK -> REVIEW (Claude) -> MERGE (you) -> PROGRESS
```

1. **Spec.** Claude writes `docs/platform/tasks/Txxx-name.md`: goal, scope, out of scope, acceptance criteria, tests, "Touches". Short and exact.
2. **Plan.** `tools/delegate.ps1 -Task Txxx -Mode plan`. Antigravity reads the spec and `AGENTS.md` and writes a plan; nothing is edited. (A plan run took about 7 minutes in rehearsal; the full plan is an artifact under `%USERPROFILE%\.gemini\antigravity-cli\brain\`, linked from the log. Use `-Effort medium` for simple tasks to save time.) Claude (or you) reads it, corrects it, approves it. Skip only for trivial tasks.
3. **Implement.** `tools/delegate.ps1 -Task Txxx -Mode accept-edits`. Antigravity edits inside the worktree, runs the checks from `AGENTS.md`, commits on `task/Txxx`, and ends with the report in section 5.
4. **Review.** Claude does not re-read all the code. It reads the report, `git diff --stat`, reruns the three checks in the worktree, and **reads in full only the sensitive parts** (vault, API auth, order manager, risk engine, anything touching money or keys). It mutation-checks new tests (break the code, confirm they fail). Findings go back to Antigravity as a corrections list; Claude does not rewrite working code.
5. **Merge.** You approve. Merge in dependency order (`git merge --no-ff task/Txxx`), then remove the worktree.
6. **Progress.** Claude updates `PROGRESS.md`: status, branch, commit, what to know next.

## 3. Commands

```powershell
.\tools\delegate.ps1 -Task T001 -Mode plan -DryRun    # show the prompt and the command, run nothing
.\tools\delegate.ps1 -Task T001 -Mode plan            # read-only plan
.\tools\delegate.ps1 -Task T001 -Mode accept-edits    # implement in the worktree
```

- Worktrees live next to the repo: `..\zoneboard-wt\Txxx` (branch `task/Txxx`). Logs: `..\zoneboard-wt\logs\`.
- The script refuses to run if the task spec is not in the last commit (a worktree only contains committed files), so **commit before delegating**.
- Run at most 2 or 3 tasks at once, and only tasks whose "Touches" don't overlap. Otherwise merge order matters.

## 4. Safety rules for delegated runs

- A separate worktree means Antigravity never sees your working copy, `.env`, `journal.db` or token files (they are git-ignored, so they are not in the worktree).
- `--sandbox` is on by default (terminal restrictions). If a task legitimately needs more, loosen it deliberately with `-NoSandbox`. Never use `--dangerously-skip-permissions`.
- **No real credentials in prompts, specs, fixtures or logs.** Antigravity never receives a real API key. Tests use fakes, recorded fixtures or the Binance Spot Testnet with throwaway keys the owner creates.
- **Agents never place real orders and never run live verification.** Only the owner does, manually, after the gates in `ROADMAP.md`.
- **Known limitation (found 2026-11-03):** `--sandbox` stops `cargo` working on Windows (`os error 5` when it writes `target/`), and the sandbox does not see your Python either. Build-and-test tasks therefore need `-NoSandbox`. Without a sandbox the agent can read any file your Windows user can, not just the worktree. That is acceptable **only while no real credentials exist on this machine**. Before the first real API key is created (T007 and later), do one of: keep to throwaway testnet keys the owner creates, run delegated tasks under a separate Windows user, or find an `agy --settings` configuration that allows only the worktree, cargo's cache and the temp folder. Track this in `PROGRESS.md`.
- The code and prompts you send go to Google's servers. That is your choice of tool; just keep secrets out of everything the agent can read.
- A task must stop and ask if the spec is wrong, an `UNVERIFIED` fact turns out different, or it would need a dependency that isn't pre-approved (`ROADMAP.md` D8).

## 5. Report format (the last thing Antigravity writes)

```
## Report: Txxx
Summary: <2-4 lines>
Files changed: <list, from git diff --stat>
Checks run: <exact commands and pass/fail counts>
Spec deviations: <none, or each one with the reason>
Verified against official docs: <fact: source URL>, or "n/a"
Open questions / risks: <list>
Commit: <hash on task/Txxx>
```

## 6. Review checklist (Claude)

1. Scope: only the spec's items changed; no new dependency or outbound host beyond what was approved.
2. Spec fidelity: every UNVERIFIED item was confirmed against official docs and recorded.
3. Secrets: grep the diff and fixtures for keys, tokens, secrets; read the redaction tests.
4. Tests can fail: mutate the code under each new test and confirm it goes red.
5. Money: decimals, not floats; fees and rounding explicit.
6. No regressions: all three suites green in the worktree, parity tests untouched, no new compiler warnings.
7. The agent's report matches its log.

## 7. How Claude keeps its own usage low

- Specs are written once, short, and reused as the contract; later tasks are specified just in time.
- Review reads the report, the diff stat and test output first; full reads only for the sensitive paths.
- Claude re-plans from `PROGRESS.md`, not from re-reading the repo.
- Claude does not rewrite or "improve" working code; it assigns corrections.
- Repetitive work (UI pages, CRUD endpoints, tests, docs) always goes to Antigravity.
- Claude writes code itself only for: the delegation tooling, spec examples, and small fixes where a round trip would cost more than the fix.

## 8. `PROGRESS.md` entry format

`| Txxx | status (spec / planned / in progress / review / merged / blocked) | branch | commit | note |` plus a short "next" list at the top.
