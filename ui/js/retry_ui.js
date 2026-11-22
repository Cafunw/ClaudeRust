// The line shown while the backend waits to retry a failed request
// ("generation-retry": conversation_id, attempt, max_attempts, wait_ms, reason).
import { t } from './i18n.js';

const WHY = { rate_limited: 'retry_rate_limited', overloaded: 'retry_overloaded', server_error: 'retry_server_error', network: 'retry_network' };

/** "Rate limited, retrying in 12 s (2/5)" for retry event `ev` with `secs`
 *  seconds left; "retrying now" once they're up. Missing fields leave
 *  their part out. */
export function retryLabel(ev, secs) {
  const why = t(WHY[ev && ev.reason] || 'retry_generic');
  const text = secs > 0 ? t('retry_in', { why, s: secs }) : t('retry_now', { why });
  const n = Number(ev && ev.attempt);
  const max = Number(ev && ev.max_attempts);
  return n > 0 && max > 0 ? `${text} (${n}/${max})` : text;
}
