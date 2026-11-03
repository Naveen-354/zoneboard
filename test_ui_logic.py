"""Tests for browser logic that has no DOM, run in Node (skipped if Node is not installed).

The order ticket refuses live-price orders while the NSE is closed, so the hours check must be exact.
"""
import json
import shutil
import subprocess
from pathlib import Path

import pytest

HERE = Path(__file__).parent
NODE = shutil.which("node")

pytestmark = pytest.mark.skipif(NODE is None, reason="node is not installed")

# IST = UTC+5:30. 2026-11-02 is a Monday. Each case: (UTC timestamp, expected open?)
CASES = [
    ("2026-11-02T03:44:00Z", False),  # 09:14 IST Mon: before the open
    ("2026-11-02T03:45:00Z", True),  # 09:15 IST Mon: opens
    ("2026-11-02T06:30:00Z", True),  # 12:00 IST Mon
    ("2026-11-02T09:59:00Z", True),  # 15:29 IST Mon: last minute
    ("2026-11-02T10:00:00Z", False),  # 15:30 IST Mon: closed
    ("2026-11-02T15:39:00Z", False),  # 21:09 IST Mon: the evening this was built
    ("2026-11-06T06:30:00Z", True),  # Friday midday
    ("2026-11-07T06:30:00Z", False),  # Saturday midday
    ("2026-11-08T06:30:00Z", False),  # Sunday midday
    ("2026-11-02T22:00:00Z", False),  # 03:30 IST Tue: before the open
    ("2026-11-09T03:45:00Z", True),  # next Monday 09:15 IST
]


def test_nse_open_matches_the_session_hours():
    script = (
        "import { nseOpen } from './static/js/hours.js';"
        f"const cases = {json.dumps(CASES)};"
        "console.log(JSON.stringify(cases.map(([t]) => nseOpen(Date.parse(t)))));"
    )
    res = subprocess.run([NODE, "--input-type=module", "-e", script], capture_output=True, text=True, cwd=HERE, timeout=30)
    assert res.returncode == 0, res.stderr
    got = json.loads(res.stdout)
    for (t, want), g in zip(CASES, got):
        assert g is want, f"{t}: expected open={want}, got {g}"
