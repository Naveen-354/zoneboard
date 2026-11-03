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

def run_node_eval(script: str) -> subprocess.CompletedProcess:
    bootstrap = (
        "globalThis.matchMedia = globalThis.matchMedia || (() => ({ matches: false }));\n"
    )
    return subprocess.run(
        [NODE, "--input-type=module", "-e", bootstrap + script],
        capture_output=True,
        text=True,
        cwd=HERE,
        timeout=30,
    )


def test_api_sends_token_from_meta_and_preserves_custom_headers():
    script = """
    let capturedHeaders = null;
    globalThis.document = {
        querySelector: (sel) => (sel === 'meta[name="zb-token"]' ? { getAttribute: (attr) => (attr === 'content' ? 'abc' : null) } : null)
    };
    globalThis.fetch = async (url, opts) => {
        capturedHeaders = Object.fromEntries(opts.headers.entries());
        return new Response(JSON.stringify({ ok: true }), { status: 200, headers: { 'Content-Type': 'application/json' } });
    };
    const { api } = await import('./static/js/util.js');
    const res = await api('/api/test', { headers: { 'Content-Type': 'application/json' } });
    console.log(JSON.stringify({ res, headers: capturedHeaders }));
    """
    res = run_node_eval(script)
    assert res.returncode == 0, res.stderr
    out = json.loads(res.stdout)
    assert out["headers"].get("x-zb-token") == "abc"
    assert out["headers"].get("content-type") == "application/json"
    assert out["res"] == {"ok": True}


@pytest.mark.parametrize("meta_content", [None, ""])
def test_api_omits_token_header_when_meta_missing_or_empty(meta_content):
    meta_json = json.dumps(meta_content)
    script = f"""
    let capturedHeaders = null;
    const metaVal = {meta_json};
    globalThis.document = {{
        querySelector: (sel) => (sel === 'meta[name="zb-token"]' && metaVal !== null
            ? {{ getAttribute: (attr) => (attr === 'content' ? metaVal : null) }}
            : null)
    }};
    globalThis.fetch = async (url, opts) => {{
        capturedHeaders = Object.fromEntries(opts.headers.entries());
        return new Response(JSON.stringify({{ ok: true }}), {{ status: 200 }});
    }};
    const {{ api }} = await import('./static/js/util.js');
    await api('/api/test');
    console.log(JSON.stringify({{ hasToken: 'x-zb-token' in capturedHeaders }}));
    """
    res = run_node_eval(script)
    assert res.returncode == 0, res.stderr
    out = json.loads(res.stdout)
    assert out["hasToken"] is False


def test_api_throws_reload_the_page_on_401():
    script = """
    globalThis.document = { querySelector: () => null };
    globalThis.fetch = async () => new Response(JSON.stringify({ error: "unauthorized" }), { status: 401 });
    const { api } = await import('./static/js/util.js');
    try {
        await api('/api/test');
        console.log(JSON.stringify({ threw: false }));
    } catch (err) {
        console.log(JSON.stringify({ threw: true, message: err.message }));
    }
    """
    res = run_node_eval(script)
    assert res.returncode == 0, res.stderr
    out = json.loads(res.stdout)
    assert out["threw"] is True
    assert "reload the page" in out["message"]


def test_api_non_401_error_and_200_json():
    script = """
    globalThis.document = { querySelector: () => null };
    globalThis.fetch = async (url) => {
        if (url === '/api/fail') {
            return new Response(JSON.stringify({ error: "boom" }), { status: 400 });
        }
        return new Response(JSON.stringify({ ok: true, count: 42 }), { status: 200 });
    };
    const { api } = await import('./static/js/util.js');
    let thrownError = null;
    try {
        await api('/api/fail');
    } catch (err) {
        thrownError = err.message;
    }
    const success = await api('/api/ok');
    console.log(JSON.stringify({ thrownError, success }));
    """
    res = run_node_eval(script)
    assert res.returncode == 0, res.stderr
    out = json.loads(res.stdout)
    assert out["thrownError"] == "boom"
    assert out["success"] == {"ok": True, "count": 42}

