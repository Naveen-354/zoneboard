"""Free market data via Yahoo Finance (yfinance).

Important:
  * Data is unofficial and delayed (often ~15 minutes). Fine for learning and
    backtesting, not for live execution.
  * Yahoo's terms limit it to personal, non-commercial use.
  * Set SAMPLE_DATA=1 to use made-up random sample data (no network; for development and tests only).
"""
from __future__ import annotations

import os
import re
import threading
import time
import zlib
from pathlib import Path

import numpy as np
import pandas as pd

SAMPLE = os.environ.get("SAMPLE_DATA") == "1"
HERE = Path(__file__).parent
IST = "Asia/Kolkata"

# interval -> (yahoo period, seconds to cache)
INTERVALS = {
    "1d": ("1y", 300),
    "1h": ("6mo", 300),
    "15m": ("1mo", 120),
}
_cache: dict = {}
_lock = threading.Lock()


def _read_symbols(path: Path, pattern: str) -> list[str]:
    syms = []
    for line in path.read_text().splitlines():
        line = line.strip().upper()
        if line and not line.startswith("#") and re.fullmatch(pattern, line):
            syms.append(line)
    return list(dict.fromkeys(syms))


def load_symbols() -> list[str]:
    """NSE symbols for the scanner (symbols.txt)."""
    return _read_symbols(HERE / "symbols.txt", r"[A-Z0-9&\-]{1,20}")


def load_crypto_symbols() -> list[str]:
    """Binance spot pairs for the live crypto board (symbols_crypto.txt)."""
    return _read_symbols(HERE / "symbols_crypto.txt", r"[A-Z0-9]{5,20}")


def yf_ticker(sym: str) -> str:
    return sym + ".NS"


def epoch_seconds(index: pd.DatetimeIndex) -> np.ndarray:
    """Bar times as true unix seconds (UTC). The browser shows them in the viewer's own time zone.

    Naive timestamps (the synthetic sample data) are taken to be India time.
    """
    idx = pd.DatetimeIndex(index)
    idx = idx.tz_localize(IST) if idx.tz is None else idx
    idx = idx.tz_convert("UTC").tz_localize(None)
    return ((idx - pd.Timestamp("1970-01-01")) // pd.Timedelta(seconds=1)).to_numpy()


def _clean(d: pd.DataFrame) -> pd.DataFrame | None:
    cols = ["Open", "High", "Low", "Close", "Volume"]
    if d is None or d.empty or any(c not in d.columns for c in cols):
        return None
    d = d[cols].dropna()
    d = d[(d["High"] >= d["Low"]) & (d["Close"] > 0)]
    return d if len(d) else None


def _synthetic(sym: str, interval: str) -> pd.DataFrame:
    rng = np.random.default_rng(zlib.crc32(f"{sym}{interval}".encode()))
    if interval == "1d":
        idx = pd.bdate_range(end=pd.Timestamp.today().normalize(), periods=260)
        vol_scale = 0.015
    else:
        per_day = 25 if interval == "15m" else 7
        step = "15min" if interval == "15m" else "60min"
        days = pd.bdate_range(end=pd.Timestamp.today().normalize(), periods=22 if interval == "15m" else 120)
        idx = pd.DatetimeIndex([d + pd.Timedelta(hours=9, minutes=15) + i * pd.Timedelta(step) for d in days for i in range(per_day)])
        vol_scale = 0.004 if interval == "15m" else 0.007
    n = len(idx)
    base = 100 + (zlib.crc32(sym.encode()) % 3000)
    rets = rng.normal(0, vol_scale, n)
    close = base * np.exp(np.cumsum(rets))
    open_ = np.r_[close[0], close[:-1]] * (1 + rng.normal(0, vol_scale / 4, n))
    span = np.abs(rng.normal(0, vol_scale / 2, n)) * close
    high = np.maximum(open_, close) + span
    low = np.minimum(open_, close) - span
    volume = rng.lognormal(13, 0.4, n)
    return pd.DataFrame({"Open": open_, "High": high, "Low": low, "Close": close, "Volume": volume}, index=idx)


def get_ohlcv(sym: str, interval: str = "1d", sample: bool | None = None) -> pd.DataFrame | None:
    """Bars for one symbol. `sample` picks synthetic data for this call; None follows the SAMPLE_DATA=1 environment."""
    if interval not in INTERVALS:
        raise ValueError("bad interval")
    sample = SAMPLE if sample is None else sample
    key = ("one", sym, interval, sample)
    ttl = INTERVALS[interval][1]
    with _lock:
        hit = _cache.get(key)
        if hit and time.time() - hit[0] < ttl:
            return hit[1]
    if sample:
        df = _synthetic(sym, interval)
    else:
        import yfinance as yf

        period = INTERVALS[interval][0]
        try:
            raw = yf.Ticker(yf_ticker(sym)).history(period=period, interval=interval, auto_adjust=False)
        except Exception:
            raw = None
        df = _clean(raw)
    with _lock:
        _cache[key] = (time.time(), df)
    return df


def daily_all(symbols: list[str] | None = None, period: str = "1y", sample: bool | None = None) -> dict[str, pd.DataFrame]:
    """Daily bars for many symbols in one request."""
    symbols = symbols or load_symbols()
    sample = SAMPLE if sample is None else sample
    key = ("all", period, tuple(symbols), sample)
    with _lock:
        hit = _cache.get(key)
        if hit and time.time() - hit[0] < 300:
            return hit[1]
    out: dict[str, pd.DataFrame] = {}
    if sample:
        for s in symbols:
            out[s] = _synthetic(s, "1d")
    else:
        import yfinance as yf

        raw = yf.download(
            [yf_ticker(s) for s in symbols],
            period=period,
            interval="1d",
            group_by="ticker",
            auto_adjust=False,
            progress=False,
            threads=True,
        )
        for s in symbols:
            try:
                d = _clean(raw[yf_ticker(s)])
            except KeyError:
                continue
            if d is not None and len(d) > 60:
                out[s] = d
    with _lock:
        _cache[key] = (time.time(), out)
    return out
