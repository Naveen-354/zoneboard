"""ZoneBoard developer SDK: a small, dependency-free client for the local API.

Create a token in ZoneBoard (Settings > Developer), pick only the scopes you need, then:

    from zoneboard_client import ZoneBoard
    zb = ZoneBoard("http://127.0.0.1:5000", token="zbk_...")
    lib = zb.library()
    report = zb.backtest(symbol="BTCUSDT", interval="1h", spec=lib["templates"][0]["spec"], bars=1000)
    print(report["metrics"]["return_pct"])

Scopes: read (GET), research (backtests, scans, strategies, AI drafts), trade (start/pause/stop bots, paper
orders). A developer token can never touch broker credentials, approvals, live arming, risk limits or the
kill switch: those stay with the person at the app. Strategies are declarative JSON (no code runs inside
ZoneBoard); this SDK helps build, test and version them.
"""

from __future__ import annotations

import json
import urllib.error
import urllib.request
from typing import Any, Dict, List, Optional

__all__ = ["ZoneBoard", "ZoneBoardError"]


class ZoneBoardError(Exception):
    """An error reply from ZoneBoard: `status` is the HTTP status, the message is the server's own text."""

    def __init__(self, status: int, message: str):
        super().__init__(f"{status}: {message}")
        self.status = status


class ZoneBoard:
    def __init__(self, base_url: str = "http://127.0.0.1:5000", token: str = "", timeout: float = 120.0):
        if not token.startswith("zbk_"):
            raise ValueError("pass a developer token (it starts with zbk_)")
        self.base = base_url.rstrip("/")
        self._token = token
        self.timeout = timeout

    def __repr__(self) -> str:  # never show the token
        return f"ZoneBoard({self.base!r}, token='zbk_…{self._token[-4:]}')"

    def _call(self, method: str, path: str, body: Optional[Dict[str, Any]] = None) -> Any:
        data = None if body is None else json.dumps(body).encode()
        req = urllib.request.Request(self.base + path, data=data, method=method)
        req.add_header("Authorization", f"Bearer {self._token}")
        if data is not None:
            req.add_header("Content-Type", "application/json")
        try:
            with urllib.request.urlopen(req, timeout=self.timeout) as r:
                raw = r.read()
                ctype = r.headers.get("Content-Type", "")
                return json.loads(raw) if "json" in ctype else raw.decode()
        except urllib.error.HTTPError as e:
            try:
                msg = json.loads(e.read()).get("error", e.reason)
            except Exception:  # noqa: BLE001 - any unreadable body
                msg = str(e.reason)
            raise ZoneBoardError(e.code, msg) from None

    # read
    def openapi(self) -> Dict[str, Any]:
        return self._call("GET", "/api/openapi.json")

    def library(self) -> Dict[str, Any]:
        return self._call("GET", "/api/strategies/library")

    def bots(self) -> List[Dict[str, Any]]:
        return self._call("GET", "/api/bots")["bots"]

    def portfolio(self) -> Dict[str, Any]:
        return self._call("GET", "/api/portfolio")

    def strategies(self) -> List[Dict[str, Any]]:
        return self._call("GET", "/api/strategies")["strategies"]

    # research
    def backtest(self, symbol: str, spec: Dict[str, Any], interval: str = "1h", market: str = "crypto", bars: int = 1000, **cost: Any) -> Dict[str, Any]:
        return self._call("POST", "/api/backtest", {"symbol": symbol, "spec": spec, "interval": interval, "market": market, "bars": bars, **cost})

    def optimize(self, symbol: str, spec: Dict[str, Any], params: List[Dict[str, Any]], objective: str = "sharpe", split: float = 0.7, **kw: Any) -> Dict[str, Any]:
        return self._call("POST", "/api/backtest/optimize", {"symbol": symbol, "spec": spec, "params": params, "objective": objective, "split": split, **kw})

    def walk_forward(self, symbol: str, spec: Dict[str, Any], params: List[Dict[str, Any]], folds: int = 4, **kw: Any) -> Dict[str, Any]:
        return self._call("POST", "/api/backtest/walkforward", {"symbol": symbol, "spec": spec, "params": params, "folds": folds, **kw})

    def scan(self, template: str, market: str = "crypto", interval: Optional[str] = None, **params: Any) -> Dict[str, Any]:
        body: Dict[str, Any] = {"template": template, "market": market, "params": params}
        if interval:
            body["interval"] = interval
        return self._call("POST", "/api/scanner/run", body)

    def publish(self, name: str, spec: Dict[str, Any], note: str = "") -> Dict[str, Any]:
        return self._call("POST", "/api/strategies", {"name": name, "spec": spec, "note": note})

    def import_pack(self, pack: Dict[str, Any]) -> Dict[str, Any]:
        return self._call("POST", "/api/strategies/pack", pack)

    # trade (paper and approved bots only; the server enforces every gate)
    def start_bot(self, bot_id: int) -> Dict[str, Any]:
        return self._call("POST", f"/api/bots/{bot_id}/start")

    def pause_bot(self, bot_id: int) -> Dict[str, Any]:
        return self._call("POST", f"/api/bots/{bot_id}/pause")

    def stop_bot(self, bot_id: int, close_positions: bool = False) -> Dict[str, Any]:
        return self._call("POST", f"/api/bots/{bot_id}/stop", {"policy": "close_positions" if close_positions else "keep_positions"})
