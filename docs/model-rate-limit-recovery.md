# Model rate-limit recovery

Native Engine model calls use a separate HTTP 429 recovery policy. These are
initial engineering defaults, not a claim that providers recover within ten
minutes. Ordinary network/5xx retries retain their existing configuration.

| Setting | Default | Meaning |
| --- | --- | --- |
| `max_attempts` | 20 | Total wire attempts, including the first, per logical operation |
| `recovery_budget_secs` | 600 | Active recovery window starting at the first refusal |
| `initial_delay_secs` | 5 | First exponential backoff base |
| `max_delay_secs` | 60 | Local exponential base ceiling; server minima may exceed it |

Backoff doubles and uses equal jitter between 50% and 100% of the local base.
The next attempt waits for the greater of that delay and the server minimum.
`Retry-After` accepts numeric seconds and an HTTP-date. Known structured retry
and reset fields are read before the diagnostic body is summarized. A hint that
does not fit the remaining budget parks the operation immediately; it is never
shortened to 60 seconds. A provider that is generating normally is not timed
out by the 429 budget. Existing header, stream-idle and generation controls
still apply; backoff does not consume their active-work allowance.

Pinvou settings expose `model_rate_limit_retry` in `~/.pinvou3/settings.json`:

```json
{
  "model_rate_limit_retry": {
    "max_attempts": 20,
    "recovery_budget_secs": 600,
    "initial_delay_secs": 5,
    "max_delay_secs": 60
  }
}
```

Policy changes apply to newly created clients; restart the application after
editing this setting. Existing settings use the defaults. Attempts must be 1–1000, the budget
1–86400 seconds, and delays must satisfy `1 <= initial <= max <= 86400`.
Standalone CodeWhale uses the same fields under `[retry.rate_limit]`.
`retry.enabled = false` still disables automatic transport retries.

Transient throttling retries; explicit account/billing failures and invalid
parameters require user action. Known quota windows preserve the checkpoint
without short retries. In particular, DashScope's `insufficient_quota` on its
official endpoint is not globally interpreted as exhausted billing credit;
OpenAI's explicit quota classification remains intact. MiniMax and Zhipu
numeric error envelopes are classified on their own provider surfaces,
including successful HTTP JSON envelopes carrying a logical error. Unknown
429s use the bounded transient policy.

Within the process, independent clients using the same endpoint, credential and
account/project headers share cooldown, request admission and a recovery-episode
attempt ceiling. All models on that credential conservatively share one quota
group. Separate accounts/endpoints are isolated. Admission starts at the explicit
provider concurrency limit, or 16 if no limit is configured; refusals halve it
and three successful admissions restore one slot. Existing child launch
scheduling reuses the same dynamic semaphore primitive. A successful HTTP
response ends the shared refusal episode; each logical operation retains its
own recovery budget, and the separate success streak still restores capacity
gradually. HTTP request slots are
released during backoff. Child wrappers do not restart an exhausted 429 budget.

Provider minimum deadlines persist under `~/.pinvou3/rate-limit-recovery/`,
keyed by a digest of the quota scope, without credentials or request content.
Reset persistence does not start tasks. Pinvou's current interactive task policy
uses user-driven continuation after an interrupted checkpoint; it does not
schedule automatic continuation. Cancellation drops the HTTP/backoff future,
and late foreground receipts are checked against the current turn. Completed
tool results stay in the transcript and logical-call idempotency identity stays
stable across retries. Mid-stream recovery continues to use the existing stream
recovery contract. Dedicated vision calls reuse native quota admission and the configured policy;
matching routes keep their frozen context headers, while a different vision
endpoint never inherits them. Tool-owned auxiliary calls keep their existing
tool error/deadline contract when recovery stops; they do not replay preceding
tools. Auto request-preview classifiers retain their
existing isolated cache/admission/connection-health and foreground-banner
contract; they use the same bounded refusal classification and server-minimum
policy without changing foreground quota state.

External ACP agents retain their own retry policy; the host
does not resend their prompts.

The desktop/web bridges and native code conversations show localized waiting
and interrupted notices with attempt count and the next attempt/reset time.
Content-free recovery receipts are retained with the local timing timeline;
the terminal event provides the end of cancelled or failed observations.
Off-turn/background child observations persist in an independent `agent:<id>`
group with their operation identity; typed child terminal events settle live
waiting notices without waiting for the parent turn to end. Foreground completion
does not dismiss a still-waiting detached child. These groups do
not create foreground user turns or attach a child receipt to a later turn.
Operational tuning should use recovery percentiles together with exhausted and
cancelled observations, rather than success-only averages. Known provider-code
classification, attempt counts, delay and stop/recovery state should be used
without collecting prompts, tool arguments or credentials.

Public provider references:

- [DeepSeek error codes](https://api-docs.deepseek.com/quick_start/error_codes)
- [DeepSeek rate limits](https://api-docs.deepseek.com/quick_start/rate_limit)
- [MiniMax China error codes](https://platform.minimax.cn/docs/api-reference/errorcode)
- [MiniMax M-Plan quota windows](https://platform.minimax.cn/docs/m-plan/faq)
- [Zhipu error codes](https://docs.bigmodel.cn/cn/api/api-code)
- [Ark burst handling](https://docs.volcengine.com/docs/ark/traffic-burst-handling-best-practices?lang=zh)

These references describe provider limits and recovery signals. They do not
establish a universal ten-minute recovery percentile.
