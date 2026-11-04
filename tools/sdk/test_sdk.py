"""SDK contract test against the real Rust server on sample data (skipped when the server is not built)."""

import json
import os
import re
import socket
import subprocess
import sys
import tempfile
import time
import urllib.request
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(Path(__file__).parent))
from zoneboard_client import ZoneBoard, ZoneBoardError  # noqa: E402

EXE = ROOT / "src-tauri" / "target" / "debug" / ("zoneboard-server.exe" if os.name == "nt" else "zoneboard-server")


@pytest.fixture(scope="module")
def server():
    if not EXE.exists():
        pytest.skip("server not built (cargo build --no-default-features --bin zoneboard-server)")
    s = socket.socket()
    s.bind(("127.0.0.1", 0))
    port = s.getsockname()[1]
    s.close()
    tmp = tempfile.mkdtemp()
    env = dict(os.environ, SAMPLE_DATA="1", PORT=str(port), ZONEBOARD_DATA=tmp, JOURNAL_DB=os.path.join(tmp, "journal.db"))
    p = subprocess.Popen([str(EXE)], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    base = f"http://127.0.0.1:{port}"
    for _ in range(100):
        try:
            html = urllib.request.urlopen(base + "/", timeout=1).read().decode()
            break
        except OSError:
            time.sleep(0.1)
    launch = re.search(r'name="zb-token" content="([0-9a-f]+)"', html).group(1)
    req = urllib.request.Request(base + "/api/dev/tokens", data=json.dumps({"name": "sdk test", "scopes": ["read", "research"]}).encode(), method="POST", headers={"X-ZB-Token": launch, "Content-Type": "application/json"})
    token = json.loads(urllib.request.urlopen(req).read())["token"]
    yield base, token
    p.kill()


def test_sdk_reads_researches_and_respects_scopes(server):
    base, token = server
    zb = ZoneBoard(base, token)
    assert "zbk_…" in repr(zb) and token not in repr(zb)
    assert "/api/backtest" in zb.openapi()["paths"]
    spec = next(t for t in zb.library()["templates"] if t["id"] == "ema_cross")["spec"]
    r = zb.backtest("BTCUSDT", spec, bars=400)
    assert r["data"]["sample"] is True and r["metrics"]["trades"] >= 0
    assert zb.scan("movers", market="india", lookback=5, threshold_pct=0.1)["scanned"] > 0
    assert zb.publish("SDK strategy", spec)["version"] >= 1
    with pytest.raises(ZoneBoardError) as e:
        zb.start_bot(1)  # no trade scope
    assert e.value.status == 403
    with pytest.raises(ValueError):
        ZoneBoard(base, "not-a-token")
