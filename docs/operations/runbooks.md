# Runbooks

**Emergency stop.** Bot Studio > My Bots > Kill switch (or `POST /api/risk/kill {"engaged": true}`): stops every
bot, closes bot positions (paper at the latest price, broker with SELL orders through the outbox), disarms live
trading, blocks new orders until released. Restarting the app also pauses every bot and disarms live trading.

**An order's outcome is unknown.** The bot stops itself. `GET /api/system/health` shows unreconciled orders; the
runner looks them up at the exchange every tick. If the exchange cannot answer, check the order on Binance by
its client order id (`zb{bot}-{bar}-{n}`) before restarting the bot.

**Stale data.** Bots make no new entries; their log says "market data is stale". Check `/api/market-data/health`.

**Rate limited by Binance.** The account pauses for the `Retry-After` time; calls fail fast meanwhile. Nothing
retries in a loop. Reduce running bots or their timeframe.

**Backup and restore.** Close the app; copy `journal.db` (and keep the copy). Restore by copying it back. A
newer schema is refused by an older build, so keep the build that matches the backup. Credentials live in Windows
Credential Manager under the `zoneboard` service, not in the backup.

**Rotate a key.** Accounts > Disconnect (deletes the vault entries), create a new key at Binance (no
withdrawals, IP-restricted), Connect. Developer tokens: Settings > Developer > Revoke.
