<#
.SYNOPSIS
  Hand a task spec (docs/platform/tasks/Txxx-*.md) to Antigravity (agy), in its own git worktree.

.DESCRIPTION
  Creates ..\zoneboard-wt\<Task> on branch task/<Task> from HEAD, then runs `agy --print` there.
  The worktree contains only committed files, so .env, journal.db and token files are never visible to the agent.
  See docs/platform/WORKFLOW.md. Never uses --dangerously-skip-permissions.

.EXAMPLE
  .\tools\delegate.ps1 -Task T001 -Mode plan -DryRun
  .\tools\delegate.ps1 -Task T001 -Mode plan
  .\tools\delegate.ps1 -Task T001 -Mode accept-edits
#>
param(
  [Parameter(Mandatory)][ValidatePattern('^T\d{3}$')][string]$Task,
  [ValidateSet('plan', 'accept-edits')][string]$Mode = 'plan',
  [ValidateSet('low', 'medium', 'high', 'xhigh', 'max')][string]$Effort = 'high',
  [switch]$NoSandbox,
  [switch]$DryRun
)
$ErrorActionPreference = 'Continue' # native stderr (git progress, agy output) must not abort: exit codes are checked explicitly

$root = (git rev-parse --show-toplevel).Trim()
$spec = Get-ChildItem (Join-Path $root 'docs/platform/tasks') -Filter "$Task-*.md" -ErrorAction SilentlyContinue | Select-Object -First 1
if (-not $spec) { throw "No spec docs/platform/tasks/$Task-*.md. Claude writes the spec first." }
$specRel = "docs/platform/tasks/$($spec.Name)"

# a worktree is made from a commit: the spec, AGENTS.md and the code the task builds on must be committed
git cat-file -e "HEAD:$specRel" 2>&1 | Out-Null
$inCommit = ($LASTEXITCODE -eq 0)
if (-not $inCommit) { throw "$specRel is not in the last commit. Commit first: a worktree only contains committed files." }
if (git status --porcelain) { Write-Warning "You have uncommitted changes. The agent works from the last commit and will not see them." }

$agy = (Get-Command agy -ErrorAction SilentlyContinue).Source
if (-not $agy) { $agy = Join-Path $env:LOCALAPPDATA 'agy\bin\agy.exe' }
if (-not (Test-Path $agy)) { throw "agy not found. Install Antigravity's CLI first." }

$base = Join-Path (Split-Path $root -Parent) 'zoneboard-wt'
$wt = Join-Path $base $Task
$logs = Join-Path $base 'logs'
$branch = "task/$Task"

$what = if ($Mode -eq 'plan') {
  "PLAN ONLY. Do not edit any file and do not run commands that change anything. Write a concrete implementation plan: files to create or change, order of work, tests to add, risks, and every question you have. Quote the spec's acceptance criteria and say how each will be met."
} else {
  "IMPLEMENT the task. Follow the spec's scope exactly and touch only what it lists (explain any other file you must change). Run the three checks from AGENTS.md, fix failures, then commit on this branch with a message starting '${Task}:'. Never run git push."
}
$prompt = @"
You are working on task $Task in this git worktree (branch $branch). Read AGENTS.md, docs/platform/WORKFLOW.md and the task spec $specRel completely first.

$what

Hard rules: never touch .env, journal.db or any token file; never use or ask for real API keys or broker credentials; never place real orders; do not add dependencies outside the pre-approved list in docs/platform/ROADMAP.md (D8) without stopping to ask. If the spec seems wrong or something it marks UNVERIFIED turns out different, stop and say so.

Finish with the report in the format given in docs/platform/WORKFLOW.md section 5.
"@

$argv = @('--print', $prompt, '--mode', $Mode, '--effort', $Effort, '--print-timeout', '0s', '--output-format', 'text')
if (-not $NoSandbox) { $argv += '--sandbox' }

if ($DryRun) {
  Write-Host "task: $Task   mode: $Mode   spec: $specRel"
  Write-Host "worktree: $wt   branch: $branch   sandbox: $(-not $NoSandbox)"
  Write-Host "--- prompt ---`n$prompt"
  return
}

New-Item -ItemType Directory -Force $logs | Out-Null
if (-not (Test-Path $wt)) {
  git worktree add -q -b $branch $wt HEAD 2>&1 | Out-Null
  if ($LASTEXITCODE -ne 0) { throw "git worktree add failed for $wt (branch $branch may already exist: remove it with 'git branch -D $branch' if it is stale)" }
  Write-Host "created worktree $wt on $branch"
}
$stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
$log = Join-Path $logs "$Task-$Mode-$stamp.log"
Write-Host "running agy ($Mode) in $wt ... log: $log"
Push-Location $wt
try { & $agy @argv 2>&1 | ForEach-Object { $_.ToString() } | Tee-Object -FilePath $log } finally { Pop-Location }

Write-Host "`n--- result ---"
Write-Host "log: $log"
git -C $wt status --short
git -C $wt log --oneline -n 3
git -C $wt diff --stat "main...$branch" 2>$null
