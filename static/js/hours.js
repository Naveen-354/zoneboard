// Market hours. No DOM or network here on purpose, so Node can test it (test_ui_logic.py).

/**
 * Is the NSE cash session open: Mon to Fri, 09:15 to 15:30 IST (open at 09:15, closed from 15:30).
 * Exchange holidays aren't known here, so a holiday reads as open (its prices just won't move).
 */
export function nseOpen(now = Date.now()) {
  const ist = new Date(now + 5.5 * 3600e3);
  const day = ist.getUTCDay();
  const mins = ist.getUTCHours() * 60 + ist.getUTCMinutes();
  return day >= 1 && day <= 5 && mins >= 9 * 60 + 15 && mins < 15 * 60 + 30;
}
